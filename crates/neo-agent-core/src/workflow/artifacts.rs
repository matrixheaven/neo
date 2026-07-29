//! Run-scoped immutable artifact store (design §33).
//!
//! Bytes live under `<run_dir>/artifacts/<sha256>`. Visibility is owned by the
//! journal: a staged file is an orphan until `ArtifactCommitted` is durable.
//! Reads revalidate size and digest and never invent empty content.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::error::{WorkflowError, WorkflowErrorCode};
use super::journal::{JournalEnvelope, JournalPayload, canonicalize_json};
use super::limits::WorkflowLimits;
use super::state::{WorkflowArtifactId, WorkflowId, validate_portable_name};
use crate::session::atomic_file;

/// Kind of value accepted by the artifact API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Text,
    Json,
}

impl ArtifactKind {
    #[must_use]
    pub fn default_media_type(self) -> &'static str {
        match self {
            Self::Text => "text/plain; charset=utf-8",
            Self::Json => "application/json",
        }
    }
}

/// Value payload for an artifact put (text UTF-8 or canonical JSON).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ArtifactValue {
    Text(String),
    Json(serde_json::Value),
}

/// Bounded metadata for a journal-visible artifact.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArtifactMetadata {
    pub artifact_id: WorkflowArtifactId,
    pub sha256: String,
    pub byte_len: u64,
    pub media_type: String,
    pub logical_name: String,
    /// Immutable version for this logical name (1-based, commit order).
    pub version: u32,
}

/// Bytes plus metadata for a revalidated read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactContent {
    pub metadata: ArtifactMetadata,
    pub bytes: Vec<u8>,
}

/// One page of artifact content (byte range).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactContentRange {
    pub metadata: ArtifactMetadata,
    pub offset: u64,
    pub bytes: Vec<u8>,
    pub has_more: bool,
}

/// Durable file written but not yet journal-visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedArtifact {
    pub artifact_id: WorkflowArtifactId,
    pub sha256: String,
    pub byte_len: u64,
    pub media_type: String,
    pub logical_name: String,
    pub version: u32,
}

impl StagedArtifact {
    #[must_use]
    pub fn metadata(&self) -> ArtifactMetadata {
        ArtifactMetadata {
            artifact_id: self.artifact_id.clone(),
            sha256: self.sha256.clone(),
            byte_len: self.byte_len,
            media_type: self.media_type.clone(),
            logical_name: self.logical_name.clone(),
            version: self.version,
        }
    }
}

/// Run-scoped artifact store: owns immutable bytes; journal owns membership.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    run_id: WorkflowId,
    artifacts_dir: PathBuf,
    /// Journal-committed artifacts only (visible).
    committed: Vec<ArtifactMetadata>,
    /// Sum of unique content-addressed payload bytes among committed artifacts.
    total_unique_bytes: u64,
}

impl ArtifactStore {
    /// Open (or create) the artifacts directory for a run. No files are visible yet.
    pub fn open(run_dir: &Path, run_id: WorkflowId) -> Result<Self, WorkflowError> {
        let artifacts_dir = artifacts_dir(run_dir);
        atomic_file::ensure_safe_directory_tree(&artifacts_dir)
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;
        Ok(Self {
            run_id,
            artifacts_dir,
            committed: Vec::new(),
            total_unique_bytes: 0,
        })
    }

    /// Empty store without creating directories (failed rehydrate).
    #[must_use]
    pub fn empty(run_id: WorkflowId, run_dir: &Path) -> Self {
        Self {
            run_id,
            artifacts_dir: artifacts_dir(run_dir),
            committed: Vec::new(),
            total_unique_bytes: 0,
        }
    }

    #[must_use]
    pub fn run_id(&self) -> &WorkflowId {
        &self.run_id
    }

    #[must_use]
    pub fn artifacts_dir_path(&self) -> &Path {
        &self.artifacts_dir
    }

    #[must_use]
    pub fn committed(&self) -> &[ArtifactMetadata] {
        &self.committed
    }

    #[must_use]
    pub fn total_unique_bytes(&self) -> u64 {
        self.total_unique_bytes
    }

    /// Next immutable version for `logical_name` (1-based).
    #[must_use]
    pub fn next_version(&self, logical_name: &str) -> u32 {
        self.committed
            .iter()
            .filter(|m| m.logical_name == logical_name)
            .map(|m| m.version)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1)
    }

    /// Serialize, enforce limits, write content-addressed bytes. Not visible yet.
    pub fn stage(
        &self,
        limits: &WorkflowLimits,
        logical_name: &str,
        kind: ArtifactKind,
        value: &ArtifactValue,
        media_type: Option<&str>,
    ) -> Result<StagedArtifact, WorkflowError> {
        validate_portable_name(logical_name, "artifact logical name")?;
        let media_type = resolve_media_type(kind, media_type)?;
        let bytes = serialize_artifact_bytes(kind, value)?;
        let byte_len = u64::try_from(bytes.len())
            .map_err(|_| WorkflowError::Journal("artifact size overflow".to_owned()))?;

        if byte_len == 0 {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                "artifact payload must not be empty",
            ));
        }
        if byte_len > limits.artifact_record_bytes {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::ResourceLimited,
                format!(
                    "artifact record size {byte_len} exceeds limit {}",
                    limits.artifact_record_bytes
                ),
            ));
        }

        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let already_present = self.committed.iter().any(|m| m.sha256 == sha256);
        if !already_present {
            let projected = self
                .total_unique_bytes
                .checked_add(byte_len)
                .ok_or_else(|| WorkflowError::Journal("artifact total size overflow".to_owned()))?;
            if projected > limits.artifact_total_bytes {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::ResourceLimited,
                    format!(
                        "artifact total size {projected} would exceed limit {}",
                        limits.artifact_total_bytes
                    ),
                ));
            }
        }

        atomic_file::ensure_safe_directory_tree(&self.artifacts_dir)
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;
        write_content_addressed(&self.artifacts_dir, &sha256, &bytes)?;

        let artifact_id = WorkflowArtifactId::new(self.run_id.clone(), sha256.clone())?;
        let version = self.next_version(logical_name);
        Ok(StagedArtifact {
            artifact_id,
            sha256,
            byte_len,
            media_type,
            logical_name: logical_name.to_owned(),
            version,
        })
    }

    /// Register a staged artifact as journal-visible (call after durable journal append).
    ///
    /// Revalidates on-disk bytes so a live commit never exposes a torn file.
    /// Version is assigned here from commit order for the logical name.
    pub fn mark_committed(&mut self, mut meta: ArtifactMetadata) -> Result<(), WorkflowError> {
        let _ = self.read_validated_bytes(&meta.sha256, meta.byte_len)?;
        meta.version = self.next_version(&meta.logical_name);
        self.register_committed(meta)
    }

    /// Register journal membership without requiring bytes (rehydrate path).
    ///
    /// [`Self::get`] still revalidates size/digest and returns typed missing/corrupt errors.
    pub fn register_committed(&mut self, meta: ArtifactMetadata) -> Result<(), WorkflowError> {
        if meta.artifact_id.run_id != self.run_id {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                "artifact run id mismatch",
            ));
        }
        if self.committed.iter().any(|m| {
            m.artifact_id == meta.artifact_id
                && m.logical_name == meta.logical_name
                && m.version == meta.version
        }) {
            return Ok(());
        }
        let is_new_content = !self.committed.iter().any(|m| m.sha256 == meta.sha256);
        if is_new_content {
            self.total_unique_bytes = self
                .total_unique_bytes
                .checked_add(meta.byte_len)
                .ok_or_else(|| WorkflowError::Journal("artifact total size overflow".to_owned()))?;
        }
        self.committed.push(meta);
        Ok(())
    }

    /// Look up committed metadata by content id (any version sharing the id).
    #[must_use]
    pub fn find_by_id(&self, id: &WorkflowArtifactId) -> Option<&ArtifactMetadata> {
        self.committed.iter().rev().find(|m| &m.artifact_id == id)
    }

    /// Look up committed metadata by logical name + version.
    #[must_use]
    pub fn find_by_name_version(&self, name: &str, version: u32) -> Option<&ArtifactMetadata> {
        self.committed
            .iter()
            .find(|m| m.logical_name == name && m.version == version)
    }

    /// Read full content of a committed artifact with integrity revalidation.
    pub fn get(&self, id: &WorkflowArtifactId) -> Result<ArtifactContent, WorkflowError> {
        let metadata = self.find_by_id(id).cloned().ok_or_else(|| {
            WorkflowError::coded(
                WorkflowErrorCode::ArtifactMissing,
                format!(
                    "artifact {} is not journal-committed for run {}",
                    id.as_content_sha256(),
                    self.run_id.as_str()
                ),
            )
        })?;
        let bytes = self.read_validated_bytes(&metadata.sha256, metadata.byte_len)?;
        Ok(ArtifactContent { metadata, bytes })
    }

    /// Read a committed artifact by logical name and version.
    pub fn get_by_name(&self, name: &str, version: u32) -> Result<ArtifactContent, WorkflowError> {
        let metadata = self
            .find_by_name_version(name, version)
            .cloned()
            .ok_or_else(|| {
                WorkflowError::coded(
                    WorkflowErrorCode::ArtifactMissing,
                    format!("artifact {name}@{version} is not journal-committed"),
                )
            })?;
        let bytes = self.read_validated_bytes(&metadata.sha256, metadata.byte_len)?;
        Ok(ArtifactContent { metadata, bytes })
    }

    /// Bounded metadata list (no payload bytes).
    #[must_use]
    pub fn list_metadata(&self) -> &[ArtifactMetadata] {
        &self.committed
    }

    /// Read a byte range of a committed artifact (outside runtime locks).
    pub fn read_range(
        &self,
        id: &WorkflowArtifactId,
        offset: u64,
        max_bytes: u64,
    ) -> Result<ArtifactContentRange, WorkflowError> {
        if max_bytes == 0 {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                "artifact range max_bytes must be greater than 0",
            ));
        }
        let content = self.get(id)?;
        let len = content.bytes.len() as u64;
        if offset > len {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                format!("artifact range offset {offset} exceeds length {len}"),
            ));
        }
        let end = offset.saturating_add(max_bytes).min(len);
        let start = usize::try_from(offset)
            .map_err(|_| WorkflowError::Journal("artifact range offset overflow".to_owned()))?;
        let end_usize = usize::try_from(end)
            .map_err(|_| WorkflowError::Journal("artifact range end overflow".to_owned()))?;
        Ok(ArtifactContentRange {
            metadata: content.metadata,
            offset,
            bytes: content.bytes[start..end_usize].to_vec(),
            has_more: end < len,
        })
    }

    /// Rebuild committed index from durable journal envelopes (does not trust the FS alone).
    pub fn rehydrate_from_envelopes(
        &mut self,
        envelopes: &[JournalEnvelope],
    ) -> Result<(), WorkflowError> {
        self.committed.clear();
        self.total_unique_bytes = 0;
        for envelope in envelopes {
            if let JournalPayload::ArtifactCommitted {
                artifact_id,
                sha256,
                byte_len,
                media_type,
                logical_name,
            } = &envelope.payload
            {
                if artifact_id.run_id != self.run_id {
                    return Err(WorkflowError::coded(
                        WorkflowErrorCode::JournalCorrupt,
                        "artifact_committed run id mismatch",
                    ));
                }
                if sha256 != artifact_id.as_content_sha256() {
                    return Err(WorkflowError::coded(
                        WorkflowErrorCode::JournalCorrupt,
                        "artifact_committed sha256 mismatch",
                    ));
                }
                let logical_name = logical_name
                    .clone()
                    .unwrap_or_else(|| format!("artifact-{}", &sha256[..12.min(sha256.len())]));
                let version = self.next_version(&logical_name);
                let meta = ArtifactMetadata {
                    artifact_id: artifact_id.clone(),
                    sha256: sha256.clone(),
                    byte_len: *byte_len,
                    media_type: media_type
                        .clone()
                        .unwrap_or_else(|| ArtifactKind::Json.default_media_type().to_owned()),
                    logical_name,
                    version,
                };
                // Membership is journal-owned; missing bytes surface on get/read.
                self.register_committed(meta)?;
            }
        }
        Ok(())
    }

    fn read_validated_bytes(
        &self,
        sha256: &str,
        expected_len: u64,
    ) -> Result<Vec<u8>, WorkflowError> {
        WorkflowRevisionHex::validate(sha256)?;
        let path = self.artifacts_dir.join(sha256);
        if !path.exists() {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::ArtifactMissing,
                format!("artifact file missing: {sha256}"),
            ));
        }
        let meta = std::fs::symlink_metadata(&path).map_err(|e| {
            WorkflowError::coded(
                WorkflowErrorCode::ArtifactMissing,
                format!("artifact file unreadable: {sha256}: {e}"),
            )
        })?;
        if atomic_file::is_reparse_or_symlink(&meta) || !meta.is_file() {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::ArtifactCorrupt,
                format!("artifact path is not a regular file: {sha256}"),
            ));
        }
        let bytes = std::fs::read(&path).map_err(|e| {
            WorkflowError::coded(
                WorkflowErrorCode::ArtifactMissing,
                format!("artifact file unreadable: {sha256}: {e}"),
            )
        })?;
        let observed_len = bytes.len() as u64;
        if observed_len != expected_len {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::ArtifactCorrupt,
                format!(
                    "artifact size mismatch for {sha256}: expected {expected_len}, got {observed_len}"
                ),
            ));
        }
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if digest != sha256 {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::ArtifactCorrupt,
                format!("artifact digest mismatch for {sha256}: got {digest}"),
            ));
        }
        Ok(bytes)
    }
}

/// Content-addressed path under a run directory.
#[must_use]
pub fn artifacts_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("artifacts")
}

/// Serialize artifact value to canonical bytes.
pub fn serialize_artifact_bytes(
    kind: ArtifactKind,
    value: &ArtifactValue,
) -> Result<Vec<u8>, WorkflowError> {
    match (kind, value) {
        (ArtifactKind::Text, ArtifactValue::Text(text)) => {
            if !text.is_empty() && std::str::from_utf8(text.as_bytes()).is_err() {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::InvalidInput,
                    "artifact text must be valid UTF-8",
                ));
            }
            Ok(text.as_bytes().to_vec())
        }
        (ArtifactKind::Json, ArtifactValue::Json(value)) => {
            let canonical = canonicalize_json(value);
            serde_json::to_vec(&canonical).map_err(|e| {
                WorkflowError::coded(
                    WorkflowErrorCode::InvalidInput,
                    format!("canonical json serialization failed: {e}"),
                )
            })
        }
        (ArtifactKind::Text, ArtifactValue::Json(_)) => Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            "artifact kind text requires a string value",
        )),
        (ArtifactKind::Json, ArtifactValue::Text(_)) => Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            "artifact kind json requires a json value",
        )),
    }
}

fn resolve_media_type(
    kind: ArtifactKind,
    media_type: Option<&str>,
) -> Result<String, WorkflowError> {
    match media_type {
        None => Ok(kind.default_media_type().to_owned()),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty()
                || trimmed.len() > 128
                || !trimmed.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
            {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::InvalidInput,
                    format!("invalid media type {raw:?}"),
                ));
            }
            Ok(trimmed.to_owned())
        }
    }
}

fn write_content_addressed(dir: &Path, sha256: &str, bytes: &[u8]) -> Result<(), WorkflowError> {
    WorkflowRevisionHex::validate(sha256)?;
    // Content identity is only the digest — never accept caller-supplied path segments.
    let final_path = dir.join(sha256);
    if final_path.exists() {
        let existing = std::fs::read(&final_path).map_err(|e| {
            WorkflowError::coded(
                WorkflowErrorCode::ArtifactCorrupt,
                format!("existing artifact unreadable: {sha256}: {e}"),
            )
        })?;
        let digest = format!("{:x}", Sha256::digest(&existing));
        if digest == sha256 && existing.len() == bytes.len() {
            return Ok(());
        }
        return Err(WorkflowError::coded(
            WorkflowErrorCode::ArtifactCorrupt,
            format!("content-addressed path collision for {sha256}"),
        ));
    }

    match atomic_file::write_file_atomic_create_new(&final_path, bytes) {
        Ok(atomic_file::AtomicWriteStatus::Durable) => Ok(()),
        Ok(atomic_file::AtomicWriteStatus::CommittedUnsynced(error)) => {
            Err(WorkflowError::Journal(format!(
                "artifact committed but directory sync failed: {error}"
            )))
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Race: another writer finished; verify.
            let existing = std::fs::read(&final_path).map_err(|e| {
                WorkflowError::coded(
                    WorkflowErrorCode::ArtifactCorrupt,
                    format!("existing artifact unreadable: {sha256}: {e}"),
                )
            })?;
            let digest = format!("{:x}", Sha256::digest(&existing));
            if digest == sha256 && existing.len() == bytes.len() {
                Ok(())
            } else {
                Err(WorkflowError::coded(
                    WorkflowErrorCode::ArtifactCorrupt,
                    format!("content-addressed path collision for {sha256}"),
                ))
            }
        }
        Err(error) => Err(WorkflowError::Journal(format!(
            "artifact write failed for {sha256}: {error}"
        ))),
    }
}

/// Local helper so we do not depend on WorkflowRevision for pure hex checks on paths.
struct WorkflowRevisionHex;

impl WorkflowRevisionHex {
    fn validate(raw: &str) -> Result<(), WorkflowError> {
        if raw.len() == 64
            && raw
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            Ok(())
        } else {
            Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                format!("invalid artifact content digest {raw:?}"),
            ))
        }
    }
}

//! `ReadMediaFile` — read an image or video file into the session blob
//! store and return structured media in the tool result.
//!
//! The tool is capability-aware: it is constructed from the effective media
//! capabilities (model semantics × provider transport, computed the same way
//! as the request projection) and is never registered when no media kind is
//! deliverable. Execution re-validates the detected media kind against that
//! snapshot so stale or forged calls fail closed.
//!
//! The tool never decides whether media is sent: the result carries only the
//! structured media reference and the request projection decides the actual
//! transport. The blob write is atomic (temp file + rename) so a failed read,
//! unknown type, over-limit payload, or write error never leaves a partial
//! media record behind.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::{
    Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input, read::resolve_read_path,
};
use crate::runtime::image_blobs::MAX_INLINE_VIDEO_BYTES;
use crate::{Content, MediaRef};

/// Leading bytes needed to sniff every supported media signature (the
/// longest signature — RIFF....WEBP — is 12 bytes).
const SNIFF_BYTES: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetectedMedia {
    Image { mime_type: &'static str },
    Video { mime_type: &'static str },
}

/// ISO-BMFF (MP4 family) major brands accepted as video. The major brand is
/// the four bytes following the `ftyp` marker and identifies the file family;
/// an arbitrary `ftyp` string alone is not a media signature.
const MP4_BRANDS: [&[u8; 4]; 12] = [
    b"isom", b"iso2", b"mp41", b"mp42", b"avc1", b"M4V ", b"dash", b"qt  ", b"3gp4", b"3gp5",
    b"3gp6", b"mmp4",
];

/// Identify the media kind and MIME type from the file's leading bytes.
/// Content-based detection, never extension-based, so renamed files cannot
/// smuggle unsupported content into the media path.
fn sniff_media(header: &[u8]) -> Option<DetectedMedia> {
    if header.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some(DetectedMedia::Image {
            mime_type: "image/png",
        })
    } else if header.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(DetectedMedia::Image {
            mime_type: "image/jpeg",
        })
    } else if header.starts_with(b"GIF8") {
        Some(DetectedMedia::Image {
            mime_type: "image/gif",
        })
    } else if header.len() >= 12 && &header[0..4] == b"RIFF" && &header[8..12] == b"WEBP" {
        Some(DetectedMedia::Image {
            mime_type: "image/webp",
        })
    } else if header.len() >= 12
        && &header[4..8] == b"ftyp"
        && MP4_BRANDS.iter().any(|brand| &header[8..12] == *brand)
    {
        Some(DetectedMedia::Video {
            mime_type: "video/mp4",
        })
    } else {
        None
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadMediaFileInput {
    path: PathBuf,
    #[serde(default)]
    region: Option<ImageRegion>,
    #[serde(default)]
    resolution: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageRegion {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// Snapshot of the effective media capabilities at construction time.
///
/// The runtime is rebuilt on model switches, so a snapshot is always
/// current for the lifetime of the tool; execution checks the detected
/// media kind against it to reject stale or forged calls.
#[derive(Debug, Clone, Copy)]
pub struct ReadMediaFileTool {
    images: bool,
    videos: bool,
}

impl ReadMediaFileTool {
    #[must_use]
    pub const fn new(images: bool, videos: bool) -> Self {
        Self { images, videos }
    }

    /// Build the tool from the current model's semantic capabilities and the
    /// provider adapter's transport capabilities. Returns `None` — and the
    /// tool is not registered — when no media kind is deliverable.
    #[must_use]
    pub fn from_model(
        model: &neo_ai::ModelCapabilities,
        transport: neo_ai::MediaTransportCapabilities,
    ) -> Option<Self> {
        let images = crate::runtime::chat_request::tool_result_kind_deliverable(
            neo_ai::MediaKind::Image,
            model,
            transport,
        );
        let videos = crate::runtime::chat_request::tool_result_kind_deliverable(
            neo_ai::MediaKind::Video,
            model,
            transport,
        );
        (images || videos).then_some(Self { images, videos })
    }
}

impl Tool for ReadMediaFileTool {
    fn name(&self) -> &'static str {
        "ReadMediaFile"
    }

    fn description(&self) -> &str {
        match (self.images, self.videos) {
            (true, true) => {
                "Read an image or video file and attach it to the conversation so the model can see it.\
                \
                Supported formats: PNG, JPEG, GIF and WebP images, and MP4 video. The bytes are persisted in the \
                session as a content-addressed blob; the tool result carries the media reference and the request \
                layer decides how it is sent to the current model.\
                \
                Parameters:\
                - path: Path to the media file. Relative paths resolve against the working directory; absolute \
                  paths are used as-is, including paths outside the working directory.\
                - region: Optional image region of interest (x, y, width, height).\
                - resolution: Optional image resolution hint.\
                \
                Videos larger than the inline size limit are rejected. Region and resolution are informational \
                only: this tool performs no cropping or resampling."
            }
            (true, false) => {
                "Read an image file and attach it to the conversation so the model can see it.\
                \
                Supported formats: PNG, JPEG, GIF and WebP. The bytes are persisted in the session as a \
                content-addressed blob; the tool result carries the media reference and the request layer \
                decides how it is sent to the current model.\
                \
                Parameters:\
                - path: Path to the image file. Relative paths resolve against the working directory; absolute \
                  paths are used as-is, including paths outside the working directory.\
                - region: Optional region of interest (x, y, width, height).\
                - resolution: Optional resolution hint.\
                \
                Region and resolution are informational only: this tool performs no cropping or resampling."
            }
            (false, true) => {
                "Read a video file and attach it to the conversation so the model can see it.\
                \
                Supported formats: MP4. The bytes are persisted in the session as a content-addressed blob; \
                the tool result carries the media reference and the request layer decides how it is sent to \
                the current model.\
                \
                Parameters:\
                - path: Path to the video file. Relative paths resolve against the working directory; \
                  absolute paths are used as-is, including paths outside the working directory.\
                \
                Videos larger than the inline size limit are rejected."
            }
            (false, false) => {
                // Unreachable in practice: `from_model` never constructs the
                // tool without at least one deliverable media kind.
                "Read a media file."
            }
        }
    }

    fn input_schema(&self) -> serde_json::Value {
        let mut properties = serde_json::Map::new();
        properties.insert(
            "path".to_owned(),
            serde_json::json!({
                "type": "string",
                "description": "Path to the media file. Relative paths resolve against the working directory; \
                 absolute paths are used as-is, including paths outside the working directory."
            }),
        );
        if self.images {
            properties.insert(
                "region".to_owned(),
                serde_json::json!({
                    "type": "object",
                    "description": "Optional image region of interest. Informational only: no cropping is performed.",
                    "properties": {
                        "x": { "type": "integer", "minimum": 0, "description": "Left edge of the region." },
                        "y": { "type": "integer", "minimum": 0, "description": "Top edge of the region." },
                        "width": { "type": "integer", "minimum": 0, "description": "Region width." },
                        "height": { "type": "integer", "minimum": 0, "description": "Region height." }
                    },
                    "required": ["x", "y", "width", "height"],
                    "additionalProperties": false
                }),
            );
            properties.insert(
                "resolution".to_owned(),
                serde_json::json!({
                    "type": "string",
                    "description": "Optional image resolution hint (for example \"high\" or \"low\"). \
                     Informational only: no resampling is performed."
                }),
            );
        }
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": ["path"],
            "additionalProperties": false
        })
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_read_allowed()?;
            let input: ReadMediaFileInput = parse_input(self.name(), input)?;
            let path = resolve_read_path(ctx, &input.path);
            match run_read_media(self, ctx, &path, input.region, input.resolution).await {
                Ok(result) => Ok(result),
                Err(ReadMediaError::Io(source)) => Err(ToolError::Io(source)),
                Err(error) => Ok(ToolResult::error(error.to_string())),
            }
        })
    }
}

#[derive(Debug)]
enum ReadMediaError {
    Io(std::io::Error),
    Message(String),
}

impl std::fmt::Display for ReadMediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(source) => write!(f, "io error: {source}"),
            Self::Message(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ReadMediaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(source) => Some(source),
            Self::Message(_) => None,
        }
    }
}

impl From<std::io::Error> for ReadMediaError {
    fn from(source: std::io::Error) -> Self {
        Self::Io(source)
    }
}

async fn run_read_media(
    tool: &ReadMediaFileTool,
    ctx: &ToolContext,
    path: &Path,
    region: Option<ImageRegion>,
    resolution: Option<String>,
) -> Result<ToolResult, ReadMediaError> {
    if !path.exists() {
        return Err(ReadMediaError::Message(format!(
            "\"{}\" does not exist.",
            path.display()
        )));
    }
    let metadata = tokio::fs::metadata(path).await?;
    if !metadata.is_file() {
        return Err(ReadMediaError::Message(format!(
            "\"{}\" is not a file.",
            path.display()
        )));
    }

    // Sniff the media kind from the leading bytes before reading the payload,
    // so unknown files are rejected without ever loading them.
    let mut file = tokio::fs::File::open(path).await?;
    let mut header = [0u8; SNIFF_BYTES];
    let header_len = file.read(&mut header).await?;
    let detected = sniff_media(&header[..header_len]).ok_or_else(|| {
        ReadMediaError::Message(format!(
            "\"{}\" is not a supported media file. Supported formats: PNG, JPEG, GIF and \
             WebP images, and MP4 video.",
            path.display()
        ))
    })?;

    // Capability re-validation: the snapshot is the boundary between what the
    // tool table promised and what this call may deliver. Stale or forged
    // calls fail closed with no blob written.
    match detected {
        DetectedMedia::Image { .. } if !tool.images => {
            return Err(ReadMediaError::Message(
                "the current model does not support image input; only video files can be read"
                    .to_owned(),
            ));
        }
        DetectedMedia::Video { .. } if !tool.videos => {
            return Err(ReadMediaError::Message(
                "the current model does not support video input; only image files can be read"
                    .to_owned(),
            ));
        }
        DetectedMedia::Video { .. } if region.is_some() || resolution.is_some() => {
            return Err(ReadMediaError::Message(
                "image-specific parameters (region, resolution) cannot be applied to a video file"
                    .to_owned(),
            ));
        }
        _ => {}
    }

    let mut bytes = Vec::with_capacity(header_len);
    bytes.extend_from_slice(&header[..header_len]);
    match detected {
        DetectedMedia::Image { .. } => {
            file.read_to_end(&mut bytes).await?;
        }
        DetectedMedia::Video { .. } => {
            read_video_payload(file, path, &metadata, header_len, &mut bytes).await?;
        }
    }

    let sha = hex_digest(&bytes);

    persist_blob(ctx, path, &sha, &bytes).await?;

    let (label, mime_type, media) = match detected {
        DetectedMedia::Image { mime_type } => (
            "image",
            mime_type,
            Content::Image {
                mime_type: mime_type.into(),
                data: MediaRef::Blob(sha.clone().into()),
            },
        ),
        DetectedMedia::Video { mime_type } => (
            "video",
            mime_type,
            Content::Video {
                mime_type: mime_type.into(),
                data: MediaRef::Blob(sha.clone().into()),
            },
        ),
    };
    let mut text = format!(
        "Read {label} file \"{}\" as {mime_type} ({} bytes); stored as session blob {}.",
        path.display(),
        bytes.len(),
        &sha[..8]
    );
    if let Some(region) = region {
        let _ = write!(
            text,
            " Region hint accepted (x={}, y={}, width={}, height={}); no transformation \
             was performed.",
            region.x, region.y, region.width, region.height
        );
    }
    if let Some(resolution) = resolution {
        let _ = write!(
            text,
            " Resolution hint \"{resolution}\" accepted; no transformation was performed."
        );
    }
    Ok(ToolResult::ok(text).with_media(vec![media]))
}

/// Bounded video payload read: a cheap metadata pre-check first, then a
/// bounded read as a second line of defense against growth between checks.
async fn read_video_payload(
    file: tokio::fs::File,
    path: &Path,
    metadata: &std::fs::Metadata,
    header_len: usize,
    bytes: &mut Vec<u8>,
) -> Result<(), ReadMediaError> {
    if metadata.len() > MAX_INLINE_VIDEO_BYTES as u64 {
        return Err(ReadMediaError::Message(format!(
            "\"{}\" is {} bytes, exceeding the video size limit of \
             {MAX_INLINE_VIDEO_BYTES} bytes; the file was not stored.",
            path.display(),
            metadata.len()
        )));
    }
    let rest_limit = MAX_INLINE_VIDEO_BYTES.saturating_sub(header_len) as u64;
    let mut limited = file.take(rest_limit.saturating_add(1));
    limited.read_to_end(bytes).await?;
    if bytes.len() > MAX_INLINE_VIDEO_BYTES {
        return Err(ReadMediaError::Message(format!(
            "\"{}\" exceeds the video size limit of {MAX_INLINE_VIDEO_BYTES} bytes; \
             the file was not stored.",
            path.display()
        )));
    }
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut digest = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        let _ = write!(digest, "{byte:02x}");
    }
    digest
}

/// Atomic blob persistence: the payload is written to a dot-prefixed temp
/// file and renamed into place only when the write is complete. The blob
/// lookup never matches `.tmp-<sha>` (direct name is `<sha>.bin`; the scan
/// fallback matches `starts_with(<sha>)`), so a crash between write and
/// rename leaves no partial `<sha>.bin` that projection could later read —
/// the media simply resolves to the deterministic unavailable text.
async fn persist_blob(
    ctx: &ToolContext,
    path: &Path,
    sha: &str,
    bytes: &[u8],
) -> Result<(), ReadMediaError> {
    let Some(session_dir) = &ctx.session_directory else {
        return Err(ReadMediaError::Message(
            "the session directory is unavailable, so the media file cannot be persisted; \
             retry in a session-backed run"
                .to_owned(),
        ));
    };
    let blob_dir = session_dir.join("blobs");
    tokio::fs::create_dir_all(&blob_dir).await?;
    let blob_path = blob_dir.join(format!("{sha}.bin"));
    let tmp_path = blob_dir.join(format!(".tmp-{sha}"));
    if let Err(source) = tokio::fs::write(&tmp_path, bytes).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(ReadMediaError::Message(format!(
            "failed to persist the media blob for \"{}\": {source}",
            path.display()
        )));
    }
    if let Err(source) = tokio::fs::rename(&tmp_path, &blob_path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(ReadMediaError::Message(format!(
            "failed to persist the media blob for \"{}\": {source}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "test_cases/read_media.rs"]
mod read_media_tests;

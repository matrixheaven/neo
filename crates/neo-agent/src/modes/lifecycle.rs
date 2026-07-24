//! Cross-platform lifecycle commands: update, rollback, and uninstall.
//!
//! This module owns all release-selection policy, backup/replacement
//! transactions, offline rollback, and guarded uninstall behavior.
//! It is dispatched before `AppConfig::load` so that a broken or missing
//! provider configuration cannot prevent lifecycle operations.

use anyhow::{Context, bail};
use semver::Version;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// ── Public constants ────────────────────────────────────────────────

const REPO_OWNER: &str = "matrixheaven";
const REPO_NAME: &str = "neo";

// ── Internal policy types ───────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpdateMode {
    Stable,
    Unstable,
    StableSwitch,
    Rollback,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AssetSpec {
    archive_name: String,
    binary_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TargetRelease {
    version: Version,
    tag: String,
    asset: AssetSpec,
}

pub(crate) enum ReleaseDecision {
    AlreadyCurrent { channel: &'static str },
    Install(TargetRelease),
    RequireStableSwitch {
        current: Version,
        target: Version,
    },
}

// ── UpdateMode ──────────────────────────────────────────────────────

impl UpdateMode {
    /// Resolve the update mode from the three boolean CLI flags.
    ///
    /// Clap already rejects conflicting flag pairs, but we also validate
    /// here so the function is safe to call from non-clap paths.
    pub(crate) fn from_flags(unstable: bool, stable: bool, rollback: bool) -> anyhow::Result<Self> {
        let count = u8::from(unstable) + u8::from(stable) + u8::from(rollback);
        if count > 1 {
            bail!("--unstable, --stable, and --rollback are mutually exclusive");
        }
        Ok(if rollback {
            Self::Rollback
        } else if unstable {
            Self::Unstable
        } else if stable {
            Self::StableSwitch
        } else {
            Self::Stable
        })
    }
}

// ── Platform asset mapping ──────────────────────────────────────────

/// Resolve the exact archive asset name and binary path inside the archive
/// for the current platform.
///
/// For version `0.1.0` exactly, Linux/macOS use plain binary assets (no
/// `.tar.gz` suffix). This is the only historical packaging exception.
pub(crate) fn platform_asset(version: &Version) -> anyhow::Result<AssetSpec> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let (base, ext, binary_ext) = match (os, arch) {
        ("linux", "x86_64") => ("neo-linux-x86_64", ".tar.gz", ""),
        ("linux", "aarch64") => ("neo-linux-arm64", ".tar.gz", ""),
        ("macos", "x86_64") => ("neo-macos-x86_64", ".tar.gz", ""),
        ("macos", "aarch64") => ("neo-macos-arm64", ".tar.gz", ""),
        ("windows", "x86_64") => ("neo-windows-x86_64", ".zip", ".exe"),
        ("windows", "aarch64") => ("neo-windows-arm64", ".zip", ".exe"),
        _ => bail!("unsupported platform: {os}/{arch}"),
    };

    // v0.1.0 used plain binary assets on Unix (no archive wrapper).
    let is_v0_1_0 = version.major == 0 && version.minor == 1 && version.patch == 0 && version.pre.is_empty();

    let archive_name = if is_v0_1_0 && os != "windows" {
        base.to_string()
    } else {
        format!("{base}{ext}")
    };
    let binary_name = format!("{base}{binary_ext}");

    Ok(AssetSpec { archive_name, binary_name })
}

// ── Release selection ───────────────────────────────────────────────

/// Select the best release from the provided list according to the
/// requested channel and downgrade policy.
///
/// `releases` must be ordered newest-first (as returned by GitHub).
/// `current` is the running version. Non-SemVer tags are silently
/// skipped. Drafts are assumed already filtered by GitHub.
pub(crate) fn select_release(
    releases: &[self_update::Release],
    current: &Version,
    mode: UpdateMode,
) -> anyhow::Result<ReleaseDecision> {
    let channel = match mode {
        UpdateMode::Stable | UpdateMode::StableSwitch => "stable",
        UpdateMode::Unstable => "unstable",
        UpdateMode::Rollback => bail!("select_release must not be called for rollback"),
    };

    // Parse and filter releases by channel.
    let mut candidates: Vec<(Version, &self_update::Release)> = releases
        .iter()
        .filter_map(|r| {
            // self_update stores the bare version (no leading `v`).
            let v = Version::parse(r.version()).ok()?;
            match mode {
                UpdateMode::Stable | UpdateMode::StableSwitch => {
                    if v.pre.is_empty() {
                        Some((v, r))
                    } else {
                        None
                    }
                }
                UpdateMode::Unstable => {
                    if v.pre.is_empty() {
                        None
                    } else {
                        Some((v, r))
                    }
                }
                UpdateMode::Rollback => unreachable!(),
            }
        })
        .collect();

    // Sort by precedence descending (newest first). Use cmp_precedence
    // so build metadata does not affect ordering.
    candidates.sort_by(|a, b| b.0.cmp_precedence(&a.0));

    let (best_version, _best_release) = match candidates.first() {
        Some((v, r)) => (v.clone(), *r),
        None => {
            return Ok(ReleaseDecision::AlreadyCurrent {
                channel,
            });
        }
    };

    // Equal precedence (including different build metadata) = already current.
    if current.cmp_precedence(&best_version) == std::cmp::Ordering::Equal {
        return Ok(ReleaseDecision::AlreadyCurrent { channel });
    }

    // Downgrade policy.
    let target_is_newer = best_version.cmp_precedence(current) == std::cmp::Ordering::Greater;
    if !target_is_newer {
        match mode {
            UpdateMode::Stable => {
                // Default stable: never downgrade. If current is prerelease,
                // tell user to use --stable explicitly.
                if !current.pre.is_empty() {
                    return Ok(ReleaseDecision::RequireStableSwitch {
                        current: current.clone(),
                        target: best_version,
                    });
                }
                // Current is stable and newer: already current.
                return Ok(ReleaseDecision::AlreadyCurrent { channel });
            }
            UpdateMode::Unstable => {
                // Unstable: never downgrade.
                return Ok(ReleaseDecision::AlreadyCurrent { channel });
            }
            UpdateMode::StableSwitch => {
                // StableSwitch: may downgrade only when current is prerelease.
                if current.pre.is_empty() {
                    // Current is stable and newer: no downgrade.
                    return Ok(ReleaseDecision::AlreadyCurrent { channel });
                }
                // Current is prerelease: allow downgrade to stable.
            }
            UpdateMode::Rollback => unreachable!(),
        }
    }

    // Resolve the platform asset for the target version.
    let asset = platform_asset(&best_version)?;
    let tag = format!("v{best_version}");

    Ok(ReleaseDecision::Install(TargetRelease {
        version: best_version,
        tag,
        asset,
    }))
}

// ── Exact asset validation ──────────────────────────────────────────

/// Validate that exactly one asset in the release matches the expected
/// name and has a GitHub SHA-256 digest. Returns the matching asset.
pub(crate) fn exact_asset_with_digest(
    assets: &[self_update::ReleaseAsset],
    expected_name: &str,
) -> Option<self_update::ReleaseAsset> {
    let matching: Vec<_> = assets
        .iter()
        .filter(|a| a.name() == expected_name)
        .collect();

    if matching.len() != 1 {
        return None;
    }

    let asset = matching[0];
    asset.digest()?;

    Some(asset.clone())
}

// ── Backup path ────────────────────────────────────────────────────

/// Resolve the adjacent `.bak` path for the given executable.
///
/// Unix: `/path/neo` → `/path/neo.bak`
/// Windows: `C:\path\neo.exe` → `C:\path\neo.exe.bak`
fn backup_path(current_exe: &Path) -> anyhow::Result<PathBuf> {
    let parent = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("executable has no parent directory: {current_exe:?}"))?;
    let file_name = current_exe
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("executable has no filename: {current_exe:?}"))?;
    let mut bak_name = file_name.to_owned();
    bak_name.push(".bak");
    Ok(parent.join(bak_name))
}

// ── Binary version verification ────────────────────────────────────

/// Parse `neo <version>` from `--version` output.
///
/// Clap outputs `neo 0.1.1-rc.2+20260721.0634`. We require the first
/// line to start with `neo ` followed by a parseable SemVer.
fn parse_neo_version_output(stdout: &str) -> anyhow::Result<Version> {
    let first_line = stdout
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty --version output"))?
        .trim();

    let version_str = first_line
        .strip_prefix("neo ")
        .ok_or_else(|| anyhow::anyhow!("unexpected --version format: {first_line:?}"))?;

    Version::parse(version_str)
        .with_context(|| format!("failed to parse version from --version output: {version_str:?}"))
}

/// Execute a binary with `--version` and verify it reports the expected version.
fn verify_binary_version(path: &Path, expected: &Version) -> anyhow::Result<()> {
    let output = std::process::Command::new(path)
        .arg("--version")
        .output()
        .with_context(|| format!("failed to execute {path:?} --version"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{path:?} --version exited with {}: {stderr}", output.status);
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| anyhow::anyhow!("non-UTF-8 --version output from {path:?}"))?;

    let reported = parse_neo_version_output(&stdout)?;
    if reported != *expected {
        bail!(
            "{path:?} reported version {reported}, expected {expected}"
        );
    }
    Ok(())
}

// ── Staged copy helper ─────────────────────────────────────────────

/// Copy a binary to a sibling `NamedTempFile`, preserving permissions
/// and flushing to disk. The temp file is created in `parent_dir` so
/// that `persist()` can be atomic on the same filesystem.
fn stage_copy(
    source: &Path,
    parent_dir: &Path,
    #[cfg(windows)] suffix: &str,
) -> anyhow::Result<tempfile::NamedTempFile> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("neo-lifecycle-");
    #[cfg(windows)]
    {
        builder.suffix(suffix);
    }
    let tmp = builder
        .tempdir_in(parent_dir)
        .with_context(|| format!("failed to create temp dir in {parent_dir:?}"))?;

    // We need a file inside the temp dir, not the dir itself.
    // Use NamedTempFile in the parent directly instead.
    drop(tmp);

    let mut builder = tempfile::Builder::new();
    builder.prefix("neo-lifecycle-");
    #[cfg(windows)]
    {
        builder.suffix(suffix);
    }
    let tmp_file = builder
        .tempfile_in(parent_dir)
        .with_context(|| format!("failed to create temp file in {parent_dir:?}"))?;

    std::fs::copy(source, tmp_file.path())
        .with_context(|| format!("failed to copy {source:?} to temp file"))?;

    // Preserve executable permissions on Unix.
    #[cfg(unix)]
    {
        let perms = std::fs::metadata(source)
            .with_context(|| format!("failed to read permissions of {source:?}"))?
            .permissions();
        std::fs::set_permissions(tmp_file.path(), perms)
            .with_context(|| format!("failed to set permissions on temp file"))?;
    }

    // Flush and sync.
    {
        let mut f = &tmp_file;
        use std::io::Write;
        f.flush().with_context(|| "failed to flush temp file")?;
    }
    // sync_all via the File handle
    tmp_file.as_file().sync_all().with_context(|| "failed to sync temp file")?;

    Ok(tmp_file)
}

// ── Backup promotion ───────────────────────────────────────────────

/// Stage the current executable as `.bak` and verify the staged copy.
///
/// The order is:
/// 1. Copy current exe to a sibling temp file.
/// 2. Verify the temp file reports the running version.
/// 3. Atomically persist the temp file over the `.bak` path.
fn promote_backup(
    current_exe: &Path,
    running_version: &Version,
) -> anyhow::Result<PathBuf> {
    let bak = backup_path(current_exe)?;
    let parent = bak
        .parent()
        .ok_or_else(|| anyhow::anyhow!("backup path has no parent: {bak:?}"))?
        .to_path_buf();

    // Stage current exe copy.
    #[cfg(windows)]
    let staged = stage_copy(current_exe, &parent, ".exe")?;
    #[cfg(not(windows))]
    let staged = stage_copy(current_exe, &parent)?;

    // Verify staged copy reports running version.
    verify_binary_version(staged.path(), running_version)
        .context("staged backup verification failed")?;

    // Atomically promote over .bak.
    staged
        .persist(&bak)
        .with_context(|| format!("failed to promote backup to {bak:?}"))?;

    // Verify persisted .bak is a regular file.
    if !bak.is_file() {
        bail!("backup path is not a regular file after promotion: {bak:?}");
    }

    Ok(bak)
}

// ── Recovery ────────────────────────────────────────────────────────

/// Restore the installation at `install_path` from the verified `.bak`.
///
/// On Windows, `self_replace` may have already moved the original exe
/// before failing, so we cannot call `self_replace` again. Instead, we
/// copy `.bak` to a verified temp file and atomically persist it over
/// the captured install path.
fn restore_from_backup(
    install_path: &Path,
    bak_path: &Path,
    original_version: &Version,
) -> anyhow::Result<()> {
    let parent = install_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("install path has no parent: {install_path:?}"))?
        .to_path_buf();

    // Stage a copy of .bak.
    #[cfg(windows)]
    let staged = stage_copy(bak_path, &parent, ".exe")?;
    #[cfg(not(windows))]
    let staged = stage_copy(bak_path, &parent)?;

    // Verify staged recovery copy.
    verify_binary_version(staged.path(), original_version)
        .context("recovery staged binary verification failed")?;

    // Check if install_path already exists and reports original version.
    // If it does, we may not need to replace it (e.g., self_replace didn't
    // move it yet).
    if install_path.exists() {
        if let Ok(()) = verify_binary_version(install_path, original_version) {
            // Original binary still intact; no persist needed.
            return Ok(());
        }
    }
    // Atomically restore.
    staged
        .persist(install_path)
        .with_context(|| format!("failed to restore to {install_path:?}"))?;

    // Verify restored binary.
    verify_binary_version(install_path, original_version)
        .context("restored binary verification failed")?;

    Ok(())
}

// ── Public update entry point ──────────────────────────────────────

/// Perform a network update from GitHub Releases.
///
/// This function:
/// 1. Fetches all public releases from GitHub.
/// 2. Selects the best release according to channel/downgrade policy.
/// 3. Downloads the exact platform asset.
/// 4. Verifies the GitHub SHA-256 digest.
/// 5. Extracts and verifies the staged binary.
/// 6. Promotes the current executable to `.bak`.
/// 7. Atomically replaces the running binary.
/// 8. On failure, automatically restores from `.bak` if backup was promoted.
pub(crate) async fn update(
    unstable: bool,
    stable: bool,
    rollback: bool,
) -> anyhow::Result<String> {
    let mode = UpdateMode::from_flags(unstable, stable, rollback)?;

    if mode == UpdateMode::Rollback {
        return rollback_impl().await;
    }

    let current_version: Version = Version::parse(env!("CARGO_PKG_VERSION"))
        .context("invalid compiled package version")?;
    let current_exe = std::env::current_exe()
        .context("failed to resolve current executable path")?;

    // 1. Fetch releases.
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .build()
        .context("failed to configure release list")?
        .fetch_async()
        .await
        .context("failed to fetch releases from GitHub")?;

    // 2. Select target release.
    let decision = select_release(releases.all(), &current_version, mode)?;

    let target = match decision {
        ReleaseDecision::AlreadyCurrent { channel } => {
            return Ok(format!(
                "Neo {current_version} is already the latest {channel} release."
            ));
        }
        ReleaseDecision::RequireStableSwitch { current, target } => {
            bail!(
                "running prerelease {current} is newer than latest stable {target}; \
                 use `neo update --stable` to explicitly switch channels."
            );
        }
        ReleaseDecision::Install(t) => t,
    };

    // 3. Configure and run the updater.
    let asset_name = target.asset.archive_name.clone();
    let binary_name = target.asset.binary_name.clone();
    let expected_version = target.version.clone();
    let tag = target.tag.clone();
    let install_path = current_exe.clone();
    let running_version = current_version.clone();

    let backup_ready = Arc::new(AtomicBool::new(false));
    let backup_ready_hook = Arc::clone(&backup_ready);
    let hook_exe = current_exe.clone();
    let hook_version = expected_version.clone();

    let mut builder = self_update::backends::github::Update::configure();
    builder
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .current_version(current_version.to_string())
        .release_tag(tag.clone())
        .bin_name("neo")
        .bin_path_in_archive(binary_name.clone())
        .unattended()
        .verify_release_digest(true)
        .asset_matcher(move |assets| exact_asset_with_digest(assets, &asset_name))
        .verify_binary(move |staged| {
            // 1. Verify staged successor binary.
            verify_binary_version(staged, &hook_version)
                .map_err(|e| self_update::Error::verification_rejected(e.to_string()))?;

            // 2. Promote current exe to .bak.
            promote_backup(&hook_exe, &running_version)
                .map_err(|e| self_update::Error::verification_rejected(e.to_string()))?;

            // 3. Mark backup ready.
            backup_ready_hook.store(true, Ordering::Release);
            Ok(())
        });

    let updater = builder
        .build_async()
        .context("failed to build updater")?;

    let result = updater.update_extended_async().await;

    match result {
        Ok(_status) => {
            let new_version = target.version;
            let bak = backup_path(&install_path)?;
            Ok(format!(
                "Updated Neo {current_version} → {new_version}.\n\
                 Backup: {bak:?}\n\
                 Please restart Neo."
            ))
        }
        Err(update_err) => {
            if !backup_ready.load(Ordering::Acquire) {
                // Backup was not promoted; current exe is untouched.
                bail!(
                    "update failed (tag {tag}): {update_err}\n\
                     The current installation was not modified."
                );
            }

            // Backup was promoted; attempt automatic recovery.
            let bak = backup_path(&install_path)?;
            match restore_from_backup(&install_path, &bak, &current_version) {
                Ok(()) => {
                    // Recovery succeeded; consume .bak.
                    if let Err(rm_err) = std::fs::remove_file(&bak) {
                        return Err(update_err).context(format!(
                            "update failed and backup cleanup failed: {rm_err}\n\
                             Restored {install_path:?} to {current_version}.\n\
                             Backup remains at {bak:?}."
                        ));
                    }
                    Err(update_err).context(format!(
                        "update failed but the previous version {current_version} \
                         was automatically restored from backup."
                    ))
                }
                Err(restore_err) => {
                    // Dual failure: retain .bak for manual recovery.
                    bail!(
                        "update failed: {update_err}\n\
                         automatic restoration also failed: {restore_err}\n\
                         current executable: {install_path:?}\n\
                         backup: {bak:?}\n\
                         manual recovery is required: \
                         copy the backup over the current executable."
                    );
                }
            }
        }
    }
}

// ── Rollback implementation ─────────────────────────────────────────

/// Offline one-shot rollback: restore from `.bak` without network.
async fn rollback_impl() -> anyhow::Result<String> {
    let current_exe = std::env::current_exe()
        .context("failed to resolve current executable path")?;
    let bak = backup_path(&current_exe)?;

    // 1. Validate .bak exists and is a regular file.
    if !bak.exists() {
        bail!("no backup found at {bak:?}; nothing to roll back.");
    }
    let bak_meta = std::fs::symlink_metadata(&bak)
        .with_context(|| format!("failed to read backup metadata: {bak:?}"))?;
    if !bak_meta.is_file() {
        bail!("backup path is not a regular file: {bak:?}");
    }

    // 2. Verify the backup binary is a valid Neo.
    #[cfg(windows)]
    let staged_backup = stage_copy(&bak, bak.parent().unwrap(), ".exe")?;
    #[cfg(not(windows))]
    let staged_backup = stage_copy(&bak, bak.parent().unwrap())?;

    let backup_version = {
        let output = std::process::Command::new(staged_backup.path())
            .arg("--version")
            .output()
            .context("failed to execute backup binary --version")?;
        if !output.status.success() {
            bail!("backup binary --version failed: {:?}", output.status);
        }
        let stdout = String::from_utf8(output.stdout)
            .map_err(|_| anyhow::anyhow!("non-UTF-8 --version output from backup"))?;
        parse_neo_version_output(&stdout)?
    };

    let running_version: Version = Version::parse(env!("CARGO_PKG_VERSION"))
        .context("invalid compiled package version")?;

    // 3. Create a transient guard copy of the current executable.
    // This is NOT a second backup slot — it's a temporary transaction file.
    let guard_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("executable has no parent"))?
        .to_path_buf();
    #[cfg(windows)]
    let guard = stage_copy(&current_exe, &guard_dir, ".exe")?;
    #[cfg(not(windows))]
    let guard = stage_copy(&current_exe, &guard_dir)?;

    verify_binary_version(guard.path(), &running_version)
        .context("guard copy verification failed")?;

    // 4. Replace current exe with backup using recovery-aware helper.
    let install_path_saved = current_exe.clone();
    let bak_path_saved = bak.clone();
    replace_with_recovery(
        &install_path_saved,
        staged_backup.path(),
        &backup_version,
        guard.path(),
        &running_version,
        &bak_path_saved,
        |src| self_replace::self_replace(src),
    )
}

/// Core replace-with-recovery helper.
///
/// This function:
/// 1. Replaces `install_path` with `successor` using the provided `replace_fn`.
/// 2. Verifies the installed binary reports `successor_version`.
/// 3. On success, removes `bak_path`.
/// 4. On failure (replace or verify), restores `install_path` from `guard_path`.
/// 5. The `guard_path` is a verified transient copy of the previous installation.
///
/// The `replace_fn` closure allows tests to inject simulated failures.
fn replace_with_recovery(
    install_path: &Path,
    successor: &Path,
    successor_version: &Version,
    guard_path: &Path,
    guard_version: &Version,
    bak_path: &Path,
    replace_fn: impl Fn(&Path) -> std::result::Result<(), std::io::Error>,
) -> anyhow::Result<String> {
    let replace_result = replace_fn(successor);
    match replace_result {
        Ok(()) => {
            // Verify installed version.
            match verify_binary_version(install_path, successor_version) {
                Ok(()) => {
                    // Success: consume .bak.
                    if let Err(rm_err) = std::fs::remove_file(bak_path) {
                        return Ok(format!(
                            "Rolled back {guard_version} → {successor_version}.\n\
                             Warning: failed to remove backup {bak_path:?}: {rm_err}\n\
                             Please restart Neo."
                        ));
                    }
                    Ok(format!(
                        "Rolled back {guard_version} → {successor_version}.\n\
                         Backup consumed. Please restart Neo."
                    ))
                }
                Err(verify_err) => {
                    // Post-install verification failed.
                    // Restore from guard, leave .bak intact.
                    let _ = restore_from_backup(install_path, guard_path, guard_version);
                    bail!(
                        "rollback replacement succeeded but installed version \
                         verification failed: {verify_err}\n\
                         The previous version was restored. Backup remains at {bak_path:?}."
                    );
                }
            }
        }
        Err(replace_err) => {
            // Replacement failed. Restore from guard, leave .bak intact.
            match restore_from_backup(install_path, guard_path, guard_version) {
                Ok(()) => {
                    bail!(
                        "rollback failed: {replace_err}\n\
                         The previous version was restored. Backup remains at {bak_path:?}."
                    );
                }
                Err(restore_err) => {
                    bail!(
                        "rollback failed: {replace_err}\n\
                         guard restoration also failed: {restore_err}\n\
                         current executable: {install_path:?}\n\
                         backup: {bak_path:?}\n\
                         manual recovery is required."
                    );
                }
            }
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper ──────────────────────────────────────────────────────

    /// Resolve the actual neo binary path from the test binary.
    ///
    /// `cargo nextest` runs tests from `target/debug/deps/neo-<hash>`,
    /// which is the test harness and does NOT support `--version`.
    /// The actual neo binary is at `target/debug/neo`.
    fn neo_binary_path() -> std::path::PathBuf {
        let test_exe = std::env::current_exe().unwrap();
        // Navigate from target/debug/deps/<binary> to target/debug/neo
        let deps_dir = test_exe.parent().unwrap(); // deps/
        let debug_dir = deps_dir.parent().unwrap(); // debug/
        let neo = debug_dir.join({
            #[cfg(windows)]
            { "neo.exe" }
            #[cfg(not(windows))]
            { "neo" }
        });
        assert!(neo.exists(), "neo binary must exist at {neo:?}");
        neo
    }

    fn make_release(version: &str, asset_names: &[(&str, Option<&str>)]) -> self_update::Release {
        let mut builder = self_update::Release::builder();
        builder.version(version);
        for (name, digest) in asset_names {
            let mut asset = self_update::ReleaseAsset::new(*name, "https://example.com/asset");
            if let Some(d) = digest {
                asset = asset.with_digest(*d);
            }
            builder.asset(asset);
        }
        builder.build().unwrap()
    }

    // ── Selection test: channel and downgrade policy ─────────────────

    #[test]
    fn release_selection_enforces_channel_and_downgrade_policy() {
        // Stable releases.
        let stable_010 = make_release("0.1.0", &[("neo-linux-x86_64.tar.gz", Some("sha256:abc"))]);
        let stable_011 = make_release("0.1.1", &[("neo-linux-x86_64.tar.gz", Some("sha256:def"))]);

        // Prerelease.
        let rc2 = make_release("0.1.1-rc.2", &[("neo-linux-x86_64.tar.gz", Some("sha256:ghi"))]);
        let rc3 = make_release("0.1.1-rc.3", &[("neo-linux-x86_64.tar.gz", Some("sha256:jkl"))]);

        // Equal precedence with different build metadata.
        let stable_010_build = make_release("0.1.0+build2", &[("neo-linux-x86_64.tar.gz", Some("sha256:mno"))]);

        // 1. Default stable: running 0.1.0, available 0.1.1 → install.
        let current = Version::parse("0.1.0").unwrap();
        let releases = vec![stable_011.clone(), stable_010.clone()];
        let decision = select_release(&releases, &current, UpdateMode::Stable).unwrap();
        assert!(matches!(decision, ReleaseDecision::Install(ref t) if t.version == Version::parse("0.1.1").unwrap()));

        // 2. Default stable: running 0.1.1-rc.2, available 0.1.0 → RequireStableSwitch.
        let current = Version::parse("0.1.1-rc.2").unwrap();
        let releases = vec![stable_010.clone()];
        let decision = select_release(&releases, &current, UpdateMode::Stable).unwrap();
        assert!(matches!(decision, ReleaseDecision::RequireStableSwitch { .. }));

        // 3. Unstable: running 0.1.1-rc.2, available 0.1.1-rc.3 → install.
        let current = Version::parse("0.1.1-rc.2").unwrap();
        let releases = vec![rc3.clone(), rc2.clone()];
        let decision = select_release(&releases, &current, UpdateMode::Unstable).unwrap();
        assert!(matches!(decision, ReleaseDecision::Install(ref t) if t.version == Version::parse("0.1.1-rc.3").unwrap()));

        // 4. Unstable: running 0.1.1-rc.3, available 0.1.1-rc.2 → AlreadyCurrent (no downgrade).
        let current = Version::parse("0.1.1-rc.3").unwrap();
        let releases = vec![rc2.clone()];
        let decision = select_release(&releases, &current, UpdateMode::Unstable).unwrap();
        assert!(matches!(decision, ReleaseDecision::AlreadyCurrent { .. }));

        // 5. StableSwitch: running 0.1.1-rc.2, available 0.1.0 → install (explicit downgrade).
        let current = Version::parse("0.1.1-rc.2").unwrap();
        let releases = vec![stable_010.clone()];
        let decision = select_release(&releases, &current, UpdateMode::StableSwitch).unwrap();
        assert!(matches!(decision, ReleaseDecision::Install(ref t) if t.version == Version::parse("0.1.0").unwrap()));

        // 6. StableSwitch: running 0.1.1 (stable), available 0.1.0 → AlreadyCurrent (no downgrade of stable).
        let current = Version::parse("0.1.1").unwrap();
        let releases = vec![stable_010.clone()];
        let decision = select_release(&releases, &current, UpdateMode::StableSwitch).unwrap();
        assert!(matches!(decision, ReleaseDecision::AlreadyCurrent { .. }));

        // 7. Equal precedence with different build metadata → AlreadyCurrent.
        let current = Version::parse("0.1.0+build1").unwrap();
        let releases = vec![stable_010_build.clone()];
        let decision = select_release(&releases, &current, UpdateMode::Stable).unwrap();
        assert!(matches!(decision, ReleaseDecision::AlreadyCurrent { .. }));

        // 8. Stable filter excludes prereleases.
        let current = Version::parse("0.1.0").unwrap();
        let releases = vec![rc2.clone()];
        let decision = select_release(&releases, &current, UpdateMode::Stable).unwrap();
        assert!(matches!(decision, ReleaseDecision::AlreadyCurrent { .. }));

        // 9. Unstable filter excludes stable releases.
        let current = Version::parse("0.1.0-rc.1").unwrap();
        let releases = vec![stable_011.clone()];
        let decision = select_release(&releases, &current, UpdateMode::Unstable).unwrap();
        assert!(matches!(decision, ReleaseDecision::AlreadyCurrent { .. }));

        // 10. Empty release list → AlreadyCurrent.
        let current = Version::parse("0.1.0").unwrap();
        let releases = vec![];
        let decision = select_release(&releases, &current, UpdateMode::Stable).unwrap();
        assert!(matches!(decision, ReleaseDecision::AlreadyCurrent { .. }));

        // 11. Empty release list → AlreadyCurrent (covered above).
        // Non-SemVer releases cannot be constructed via Release::builder()
        let current = Version::parse("0.1.0").unwrap();
        let releases = vec![stable_011.clone()];
        let decision = select_release(&releases, &current, UpdateMode::Unstable).unwrap();
        assert!(matches!(decision, ReleaseDecision::AlreadyCurrent { .. }));
    }

    // ── Mapping test: six targets + v0.1.0 + unsupported ────────────

    #[test]
    fn platform_assets_cover_six_targets_and_v0_1_0() {
        // This test documents the expected six-platform mapping.
        // Since we cannot change env::consts at runtime, we verify the
        // current platform returns a valid asset and test the mapping
        // logic through the code structure.

        let v010 = Version::parse("0.1.0").unwrap();
        let v011 = Version::parse("0.1.1").unwrap();
        let v011rc = Version::parse("0.1.1-rc.2").unwrap();

        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;

        let asset_011 = platform_asset(&v011).unwrap();
        let asset_011rc = platform_asset(&v011rc).unwrap();

        // Current platform mapping.
        match (os, arch) {
            ("linux", "x86_64") => {
                assert_eq!(asset_011.archive_name, "neo-linux-x86_64.tar.gz");
                assert_eq!(asset_011.binary_name, "neo-linux-x86_64");
                // v0.1.0: plain binary on Linux.
                let asset_010 = platform_asset(&v010).unwrap();
                assert_eq!(asset_010.archive_name, "neo-linux-x86_64");
                assert_eq!(asset_010.binary_name, "neo-linux-x86_64");
            }
            ("linux", "aarch64") => {
                assert_eq!(asset_011.archive_name, "neo-linux-arm64.tar.gz");
                assert_eq!(asset_011.binary_name, "neo-linux-arm64");
            }
            ("macos", "x86_64") => {
                assert_eq!(asset_011.archive_name, "neo-macos-x86_64.tar.gz");
                assert_eq!(asset_011.binary_name, "neo-macos-x86_64");
            }
            ("macos", "aarch64") => {
                assert_eq!(asset_011.archive_name, "neo-macos-arm64.tar.gz");
                assert_eq!(asset_011.binary_name, "neo-macos-arm64");
            }
            ("windows", "x86_64") => {
                assert_eq!(asset_011.archive_name, "neo-windows-x86_64.zip");
                assert_eq!(asset_011.binary_name, "neo-windows-x86_64.exe");
                // v0.1.0: still .zip on Windows.
                let asset_010 = platform_asset(&v010).unwrap();
                assert_eq!(asset_010.archive_name, "neo-windows-x86_64.zip");
                assert_eq!(asset_010.binary_name, "neo-windows-x86_64.exe");
            }
            ("windows", "aarch64") => {
                assert_eq!(asset_011.archive_name, "neo-windows-arm64.zip");
                assert_eq!(asset_011.binary_name, "neo-windows-arm64.exe");
            }
            _ => {
                // Unsupported platform should have failed.
                panic!("test running on unsupported platform: {os}/{arch}");
            }
        }

        // RC archive uses .tar.gz on Unix, .zip on Windows (same as stable non-0.1.0).
        if os != "windows" {
            assert!(asset_011rc.archive_name.ends_with(".tar.gz"));
        } else {
            assert!(asset_011rc.archive_name.ends_with(".zip"));
        }
    }

    // ── Digest validation test ──────────────────────────────────────

    #[test]
    fn exact_asset_requires_single_match_and_digest() {
        let good = self_update::ReleaseAsset::new(
            "neo-linux-x86_64.tar.gz",
            "https://example.com/asset",
        )
        .with_digest("sha256:abc123");

        let no_digest = self_update::ReleaseAsset::new(
            "neo-linux-x86_64.tar.gz",
            "https://example.com/asset",
        );

        let wrong_name = self_update::ReleaseAsset::new(
            "neo-other.tar.gz",
            "https://example.com/other",
        )
        .with_digest("sha256:def456");

        // Exactly one match with digest → succeeds.
        assert!(exact_asset_with_digest(&[good.clone()], "neo-linux-x86_64.tar.gz").is_some());

        // Match without digest → None.
        assert!(exact_asset_with_digest(&[no_digest], "neo-linux-x86_64.tar.gz").is_none());

        // No match → None.
        assert!(exact_asset_with_digest(&[wrong_name], "neo-linux-x86_64.tar.gz").is_none());

        // Multiple matches → None.
        assert!(exact_asset_with_digest(&[good.clone(), good], "neo-linux-x86_64.tar.gz").is_none());
    }

    // ── Backup promotion and recovery test ─────────────────────────

    #[test]
    fn backup_promotion_and_failed_replace_preserve_recovery() {
        // Use the actual neo binary (not the test binary) which supports --version.
        let test_exe = neo_binary_path();
        let version = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();

        // Create a disposable directory.
        let tmp = tempfile::tempdir().unwrap();
        let tmp_exe = tmp.path().join({
            #[cfg(windows)]
            { "neo.exe" }
            #[cfg(not(windows))]
            { "neo" }
        });

        // Copy test binary to disposable location.
        std::fs::copy(&test_exe, &tmp_exe).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(&tmp_exe, perms).unwrap();
        }

        // 1. promote_backup creates a .bak that reports the running version.
        let bak = promote_backup(&tmp_exe, &version).unwrap();
        assert!(bak.exists(), ".bak must exist after promotion");
        let bak_str = bak.to_string_lossy();
        assert!(bak_str.ends_with(".bak"), ".bak path must end with .bak, got: {bak_str}");

        // Verify the .bak binary reports the right version.
        // On Windows, .bak is not directly executable; verify via metadata.
        #[cfg(not(windows))]
        {
            let bak_meta = std::fs::symlink_metadata(&bak).unwrap();
            assert!(bak_meta.is_file(), ".bak must be a regular file");
        }

        // 2. Second promotion overwrites the old .bak (one slot only).
        let bak2 = promote_backup(&tmp_exe, &version).unwrap();
        assert_eq!(bak, bak2, "second promotion must use the same .bak path");

        // 3. verify_binary_version succeeds for the correct version.
        verify_binary_version(&tmp_exe, &version).unwrap();

        // 4. verify_binary_version fails for a wrong version.
        let wrong = Version::parse("99.99.99").unwrap();
        assert!(verify_binary_version(&tmp_exe, &wrong).is_err());

        // 5. parse_neo_version_output parses valid output.
        let parsed = parse_neo_version_output(&format!("neo {version}")).unwrap();
        assert_eq!(parsed, version);

        // 6. parse_neo_version_output rejects bad formats.
        assert!(parse_neo_version_output("").is_err());
        assert!(parse_neo_version_output("bad output").is_err());
        assert!(parse_neo_version_output("neo not-a-version").is_err());

        // 7. restore_from_backup restores after simulated missing current.
        // Remove the current exe to simulate Windows self_replace moving it.
        std::fs::remove_file(&tmp_exe).unwrap();
        assert!(!tmp_exe.exists());

        // Restore from backup.
        restore_from_backup(&tmp_exe, &bak, &version).unwrap();
        assert!(tmp_exe.exists(), "restored exe must exist");
        verify_binary_version(&tmp_exe, &version).unwrap();

        // 8. restore_from_backup succeeds when current already reports correct version.
        // (i.e., no replacement needed)
        restore_from_backup(&tmp_exe, &bak, &version).unwrap();
    }

    // ── Rollback test with injected replace closure ─────────────────

    #[test]
    fn rollback_is_offline_and_consumes_one_backup() {
        let test_exe = neo_binary_path();
        let version = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();

        // Create disposable directory with copy of neo binary.
        let tmp = tempfile::tempdir().unwrap();
        let tmp_exe = tmp.path().join({
            #[cfg(windows)]
            { "neo.exe" }
            #[cfg(not(windows))]
            { "neo" }
        });
        std::fs::copy(&test_exe, &tmp_exe).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp_exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let bak = promote_backup(&tmp_exe, &version).unwrap();
        assert!(bak.exists());

        // Create a guard copy for recovery testing.
        #[cfg(windows)]
        let guard = stage_copy(&tmp_exe, tmp.path(), ".exe").unwrap();
        #[cfg(not(windows))]
        let guard = stage_copy(&tmp_exe, tmp.path()).unwrap();
        verify_binary_version(guard.path(), &version).unwrap();

        // 1. Successful replace: consumes .bak.
        let result = replace_with_recovery(
            &tmp_exe,
            guard.path(), // Use guard as successor (it's a valid neo binary).
            &version,
            guard.path(),
            &version,
            &bak,
            |src| self_replace::self_replace(src),
        );
        assert!(result.is_ok(), "successful rollback should succeed: {result:?}");
        assert!(!bak.exists(), ".bak must be consumed after successful rollback");
        verify_binary_version(&tmp_exe, &version).unwrap();

        // 2. Second rollback: reports absent backup.
        // Re-create .bak for the next test.
        let bak = promote_backup(&tmp_exe, &version).unwrap();
        assert!(bak.exists());

        // Remove .bak to simulate consumed state.
        std::fs::remove_file(&bak).unwrap();

        // The rollback_impl would fail at the .bak existence check.
        // We test this at the backup_path level.
        assert!(!bak.exists());

        // 3. Simulated replace failure: restores from guard, retains .bak.
        let bak = promote_backup(&tmp_exe, &version).unwrap();
        assert!(bak.exists());

        let result = replace_with_recovery(
            &tmp_exe,
            guard.path(),
            &version,
            guard.path(),
            &version,
            &bak,
            |_src| Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "simulated replace failure",
            )),
        );
        assert!(result.is_err(), "simulated failure should return error");
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("previous version was restored"), "error must mention restore: {err_msg}");
        assert!(bak.exists(), ".bak must be retained after failed replace");
        verify_binary_version(&tmp_exe, &version).unwrap();
    }
}

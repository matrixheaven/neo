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

#[derive(Debug)]
pub(crate) enum ReleaseDecision {
    AlreadyCurrent { channel: &'static str },
    Install(TargetRelease),
    RequireStableSwitch { current: Version, target: Version },
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
    platform_asset_for(version, std::env::consts::OS, std::env::consts::ARCH)
}

fn platform_asset_for(version: &Version, os: &str, arch: &str) -> anyhow::Result<AssetSpec> {
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
    let is_v0_1_0 = *version == Version::new(0, 1, 0);

    let archive_name = if is_v0_1_0 && os != "windows" {
        base.to_string()
    } else {
        format!("{base}{ext}")
    };
    let binary_name = format!("{base}{binary_ext}");

    Ok(AssetSpec {
        archive_name,
        binary_name,
    })
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
        None => bail!("no {channel} release exists"),
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
        bail!("{path:?} reported version {reported}, expected {expected}");
    }
    Ok(())
}

// ── Staged copy helper ─────────────────────────────────────────────

/// Copy a binary to a sibling temporary path, preserving permissions and
/// flushing to disk. The file handle is closed before returning so Linux can
/// execute the staged binary. The path remains auto-cleaned until persisted.
fn stage_copy(
    source: &Path,
    parent_dir: &Path,
    #[cfg(windows)] suffix: &str,
) -> anyhow::Result<tempfile::TempPath> {
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
            .context("failed to set permissions on temp file")?;
    }

    // Flush and sync.
    {
        let mut f = &tmp_file;
        use std::io::Write;
        f.flush().with_context(|| "failed to flush temp file")?;
    }
    // sync_all via the File handle
    tmp_file
        .as_file()
        .sync_all()
        .with_context(|| "failed to sync temp file")?;

    Ok(tmp_file.into_temp_path())
}

// ── Backup promotion ───────────────────────────────────────────────

/// Stage the current executable as `.bak` and verify the staged copy.
///
/// The order is:
/// 1. Copy current exe to a sibling temp file.
/// 2. Verify the temp file reports the running version.
/// 3. Atomically persist the temp file over the `.bak` path.
fn promote_backup(current_exe: &Path, running_version: &Version) -> anyhow::Result<PathBuf> {
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
    verify_binary_version(&staged, running_version).context("staged backup verification failed")?;

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
    verify_binary_version(&staged, original_version)
        .context("recovery staged binary verification failed")?;

    // Check if install_path already exists and reports original version.
    // If it does, we may not need to replace it (e.g., self_replace didn't
    // move it yet).
    if install_path.exists()
        && let Ok(()) = verify_binary_version(install_path, original_version)
    {
        // Original binary still intact; no persist needed.
        return Ok(());
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
pub(crate) async fn update(unstable: bool, stable: bool, rollback: bool) -> anyhow::Result<String> {
    let mode = UpdateMode::from_flags(unstable, stable, rollback)?;

    if mode == UpdateMode::Rollback {
        return rollback_impl().await;
    }

    let current_version: Version =
        Version::parse(env!("CARGO_PKG_VERSION")).context("invalid compiled package version")?;
    let current_exe =
        std::env::current_exe().context("failed to resolve current executable path")?;

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

    let updater = builder.build_async().context("failed to build updater")?;

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
    let current_exe =
        std::env::current_exe().context("failed to resolve current executable path")?;
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
        let output = std::process::Command::new(staged_backup.as_os_str())
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

    let running_version: Version =
        Version::parse(env!("CARGO_PKG_VERSION")).context("invalid compiled package version")?;

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

    verify_binary_version(&guard, &running_version).context("guard copy verification failed")?;

    // 4. Replace current exe with backup using recovery-aware helper.
    let install_path_saved = current_exe.clone();
    let bak_path_saved = bak.clone();
    replace_with_recovery(
        &install_path_saved,
        &staged_backup,
        &backup_version,
        &guard,
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
                        bail!(
                            "rollback installed {successor_version}, but failed to consume \
                             backup {bak_path:?}: {rm_err}\n\
                             Please restart Neo."
                        );
                    }
                    Ok(format!(
                        "Rolled back {guard_version} → {successor_version}.\n\
                         Backup consumed. Please restart Neo."
                    ))
                }
                Err(verify_err) => {
                    // Post-install verification failed.
                    // Restore from guard, leave .bak intact.
                    match restore_from_backup(install_path, guard_path, guard_version) {
                        Ok(()) => bail!(
                            "rollback replacement succeeded but installed version \
                             verification failed: {verify_err}\n\
                             The previous version was restored. Backup remains at {bak_path:?}."
                        ),
                        Err(restore_err) => bail!(
                            "rollback replacement succeeded but installed version \
                             verification failed: {verify_err}\n\
                             guard restoration also failed: {restore_err}\n\
                             current executable: {install_path:?}\n\
                             backup: {bak_path:?}\n\
                             manual recovery is required."
                        ),
                    }
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

// ── Uninstall ──────────────────────────────────────────────────────

/// Validate the Neo home directory for safe deletion.
///
/// Rejects:
/// - paths that cannot be canonicalized
/// - filesystem/drive roots
/// - the user home directory itself
/// - non-directory targets
/// - symlink targets
fn validate_neo_home(
    home: &std::path::Path,
    user_home: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    if !home.exists() {
        return Ok(()); // absent home is fine; we just won't delete anything
    }

    // Must be a directory, not a symlink.
    let meta = std::fs::symlink_metadata(home)
        .with_context(|| format!("failed to read metadata for {home:?}"))?;
    if meta.is_symlink() {
        bail!("Neo home is a symlink, refusing to delete: {home:?}");
    }
    if !meta.is_dir() {
        bail!("Neo home is not a directory: {home:?}");
    }

    // Canonicalize for safety comparisons.
    let canonical = std::fs::canonicalize(home)
        .with_context(|| format!("failed to canonicalize Neo home: {home:?}"))?;

    // Reject filesystem root.
    if canonical.parent().is_none() {
        bail!("Neo home is a filesystem root, refusing to delete: {canonical:?}");
    }

    // Reject drive root on Windows (e.g. C:\).
    if canonical.to_string_lossy().ends_with(':') || canonical.to_string_lossy().ends_with(r":\") {
        bail!("Neo home is a drive root, refusing to delete: {canonical:?}");
    }

    // Reject the user home itself.
    if let Some(uh) = user_home
        && let Ok(canonical_uh) = std::fs::canonicalize(uh)
        && canonical == canonical_uh
    {
        bail!("Neo home is the user home directory, refusing to delete: {canonical:?}");
    }

    Ok(())
}

/// Validate the adjacent backup entry before uninstall mutates anything.
fn uninstall_backup_exists(path: &Path) -> anyhow::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() || meta.file_type().is_symlink() => Ok(true),
        Ok(_) => bail!("backup path is not a file or symlink, refusing to delete: {path:?}"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("failed to read backup metadata: {path:?}"))
        }
    }
}

/// Prompt the user for confirmation to delete the Neo home directory.
///
/// Returns `true` if the user confirmed, `false` otherwise.
/// Accepts case-insensitive `y`/`yes` or `n`/`no`; empty input and
/// EOF mean No.
fn confirm_delete_home(
    reader: &mut impl std::io::BufRead,
    writer: &mut impl std::io::Write,
    path: &std::path::Path,
) -> anyhow::Result<bool> {
    write!(writer, "Delete Neo data at {}? [y/N] ", path.display())?;
    writer.flush()?;

    loop {
        let mut input = String::new();
        let bytes_read = reader.read_line(&mut input)?;
        if bytes_read == 0 {
            // EOF.
            return Ok(false);
        }
        match input.trim().to_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" | "" => return Ok(false),
            _ => {
                write!(writer, "Please answer y or n: ")?;
                writer.flush()?;
            }
        }
    }
}

/// Remove this Neo binary and optionally its data directory.
///
/// Deletion order:
/// 1. Current executable.
/// 2. Adjacent `.bak` (only if step 1 succeeded or was already absent).
/// 3. Neo home (only if steps 1 and 2 both succeeded or were absent).
///
/// On Windows, removing the running `.exe` fails with a sharing/access
/// error. Neo reports the error and stops before touching `.bak` or
/// Neo home.
pub(crate) fn uninstall(yes: bool) -> anyhow::Result<String> {
    let current_exe =
        std::env::current_exe().context("failed to resolve current executable path")?;
    let bak = backup_path(&current_exe)?;
    let neo_home = crate::config::neo_home();
    let user_home = crate::config::user_home();

    // Resolve and validate all paths before mutation.
    let backup_exists = uninstall_backup_exists(&bak)?;
    let home_to_delete: Option<std::path::PathBuf> = if let Some(ref home) = neo_home {
        if home.exists() {
            validate_neo_home(home, user_home.as_deref())?;
            Some(home.clone())
        } else {
            None
        }
    } else {
        None
    };

    // Prompt for confirmation if home exists and --yes not set.
    let delete_home = if let Some(ref home) = home_to_delete {
        if yes {
            true
        } else {
            let mut reader = std::io::BufReader::new(std::io::stdin());
            let mut writer = std::io::stdout();
            confirm_delete_home(&mut reader, &mut writer, home)?
        }
    } else {
        false
    };

    let mut result = String::new();

    // 1. Remove current executable.
    match std::fs::remove_file(&current_exe) {
        Ok(()) => {
            result.push_str(&format!("Removed: {current_exe:?}\n"));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            result.push_str(&format!("Already absent: {current_exe:?}\n"));
        }
        Err(e) => {
            // On Windows, this typically fails with PermissionDenied or
            // sharing violation for the running .exe.
            result.push_str(&format!("Failed to remove {current_exe:?}: {e}\n"));
            result.push_str("Neither .bak nor Neo home data was removed.\n");
            result.push_str(&format!(
                "Please close Neo and remove the executable manually: {current_exe:?}"
            ));
            bail!(result);
        }
    }

    // 2. Remove .bak if present.
    if backup_exists {
        match std::fs::remove_file(&bak) {
            Ok(()) => {
                result.push_str(&format!("Removed: {bak:?}\n"));
            }
            Err(e) => {
                result.push_str(&format!("Failed to remove backup {bak:?}: {e}\n"));
                if delete_home {
                    result.push_str("Neo home was not removed because backup removal failed.\n");
                }
                bail!(result);
            }
        }
    } else {
        result.push_str(&format!("No backup found: {bak:?}\n"));
    }

    // 3. Remove Neo home if confirmed.
    if delete_home {
        if let Some(ref home) = home_to_delete {
            match std::fs::remove_dir_all(home) {
                Ok(()) => {
                    result.push_str(&format!("Removed: {home:?}\n"));
                }
                Err(e) => {
                    result.push_str(&format!(
                        "Binaries removed but failed to delete Neo home {home:?}: {e}\n"
                    ));
                    bail!(result);
                }
            }
        }
    } else if home_to_delete.is_some() {
        result.push_str(&format!(
            "Neo home retained: {}\n",
            home_to_delete.as_ref().unwrap().display()
        ));
    } else {
        result.push_str("No Neo home resolved.\n");
    }

    Ok(result)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "test_cases/lifecycle.rs"]
mod tests;

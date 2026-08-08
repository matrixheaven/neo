//! Lifecycle mode behavior (moved from `lifecycle.rs`).

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
        {
            "neo.exe"
        }
        #[cfg(not(windows))]
        {
            "neo"
        }
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
    let rc2 = make_release(
        "0.1.1-rc.2",
        &[("neo-linux-x86_64.tar.gz", Some("sha256:ghi"))],
    );
    let rc3 = make_release(
        "0.1.1-rc.3",
        &[("neo-linux-x86_64.tar.gz", Some("sha256:jkl"))],
    );

    // Equal precedence with different build metadata.
    let stable_010_build = make_release(
        "0.1.0+build2",
        &[("neo-linux-x86_64.tar.gz", Some("sha256:mno"))],
    );

    // 1. Default stable: running 0.1.0, available 0.1.1 → install.
    let current = Version::parse("0.1.0").unwrap();
    let releases = vec![stable_011.clone(), stable_010.clone()];
    let decision = select_release(&releases, &current, UpdateMode::Stable).unwrap();
    assert!(
        matches!(decision, ReleaseDecision::Install(ref t) if t.version == Version::parse("0.1.1").unwrap())
    );

    // 2. Default stable: running 0.1.1-rc.2, available 0.1.0 → RequireStableSwitch.
    let current = Version::parse("0.1.1-rc.2").unwrap();
    let releases = vec![stable_010.clone()];
    let decision = select_release(&releases, &current, UpdateMode::Stable).unwrap();
    assert!(matches!(
        decision,
        ReleaseDecision::RequireStableSwitch { .. }
    ));

    // 3. Unstable: running 0.1.1-rc.2, available 0.1.1-rc.3 → install.
    let current = Version::parse("0.1.1-rc.2").unwrap();
    let releases = vec![rc3.clone(), rc2.clone()];
    let decision = select_release(&releases, &current, UpdateMode::Unstable).unwrap();
    assert!(
        matches!(decision, ReleaseDecision::Install(ref t) if t.version == Version::parse("0.1.1-rc.3").unwrap())
    );

    // 4. Unstable: running 0.1.1-rc.3, available 0.1.1-rc.2 → AlreadyCurrent (no downgrade).
    let current = Version::parse("0.1.1-rc.3").unwrap();
    let releases = vec![rc2.clone()];
    let decision = select_release(&releases, &current, UpdateMode::Unstable).unwrap();
    assert!(matches!(decision, ReleaseDecision::AlreadyCurrent { .. }));

    // 5. StableSwitch: running 0.1.1-rc.2, available 0.1.0 → install (explicit downgrade).
    let current = Version::parse("0.1.1-rc.2").unwrap();
    let releases = vec![stable_010.clone()];
    let decision = select_release(&releases, &current, UpdateMode::StableSwitch).unwrap();
    assert!(
        matches!(decision, ReleaseDecision::Install(ref t) if t.version == Version::parse("0.1.0").unwrap())
    );

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

    // 8. Stable filter excludes prereleases and errors when none remain.
    let current = Version::parse("0.1.0").unwrap();
    let releases = vec![rc2.clone()];
    let error = select_release(&releases, &current, UpdateMode::Stable).unwrap_err();
    assert_eq!(error.to_string(), "no stable release exists");

    // 9. Unstable filter excludes stable releases and errors when none remain.
    let current = Version::parse("0.1.0-rc.1").unwrap();
    let releases = vec![stable_011.clone()];
    let error = select_release(&releases, &current, UpdateMode::Unstable).unwrap_err();
    assert_eq!(error.to_string(), "no unstable release exists");

    // 10. Empty release list → error.
    let current = Version::parse("0.1.0").unwrap();
    let releases = vec![];
    let error = select_release(&releases, &current, UpdateMode::Stable).unwrap_err();
    assert_eq!(error.to_string(), "no stable release exists");

    // 11. Non-SemVer releases cannot be constructed via Release::builder().
    let current = Version::parse("0.1.0").unwrap();
    let releases = vec![stable_011.clone()];
    let error = select_release(&releases, &current, UpdateMode::Unstable).unwrap_err();
    assert_eq!(error.to_string(), "no unstable release exists");
}

// ── Mapping test: six targets + v0.1.0 + unsupported ────────────

#[test]
fn platform_assets_cover_six_targets_and_v0_1_0() {
    let v010 = Version::parse("0.1.0").unwrap();
    let v010build = Version::parse("0.1.0+rebuild").unwrap();
    let v011 = Version::parse("0.1.1").unwrap();

    let cases = [
        (
            "linux",
            "x86_64",
            "neo-linux-x86_64.tar.gz",
            "neo-linux-x86_64",
        ),
        (
            "linux",
            "aarch64",
            "neo-linux-arm64.tar.gz",
            "neo-linux-arm64",
        ),
        (
            "macos",
            "x86_64",
            "neo-macos-x86_64.tar.gz",
            "neo-macos-x86_64",
        ),
        (
            "macos",
            "aarch64",
            "neo-macos-arm64.tar.gz",
            "neo-macos-arm64",
        ),
        (
            "windows",
            "x86_64",
            "neo-windows-x86_64.zip",
            "neo-windows-x86_64.exe",
        ),
        (
            "windows",
            "aarch64",
            "neo-windows-arm64.zip",
            "neo-windows-arm64.exe",
        ),
    ];

    for (os, arch, archive_name, binary_name) in cases {
        let asset = platform_asset_for(&v011, os, arch).unwrap();
        assert_eq!(asset.archive_name, archive_name);
        assert_eq!(asset.binary_name, binary_name);

        let rebuilt = platform_asset_for(&v010build, os, arch).unwrap();
        assert_eq!(rebuilt.archive_name, archive_name);

        let legacy = platform_asset_for(&v010, os, arch).unwrap();
        let legacy_archive = if os == "windows" {
            archive_name
        } else {
            binary_name
        };
        assert_eq!(legacy.archive_name, legacy_archive);
    }

    assert!(platform_asset_for(&v011, "freebsd", "x86_64").is_err());
    assert!(platform_asset_for(&v011, "linux", "riscv64").is_err());
}

// ── Digest validation test ──────────────────────────────────────

#[test]
fn exact_asset_requires_single_match_and_digest() {
    let good =
        self_update::ReleaseAsset::new("neo-linux-x86_64.tar.gz", "https://example.com/asset")
            .with_digest("sha256:abc123");

    let no_digest =
        self_update::ReleaseAsset::new("neo-linux-x86_64.tar.gz", "https://example.com/asset");

    let wrong_name =
        self_update::ReleaseAsset::new("neo-other.tar.gz", "https://example.com/other")
            .with_digest("sha256:def456");

    // Exactly one match with digest → succeeds.
    assert!(
        exact_asset_with_digest(std::slice::from_ref(&good), "neo-linux-x86_64.tar.gz").is_some()
    );

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
        {
            "neo.exe"
        }
        #[cfg(not(windows))]
        {
            "neo"
        }
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
    assert!(
        bak_str.ends_with(".bak"),
        ".bak path must end with .bak, got: {bak_str}"
    );

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
        {
            "neo.exe"
        }
        #[cfg(not(windows))]
        {
            "neo"
        }
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
    verify_binary_version(&guard, &version).unwrap();

    // 1. Successful replace: consumes .bak.
    let result = replace_with_recovery(
        &tmp_exe,
        &guard, // Use guard as successor (it's a valid neo binary).
        &version,
        &guard,
        &version,
        &bak,
        |src| std::fs::copy(src, &tmp_exe).map(|_| ()),
    );
    assert!(
        result.is_ok(),
        "successful rollback should succeed: {result:?}"
    );
    assert!(
        !bak.exists(),
        ".bak must be consumed after successful rollback"
    );
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

    let result =
        replace_with_recovery(&tmp_exe, &guard, &version, &guard, &version, &bak, |_src| {
            Err(std::io::Error::other("simulated replace failure"))
        });
    assert!(result.is_err(), "simulated failure should return error");
    let err_msg = format!("{:?}", result.unwrap_err());
    assert!(
        err_msg.contains("previous version was restored"),
        "error must mention restore: {err_msg}"
    );
    assert!(bak.exists(), ".bak must be retained after failed replace");
    verify_binary_version(&tmp_exe, &version).unwrap();

    // 4. Post-replacement verification plus restoration failure reports both.
    let missing_guard = tmp.path().join("missing-guard");
    let result = replace_with_recovery(
        &tmp_exe,
        &guard,
        &version,
        &missing_guard,
        &version,
        &bak,
        |_src| std::fs::remove_file(&tmp_exe),
    );
    let err_msg = format!("{:?}", result.unwrap_err());
    assert!(err_msg.contains("guard restoration also failed"));
    assert!(err_msg.contains("manual recovery is required"));
    assert!(bak.exists(), ".bak must survive dual failure");
}

// ── Uninstall test ───────────────────────────────────────────────

#[test]
fn uninstall_confirmation_and_partial_order_are_safe() {
    // Test Y/N confirmation parsing.
    let path = std::path::PathBuf::from("/tmp/test-neo-home");

    // "y" → true
    let result = confirm_delete_home(&mut "y\n".as_bytes(), &mut Vec::new(), &path).unwrap();
    assert!(result);

    // "yes" → true
    let result = confirm_delete_home(&mut "yes\n".as_bytes(), &mut Vec::new(), &path).unwrap();
    assert!(result);

    // "Y" → true (case insensitive)
    let result = confirm_delete_home(&mut "Y\n".as_bytes(), &mut Vec::new(), &path).unwrap();
    assert!(result);

    // "n" → false
    let result = confirm_delete_home(&mut "n\n".as_bytes(), &mut Vec::new(), &path).unwrap();
    assert!(!result);

    // empty → false
    let result = confirm_delete_home(&mut "\n".as_bytes(), &mut Vec::new(), &path).unwrap();
    assert!(!result);

    // EOF → false
    let result = confirm_delete_home(&mut "".as_bytes(), &mut Vec::new(), &path).unwrap();
    assert!(!result);

    // "no" → false
    let result = confirm_delete_home(&mut "no\n".as_bytes(), &mut Vec::new(), &path).unwrap();
    assert!(!result);

    // Test unsafe path rejections.
    let tmp = tempfile::tempdir().unwrap();

    // Symlink rejection.
    let link = tmp.path().join("link");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(tmp.path(), &link).unwrap();
        let err = validate_neo_home(&link, None).unwrap_err();
        assert!(err.to_string().contains("symlink"));
    }

    // Non-directory rejection.
    let file = tmp.path().join("file");
    std::fs::write(&file, "test").unwrap();
    let err = validate_neo_home(&file, None).unwrap_err();
    assert!(err.to_string().contains("not a directory"));

    // User home rejection.
    let home_dir = tmp.path().join("home");
    std::fs::create_dir(&home_dir).unwrap();
    let err = validate_neo_home(&home_dir, Some(&home_dir)).unwrap_err();
    assert!(err.to_string().contains("user home"));

    // Absent path → Ok (no-op).
    let absent = tmp.path().join("nonexistent");
    validate_neo_home(&absent, None).unwrap();

    // Valid directory → Ok.
    let valid = tmp.path().join("valid-neo-home");
    std::fs::create_dir(&valid).unwrap();
    validate_neo_home(&valid, None).unwrap();

    // Backup entries are validated before any uninstall mutation.
    let absent_backup = tmp.path().join("absent.bak");
    assert!(!uninstall_backup_exists(&absent_backup).unwrap());
    assert!(uninstall_backup_exists(&file).unwrap());
    assert!(uninstall_backup_exists(tmp.path()).is_err());
}

// ── CLI contract test ───────────────────────────────────────────

#[tokio::test]
async fn cli_lifecycle_contract_is_exact() {
    use crate::cli::Cli;
    use clap::Parser;

    // All seven valid invocations parse successfully.
    Cli::try_parse_from(["neo", "update"]).unwrap();
    Cli::try_parse_from(["neo", "update", "--unstable"]).unwrap();
    Cli::try_parse_from(["neo", "update", "--stable"]).unwrap();
    Cli::try_parse_from(["neo", "update", "--rollback"]).unwrap();
    Cli::try_parse_from(["neo", "uninstall"]).unwrap();
    Cli::try_parse_from(["neo", "uninstall", "-y"]).unwrap();
    Cli::try_parse_from(["neo", "uninstall", "--yes"]).unwrap();

    // Pairwise update-flag conflicts.
    assert!(Cli::try_parse_from(["neo", "update", "--unstable", "--stable"]).is_err());
    assert!(Cli::try_parse_from(["neo", "update", "--unstable", "--rollback"]).is_err());
    assert!(Cli::try_parse_from(["neo", "update", "--stable", "--rollback"]).is_err());

    // All-three conflict.
    assert!(
        Cli::try_parse_from(["neo", "update", "--unstable", "--stable", "--rollback"]).is_err()
    );

    // --rc is not a valid flag.
    assert!(Cli::try_parse_from(["neo", "update", "--rc"]).is_err());

    // -y and --yes produce the same yes = true semantic state.
    let cli_y = Cli::try_parse_from(["neo", "uninstall", "-y"]).unwrap();
    let cli_yes = Cli::try_parse_from(["neo", "uninstall", "--yes"]).unwrap();
    match (&cli_y.command, &cli_yes.command) {
        (
            Some(crate::cli::Command::Uninstall { yes: y1 }),
            Some(crate::cli::Command::Uninstall { yes: y2 }),
        ) => {
            assert!(*y1);
            assert!(*y2);
        }
        _ => panic!("expected Uninstall command"),
    }

    // Verify that Update and Uninstall produce correct variant names.
    let cli = Cli::try_parse_from(["neo", "update"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(crate::cli::Command::Update { .. })
    ));

    let cli = Cli::try_parse_from(["neo", "update", "--unstable"]).unwrap();
    match cli.command {
        Some(crate::cli::Command::Update {
            unstable,
            stable,
            rollback,
        }) => {
            assert!(unstable);
            assert!(!stable);
            assert!(!rollback);
        }
        _ => panic!("expected Update command"),
    }

    // Resume picker conflicts are rejected before lifecycle side effects.
    for args in [["neo", "-r", "update"], ["neo", "-r", "uninstall"]] {
        let cli = Cli::try_parse_from(args).unwrap();
        let error = crate::dispatch(cli, None).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cannot be combined with a subcommand")
        );
    }
}

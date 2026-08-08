//! Workspace policy behavior (moved from `workspace_policy.rs`).

use super::*;

fn added_root(path: PathBuf, read: bool, write: bool) -> WorkspaceAccessRoot {
    WorkspaceAccessRoot {
        path,
        kind: WorkspaceAccessRootKind::Added,
        read,
        write,
    }
}

#[test]
fn read_allows_primary_relative_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("src.txt");
    std::fs::write(&file, "hello").expect("write");
    let policy = WorkspaceAccessPolicy::new(dir.path()).expect("policy");

    let resolved = policy
        .resolve_read_path(Path::new("src.txt"))
        .expect("resolve");

    assert_eq!(resolved, file.canonicalize().expect("canonical file"));
}

#[test]
fn read_allows_absolute_path_inside_added_read_root() {
    let primary = tempfile::tempdir().expect("primary");
    let added = tempfile::tempdir().expect("added");
    let file = added.path().join("lib.rs");
    std::fs::write(&file, "mod lib;").expect("write");
    let policy = WorkspaceAccessPolicy::with_roots(
        primary.path(),
        [added_root(
            added.path().canonicalize().expect("canonical added"),
            true,
            false,
        )],
    )
    .expect("policy");

    let resolved = policy.resolve_read_path(&file).expect("resolve");

    assert_eq!(resolved, file.canonicalize().expect("canonical file"));
}

#[test]
fn read_allows_missing_file_inside_readable_root_when_parent_exists() {
    let primary = tempfile::tempdir().expect("primary");
    let path = primary.path().join("missing.txt");
    let policy = WorkspaceAccessPolicy::new(primary.path()).expect("policy");

    let resolved = policy.resolve_read_path(&path).expect("resolve");

    assert_eq!(
        resolved,
        primary
            .path()
            .canonicalize()
            .expect("canonical primary")
            .join("missing.txt")
    );
}

#[test]
fn read_allows_added_root_supplied_as_symlink_path() {
    let primary = tempfile::tempdir().expect("primary");
    let added = tempfile::tempdir().expect("added");
    let links = tempfile::tempdir().expect("links");
    let link = links.path().join("added-link");
    if !symlink_created(create_dir_symlink(added.path(), &link)) {
        return;
    }
    let file = added.path().join("lib.rs");
    std::fs::write(&file, "mod lib;").expect("write");
    let policy =
        WorkspaceAccessPolicy::with_roots(primary.path(), [added_root(link.clone(), true, false)])
            .expect("policy");

    let resolved = policy.resolve_read_path(&file).expect("resolve");

    assert_eq!(resolved, file.canonicalize().expect("canonical file"));
    assert_eq!(
        policy.roots()[1].path,
        added.path().canonicalize().expect("canonical added")
    );
}

#[test]
fn with_roots_ignores_missing_and_non_directory_added_roots() {
    let primary = tempfile::tempdir().expect("primary");
    let added = tempfile::tempdir().expect("added");
    let non_directory = added.path().join("file.txt");
    std::fs::write(&non_directory, "not a directory").expect("write");
    let missing = added.path().join("missing");
    let policy = WorkspaceAccessPolicy::with_roots(
        primary.path(),
        [
            added_root(non_directory, true, false),
            added_root(missing, true, true),
        ],
    )
    .expect("policy");

    assert_eq!(policy.roots().len(), 1);
    assert_eq!(policy.roots()[0].kind, WorkspaceAccessRootKind::Primary);
}

#[test]
fn read_denies_absolute_path_inside_read_disabled_root() {
    let primary = tempfile::tempdir().expect("primary");
    let added = tempfile::tempdir().expect("added");
    let file = added.path().join("lib.rs");
    std::fs::write(&file, "mod lib;").expect("write");
    let policy = WorkspaceAccessPolicy::with_roots(
        primary.path(),
        [added_root(
            added.path().canonicalize().expect("canonical added"),
            false,
            false,
        )],
    )
    .expect("policy");

    let err = policy.resolve_read_path(&file).expect_err("denied");

    assert!(matches!(err, WorkspaceAccessError::ReadDenied { .. }));
}

#[test]
fn write_allows_new_file_inside_added_write_root() {
    let primary = tempfile::tempdir().expect("primary");
    let added = tempfile::tempdir().expect("added");
    let path = added.path().join("new.txt");
    let policy = WorkspaceAccessPolicy::with_roots(
        primary.path(),
        [added_root(
            added.path().canonicalize().expect("canonical added"),
            true,
            true,
        )],
    )
    .expect("policy");

    let resolved = policy.resolve_write_path(&path).expect("resolve");

    assert_eq!(
        resolved,
        added
            .path()
            .canonicalize()
            .expect("canonical added")
            .join("new.txt")
    );
}

#[test]
fn write_denies_new_file_inside_read_only_added_root() {
    let primary = tempfile::tempdir().expect("primary");
    let added = tempfile::tempdir().expect("added");
    let path = added.path().join("new.txt");
    let policy = WorkspaceAccessPolicy::with_roots(
        primary.path(),
        [added_root(
            added.path().canonicalize().expect("canonical added"),
            true,
            false,
        )],
    )
    .expect("policy");

    let err = policy.resolve_write_path(&path).expect_err("denied");

    assert!(matches!(err, WorkspaceAccessError::WriteDenied { .. }));
}

#[test]
fn write_denies_existing_symlink_escape() {
    let primary = tempfile::tempdir().expect("primary");
    let added = tempfile::tempdir().expect("added");
    let outside = tempfile::tempdir().expect("outside");
    let outside_file = outside.path().join("secret.txt");
    std::fs::write(&outside_file, "secret").expect("write");
    let link = added.path().join("link.txt");
    if !symlink_created(create_symlink(&outside_file, &link)) {
        return;
    }
    let policy = WorkspaceAccessPolicy::with_roots(
        primary.path(),
        [added_root(
            added.path().canonicalize().expect("canonical added"),
            true,
            true,
        )],
    )
    .expect("policy");

    let err = policy.resolve_write_path(&link).expect_err("denied");

    assert!(matches!(
        err,
        WorkspaceAccessError::PathOutsideWorkspace { .. }
    ));
}

#[test]
fn write_denies_dangling_symlink_escape_without_creating_target() {
    let primary = tempfile::tempdir().expect("primary");
    let added = tempfile::tempdir().expect("added");
    let outside = tempfile::tempdir().expect("outside");
    let outside_file = outside.path().join("missing.txt");
    let link = added.path().join("link.txt");
    if !symlink_created(create_symlink(&outside_file, &link)) {
        return;
    }
    let policy = WorkspaceAccessPolicy::with_roots(
        primary.path(),
        [added_root(
            added.path().canonicalize().expect("canonical added"),
            true,
            true,
        )],
    )
    .expect("policy");

    let err = policy.resolve_write_path(&link).expect_err("denied");

    assert!(matches!(
        err,
        WorkspaceAccessError::PathOutsideWorkspace { .. }
    ));
    assert!(!outside_file.exists());
}

#[test]
fn write_denies_root_without_read_even_when_write_true() {
    let primary = tempfile::tempdir().expect("primary");
    let added = tempfile::tempdir().expect("added");
    let path = added.path().join("new.txt");
    let policy = WorkspaceAccessPolicy::with_roots(
        primary.path(),
        [added_root(
            added.path().canonicalize().expect("canonical added"),
            false,
            true,
        )],
    )
    .expect("policy");

    let err = policy.resolve_write_path(&path).expect_err("denied");

    assert!(matches!(err, WorkspaceAccessError::WriteDenied { .. }));
    assert!(!policy.roots()[1].write);
}

#[test]
fn read_rejects_symlink_escape() {
    let primary = tempfile::tempdir().expect("primary");
    let outside = tempfile::tempdir().expect("outside");
    let outside_file = outside.path().join("secret.txt");
    std::fs::write(&outside_file, "secret").expect("write");
    let link = primary.path().join("link.txt");
    if !symlink_created(create_symlink(&outside_file, &link)) {
        return;
    }
    let policy = WorkspaceAccessPolicy::new(primary.path()).expect("policy");

    let err = policy.resolve_read_path(&link).expect_err("escape denied");

    assert!(matches!(
        err,
        WorkspaceAccessError::PathOutsideWorkspace { .. }
    ));
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(unix)]
fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(unix)]
fn symlink_created(result: std::io::Result<()>) -> bool {
    result.expect("symlink");
    true
}

#[cfg(windows)]
fn symlink_created(result: std::io::Result<()>) -> bool {
    result.is_ok()
}

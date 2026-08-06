use super::*;

#[test]
fn tool_context_resolve_workspace_path_uses_added_read_root() {
    let primary = tempfile::tempdir().expect("primary");
    let added = tempfile::tempdir().expect("added");
    let file = added.path().join("lib.rs");
    std::fs::write(&file, "pub fn lib() {}").expect("write");
    let policy = crate::WorkspaceAccessPolicy::with_roots(
        primary.path(),
        [crate::WorkspaceAccessRoot {
            path: added.path().canonicalize().expect("canonical added"),
            kind: crate::WorkspaceAccessRootKind::Added,
            read: true,
            write: false,
        }],
    )
    .expect("policy");
    let ctx = ToolContext::new(primary.path())
        .expect("context")
        .with_workspace_policy(policy);

    let resolved = ctx.resolve_workspace_path(&file).expect("resolve");

    assert_eq!(resolved, file.canonicalize().expect("canonical file"));
}

#[cfg(unix)]
#[test]
fn authorized_external_write_path_captures_canonical_target_before_retarget() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().expect("workspace");
    let external = tempfile::tempdir().expect("external");
    let first = external.path().join("first");
    let second = external.path().join("second");
    std::fs::create_dir(&first).expect("first");
    std::fs::create_dir(&second).expect("second");
    let link = external.path().join("active");
    symlink(&first, &link).expect("link first");
    let requested = link.join("plan.md");
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_allowed_external_write_paths([requested.clone()]);

    std::fs::remove_file(&link).expect("remove link");
    symlink(&second, &link).expect("link second");

    assert_eq!(
        ctx.resolve_parent_for_write(&requested).expect("resolve"),
        first
            .canonicalize()
            .expect("canonical first")
            .join("plan.md")
    );
}

//! validation behavior (moved from `workspaces.rs`).

use super::super::*;
use super::*;
use std::fs;

#[test]
fn validation_rejects_directory_inside_primary_workspace() {
    let root = tempfile::tempdir().expect("root");
    let project = root.path().join("project");
    let nested = project.join("nested");
    fs::create_dir_all(&nested).expect("nested");

    let err = validate_new_workspace_entry(&project, &WorkspaceProject::default(), &nested)
        .expect_err("reject nested");

    assert!(err.to_string().contains("primary workspace"));
}

#[test]
fn validation_rejects_missing_path_with_clear_error() {
    let root = tempfile::tempdir().expect("root");
    let project = root.path().join("project");
    fs::create_dir_all(&project).expect("project");
    let missing = root.path().join("missing");

    let err = validate_new_workspace_entry(&project, &WorkspaceProject::default(), &missing)
        .expect_err("reject missing");

    assert!(err.to_string().contains("does not exist"));
}

#[test]
fn validation_rejects_file_path_with_clear_error() {
    let root = tempfile::tempdir().expect("root");
    let project = root.path().join("project");
    let file = root.path().join("file.txt");
    fs::create_dir_all(&project).expect("project");
    fs::write(&file, "not a directory").expect("file");

    let err = validate_new_workspace_entry(&project, &WorkspaceProject::default(), &file)
        .expect_err("reject file path");

    assert!(err.to_string().contains("not a directory"));
}

#[test]
fn validation_canonicalizes_symlink_directory() {
    let root = tempfile::tempdir().expect("root");
    let project = root.path().join("project");
    let target = root.path().join("target");
    let link = root.path().join("link");
    fs::create_dir_all(&project).expect("project");
    fs::create_dir_all(&target).expect("target");
    if !symlink_created(create_dir_symlink(&target, &link)) {
        return;
    }

    let entry = validate_new_workspace_entry(&project, &WorkspaceProject::default(), &link)
        .expect("symlink dir entry");

    assert_eq!(entry.path, target.canonicalize().expect("canonical target"));
}

//! store behavior (moved from `workspaces.rs`).

use super::super::*;
use super::*;
use std::fs;

#[test]
fn store_writes_project_entries_under_canonical_key() {
    let root = tempfile::tempdir().expect("root");
    let project = root.path().join("project");
    let added = root.path().join("added");
    fs::create_dir_all(&project).expect("project");
    fs::create_dir_all(&added).expect("added");
    let store = WorkspaceStore::new(root.path().join("workspaces.json"));
    let entry = WorkspaceEntry::read_only(added.canonicalize().expect("canonical added"));

    store
        .write_project(
            &project,
            WorkspaceProject {
                entries: vec![entry.clone()],
            },
        )
        .expect("write project");

    let loaded = store.read_project(&project).expect("read project");
    assert_eq!(loaded.entries, vec![entry]);
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn store_keys_non_utf8_project_paths_without_rejecting_them() {
    use std::os::unix::ffi::OsStringExt;

    let root = tempfile::tempdir().expect("root");
    let project_name = std::ffi::OsString::from_vec(b"project-\xFF".to_vec());
    let project = root.path().join(project_name);
    let added = root.path().join("added");
    fs::create_dir_all(&project).expect("project");
    fs::create_dir_all(&added).expect("added");
    let store = WorkspaceStore::new(root.path().join("workspaces.json"));
    let entry = WorkspaceEntry::read_only(added.canonicalize().expect("canonical added"));

    store
        .write_project(
            &project,
            WorkspaceProject {
                entries: vec![entry.clone()],
            },
        )
        .expect("write project");

    assert_eq!(
        store.read_project(&project).expect("read project").entries,
        vec![entry]
    );
}

#[test]
fn new_entry_defaults_to_enabled_read_only() {
    let root = tempfile::tempdir().expect("root");
    let project = root.path().join("project");
    let added = root.path().join("added");
    fs::create_dir_all(&project).expect("project");
    fs::create_dir_all(&added).expect("added");

    let entry = validate_new_workspace_entry(&project, &WorkspaceProject::default(), &added)
        .expect("entry");

    assert!(entry.enabled);
    assert!(entry.read);
    assert!(!entry.write);
}

#[test]
fn access_roots_skip_disabled_entries() {
    let root = tempfile::tempdir().expect("root");
    let added = root.path().join("added");
    fs::create_dir_all(&added).expect("added");
    let mut entry = WorkspaceEntry::read_only(added.canonicalize().expect("canonical added"));
    entry.enabled = false;

    let roots = access_roots_from_project(&WorkspaceProject {
        entries: vec![entry],
    });

    assert!(roots.is_empty());
}

#[test]
fn access_roots_skip_write_only_entries() {
    let root = tempfile::tempdir().expect("root");
    let added = root.path().join("added");
    fs::create_dir_all(&added).expect("added");
    let mut entry = WorkspaceEntry::read_only(added.canonicalize().expect("canonical added"));
    entry.read = false;
    entry.write = true;

    let roots = access_roots_from_project(&WorkspaceProject {
        entries: vec![entry],
    });

    assert!(roots.is_empty());
}

#[test]
fn access_roots_skip_relative_entries() {
    let roots = access_roots_from_project(&WorkspaceProject {
        entries: vec![WorkspaceEntry::read_only(PathBuf::from("relative"))],
    });

    assert!(roots.is_empty());
}

#[test]
fn access_roots_canonicalize_existing_dirs() {
    let root = tempfile::tempdir().expect("root");
    let added = root.path().join("added");
    fs::create_dir_all(&added).expect("added");
    let non_canonical = added.join("..").join("added");

    let roots = access_roots_from_project(&WorkspaceProject {
        entries: vec![WorkspaceEntry::read_only(non_canonical)],
    });

    assert_eq!(roots.len(), 1);
    assert_eq!(
        roots[0].path,
        added.canonicalize().expect("canonical added")
    );
}

#[test]
fn write_project_backs_up_corrupted_store_before_replacing_it() {
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("workspaces.json");
    fs::write(&path, "not json").expect("write corrupted");
    let project = root.path().join("project");
    fs::create_dir_all(&project).expect("project");
    let store = WorkspaceStore::new(path.clone());
    let added = root.path().join("added");
    fs::create_dir_all(&added).expect("added");
    let entry = WorkspaceEntry::read_only(added.canonicalize().expect("canonical added"));

    store
        .write_project(
            &project,
            WorkspaceProject {
                entries: vec![entry.clone()],
            },
        )
        .expect("replace corrupted");

    assert_eq!(
        store
            .read_project(&project)
            .expect("read after repair")
            .entries,
        vec![entry]
    );
    assert!(path.with_extension("json.bak").exists());
}

#[test]
fn read_project_does_not_mutate_corrupted_store() {
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("workspaces.json");
    fs::write(&path, "not json").expect("write corrupted");
    let project = root.path().join("project");
    fs::create_dir_all(&project).expect("project");
    let store = WorkspaceStore::new(path.clone());

    let loaded = store.read_project(&project).expect("read after corruption");

    assert!(loaded.entries.is_empty());
    assert!(path.exists());
    assert!(!path.with_extension("json.bak").exists());
}

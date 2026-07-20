use std::{
    ffi::OsStr,
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};

use uuid::Uuid;

pub(crate) enum AtomicWriteStatus {
    Durable,
    CommittedUnsynced(io::Error),
}

pub(crate) fn write_file_atomic(path: &Path, content: &[u8]) -> io::Result<()> {
    match write_file_atomic_status(path, content)? {
        AtomicWriteStatus::Durable => Ok(()),
        AtomicWriteStatus::CommittedUnsynced(error) => Err(error),
    }
}

pub(crate) fn write_file_atomic_create_new(
    path: &Path,
    content: &[u8],
) -> io::Result<AtomicWriteStatus> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no parent directory: {}", path.display()),
        )
    })?;
    ensure_safe_directory_tree(parent)?;
    reject_reparse_or_symlink_if_present(path)?;
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("session");
    let temp_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    if let Err(error) = write_temp_file(&temp_path, content) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    if let Err(error) = fs::hard_link(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    let _ = fs::remove_file(&temp_path);
    match sync_parent_dir(parent) {
        Ok(()) => Ok(AtomicWriteStatus::Durable),
        Err(error) => Ok(AtomicWriteStatus::CommittedUnsynced(error)),
    }
}

pub(crate) fn write_file_atomic_status(
    path: &Path,
    content: &[u8],
) -> io::Result<AtomicWriteStatus> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no parent directory: {}", path.display()),
        )
    })?;
    ensure_safe_directory_tree(parent)?;
    reject_reparse_or_symlink_if_present(path)?;
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("session");
    let temp_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    if let Err(error) = write_temp_file(&temp_path, content) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    match sync_parent_dir(parent) {
        Ok(()) => Ok(AtomicWriteStatus::Durable),
        Err(error) => Ok(AtomicWriteStatus::CommittedUnsynced(error)),
    }
}

/// Atomically replace an existing regular file without creating any path.
///
/// Edit uses this stricter variant so a target or parent disappearing after
/// preparation cannot be recreated. Existing permissions are retained.
pub(crate) fn replace_existing_file_atomic_status(
    path: &Path,
    content: &[u8],
) -> io::Result<AtomicWriteStatus> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no parent directory: {}", path.display()),
        )
    })?;
    validate_safe_directory(parent)?;
    let metadata = fs::symlink_metadata(path)?;
    if is_reparse_or_symlink(&metadata) || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to replace non-regular file {}", path.display()),
        ));
    }

    let file_name = path.file_name().and_then(OsStr::to_str).unwrap_or("edit");
    let temp_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    if let Err(error) = write_temp_file(&temp_path, content)
        .and_then(|()| preserve_replaced_file_permissions(&temp_path, &metadata))
    {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    let cleanup_error = match replace_existing_file(&temp_path, path) {
        Ok(cleanup_error) => cleanup_error,
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }
    };
    let sync_error = sync_parent_dir(parent).err();
    match (cleanup_error, sync_error) {
        (None, None) => Ok(AtomicWriteStatus::Durable),
        (Some(error), None) | (None, Some(error)) => {
            Ok(AtomicWriteStatus::CommittedUnsynced(error))
        }
        (Some(cleanup), Some(sync)) => Ok(AtomicWriteStatus::CommittedUnsynced(io::Error::other(
            format!(
                "replacement committed, but cleanup failed ({cleanup}) and the parent directory could not be synced ({sync})"
            ),
        ))),
    }
}

pub(crate) fn ensure_safe_directory_tree(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    validate_safe_directory(path)
}

/// Directories created by [`create_missing_directories_recording`], plus the
/// first error that stopped creation. `created` lists only directories this
/// call actually created, in outermost-to-innermost order, so a later failure
/// can report exact remaining side effects.
pub(crate) struct DirectoryCreation {
    pub(crate) created: Vec<PathBuf>,
    pub(crate) error: Option<io::Error>,
}

/// Create every missing directory from the nearest existing ancestor out to
/// `dir`, recording only the directories this call created.
///
/// Existing components are validated as safe directories; symlinks, reparse
/// points, and non-directories are rejected before any creation. Concurrent
/// creation of a level is tolerated but never recorded, because Neo did not
/// create it. Nothing is ever removed.
pub(crate) fn create_missing_directories_recording(dir: &Path) -> DirectoryCreation {
    let mut missing: Vec<PathBuf> = Vec::new();
    let mut current = dir.to_path_buf();
    loop {
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
                    return DirectoryCreation {
                        created: Vec::new(),
                        error: Some(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!(
                                "refusing to create files under non-directory ancestor {}",
                                current.display()
                            ),
                        )),
                    };
                }
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => match current.parent() {
                Some(parent) => {
                    missing.push(current.clone());
                    current = parent.to_path_buf();
                }
                None => {
                    return DirectoryCreation {
                        created: Vec::new(),
                        error: Some(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("no existing ancestor for {}", dir.display()),
                        )),
                    };
                }
            },
            Err(error) => {
                return DirectoryCreation {
                    created: Vec::new(),
                    error: Some(error),
                };
            }
        }
    }

    let mut created = Vec::new();
    for path in missing.iter().rev() {
        match fs::create_dir(path) {
            Ok(()) => created.push(path.clone()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if let Err(validation) = validate_safe_directory(path) {
                    return DirectoryCreation {
                        created,
                        error: Some(validation),
                    };
                }
            }
            Err(error) => {
                return DirectoryCreation {
                    created,
                    error: Some(error),
                };
            }
        }
    }

    DirectoryCreation {
        created,
        error: None,
    }
}

pub(crate) fn validate_safe_directory_if_present(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_safe_directory(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn validate_safe_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if is_reparse_or_symlink(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing symlinked directory {}", path.display()),
        ));
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is not a directory: {}", path.display()),
        ));
    }
    Ok(())
}

pub(crate) fn reject_reparse_or_symlink_if_present(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if is_reparse_or_symlink(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing symlinked file {}", path.display()),
        ));
    }
    Ok(())
}

fn write_temp_file(path: &Path, content: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(content)?;
    file.sync_all()
}

#[cfg(not(windows))]
fn preserve_replaced_file_permissions(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    fs::set_permissions(path, metadata.permissions())
}

#[cfg(windows)]
fn preserve_replaced_file_permissions(_path: &Path, _metadata: &fs::Metadata) -> io::Result<()> {
    // ReplaceFileW preserves the replaced file's attributes and ACL.
    Ok(())
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn replace_existing_file(replacement: &Path, replaced: &Path) -> io::Result<Option<io::Error>> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, replacement, CWD, replaced, RenameFlags::EXCHANGE)?;
    Ok(fs::remove_file(replacement).err().map(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "replacement committed, but failed to remove exchanged original {}: {error}",
                replacement.display()
            ),
        )
    }))
}

#[cfg(windows)]
fn replace_existing_file(replacement: &Path, replaced: &Path) -> io::Result<Option<io::Error>> {
    let replacement = replacement.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("replacement path is not Unicode: {}", replacement.display()),
        )
    })?;
    let replaced = replaced.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("target path is not Unicode: {}", replaced.display()),
        )
    })?;
    winsafe::ReplaceFile(
        replaced,
        replacement,
        None,
        winsafe::co::REPLACEFILE::WRITE_THROUGH,
    )
    .map_err(|code| io::Error::from_raw_os_error(code.raw() as i32))?;
    Ok(None)
}

#[cfg(not(any(
    windows,
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android"
)))]
fn replace_existing_file(replacement: &Path, replaced: &Path) -> io::Result<Option<io::Error>> {
    // These targets expose no safe exchange primitive through rustix. The
    // pre-replace existence check still rejects ordinary disappearance.
    fs::rename(replacement, replaced)?;
    Ok(None)
}

#[cfg(unix)]
fn sync_parent_dir(parent: &Path) -> io::Result<()> {
    fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_dir(_parent: &Path) -> io::Result<()> {
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    sync_parent_dir(path)
}

fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink() || platform_reparse_point(metadata)
}

#[cfg(windows)]
fn platform_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn platform_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_never_creates_missing_paths() {
        let workspace = tempfile::tempdir().expect("workspace");
        let parent = workspace.path().join("src");
        fs::create_dir(&parent).expect("create parent");
        let target = parent.join("lib.rs");

        let missing_target = replace_existing_file_atomic_status(&target, b"new");
        assert!(missing_target.is_err());
        assert!(!target.exists());

        fs::remove_dir(&parent).expect("remove parent");
        let missing_parent = replace_existing_file_atomic_status(&target, b"new");
        assert!(missing_parent.is_err());
        assert!(!parent.exists());
    }

    #[cfg(unix)]
    #[test]
    fn replacement_preserves_unix_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let workspace = tempfile::tempdir().expect("workspace");
        let target = workspace.path().join("script.sh");
        fs::write(&target, b"old").expect("seed file");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o751)).expect("set mode");

        let result = replace_existing_file_atomic_status(&target, b"new");

        assert!(result.is_ok());
        assert_eq!(fs::read(&target).expect("read file"), b"new");
        assert_eq!(
            fs::metadata(&target)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o751
        );
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
    #[test]
    fn replacement_swap_fails_if_target_disappears() {
        let workspace = tempfile::tempdir().expect("workspace");
        let target = workspace.path().join("target.txt");
        let replacement = workspace.path().join("replacement.txt");
        fs::write(&target, b"old").expect("target");
        fs::write(&replacement, b"new").expect("replacement");
        fs::remove_file(&target).expect("remove target");

        let result = replace_existing_file(&replacement, &target);

        assert!(result.is_err());
        assert!(!target.exists());
        assert_eq!(fs::read(replacement).expect("replacement remains"), b"new");
    }
}

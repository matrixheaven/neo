use std::{ffi::OsStr, fs, io, io::Write, path::Path};

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

pub(crate) fn ensure_safe_directory_tree(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    validate_safe_directory(path)
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
            format!("refusing symlinked session directory {}", path.display()),
        ));
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("session path is not a directory: {}", path.display()),
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
            format!("refusing symlinked session file {}", path.display()),
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

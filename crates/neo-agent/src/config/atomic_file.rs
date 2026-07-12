use std::{fs::File, io::Write, path::Path};

use anyhow::Context as _;

pub(crate) fn write_with(
    path: &Path,
    content: &[u8],
    writer: impl FnOnce(&mut File, &[u8]) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "failed to create temporary config beside {}",
            path.display()
        )
    })?;
    writer(temporary.as_file_mut(), content)?;
    temporary.as_file_mut().flush().with_context(|| {
        format!(
            "failed to flush temporary config {}",
            temporary.path().display()
        )
    })?;
    temporary.as_file().sync_all().with_context(|| {
        format!(
            "failed to sync temporary config {}",
            temporary.path().display()
        )
    })?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to atomically replace config {}", path.display()))?;
    #[cfg(unix)]
    sync_parent(parent)?;
    // On Windows, tempfile's MoveFileExW replacement is atomic, but Rust has
    // no safe API for flushing a directory handle after the replacement.
    Ok(())
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> anyhow::Result<()> {
    File::open(parent)
        .with_context(|| format!("failed to open config directory {}", parent.display()))?
        .sync_all()
        .with_context(|| format!("failed to sync config directory {}", parent.display()))
}

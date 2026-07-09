use std::path::Path;

use anyhow::Context as _;
use sha2::{Digest, Sha256};

pub(crate) fn project_key(project_dir: &Path) -> anyhow::Result<String> {
    let canonical = project_dir.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize project dir {}",
            project_dir.display()
        )
    })?;
    Ok(format!("project_{}", path_hash_hex(&canonical)))
}

fn path_hash_hex(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hash_path_bytes(path, &mut hasher);
    format!("{:x}", hasher.finalize())
}

#[cfg(unix)]
fn hash_path_bytes(path: &Path, hasher: &mut Sha256) {
    use std::os::unix::ffi::OsStrExt as _;

    hasher.update(path.as_os_str().as_bytes());
}

#[cfg(windows)]
fn hash_path_bytes(path: &Path, hasher: &mut Sha256) {
    use std::os::windows::ffi::OsStrExt as _;

    for unit in path.as_os_str().encode_wide() {
        hasher.update(unit.to_le_bytes());
    }
}

#[cfg(not(any(unix, windows)))]
fn hash_path_bytes(path: &Path, hasher: &mut Sha256) {
    hasher.update(path.to_string_lossy().as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn path_hash_accepts_non_utf8_os_paths() {
        use std::os::unix::ffi::OsStringExt as _;

        let raw = std::ffi::OsString::from_vec(b"/tmp/project-\xFF".to_vec());
        let path = Path::new(&raw);

        assert_eq!(path_hash_hex(path).len(), 64);
    }
}

use std::path::Path;

use anyhow::Context as _;
use neo_agent_core::session::hash_os_path_into;
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
    hash_os_path_into(path, &mut hasher);
    format!("{:x}", hasher.finalize())
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

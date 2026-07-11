//! Workspace key encoding for workspace-scoped session storage.
//!
//! Each workspace (project directory) gets a deterministic, filesystem-safe
//! bucket name derived from its absolute path. Sessions are stored inside
//! the bucket directory so that `/resume` only sees sessions from the
//! current workspace.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const WORKDIR_KEY_PREFIX: &str = "wd_";
const HASH_LENGTH: usize = 12;

/// Resolve an absolute path, following symlinks. Falls back to the
/// input path if canonicalization fails (e.g. the path does not exist yet).
#[must_use]
pub fn normalize_workdir(workdir: &Path) -> PathBuf {
    std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf())
}

/// Feed a path's native OS representation into a SHA-256 digest.
pub fn hash_os_path_into(path: &Path, hasher: &mut Sha256) {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;

        hasher.update(path.as_os_str().as_bytes());
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;

        hash_wide_units_into(path.as_os_str().encode_wide(), hasher);
    }

    #[cfg(not(any(unix, windows)))]
    hasher.update(path.as_os_str().as_encoded_bytes());
}

#[cfg(windows)]
fn hash_wide_units_into(units: impl IntoIterator<Item = u16>, hasher: &mut Sha256) {
    for unit in units {
        hasher.update(unit.to_le_bytes());
    }
}

/// Convert a directory's `basename` into a filesystem-safe slug:
/// lowercase, non-`[a-z0-9._-]` → `-`, trimmed, max 40 chars.
/// Returns `"workspace"` if the result would be empty.
#[must_use]
pub fn slugify_basename(workdir: &Path) -> String {
    let base = workdir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace");
    let slug: String = base
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "workspace".to_string()
    } else {
        slug.chars().take(40).collect()
    }
}

/// Generate a workspace key: `wd_<slug>_<hash12>`.
///
/// The slug makes the directory human-readable; the SHA-256 hash of the
/// full absolute path guarantees uniqueness — two projects with the same
/// basename in different parent directories get different keys.
#[must_use]
pub fn encode_workdir_key(workdir: &Path) -> String {
    let normalized = normalize_workdir(workdir);
    let slug = slugify_basename(&normalized);
    let mut hasher = Sha256::new();
    hash_os_path_into(&normalized, &mut hasher);
    let hash = hasher.finalize();
    let hash_hex: String =
        hash.iter()
            .take(HASH_LENGTH.div_ceil(2))
            .fold(String::new(), |mut acc, byte| {
                use std::fmt::Write;
                let _ = write!(acc, "{byte:02x}");
                acc
            });
    // Take exactly HASH_LENGTH hex characters.
    let hash_hex = hash_hex.chars().take(HASH_LENGTH).collect::<String>();
    format!("{WORKDIR_KEY_PREFIX}{slug}_{hash_hex}")
}

/// Compute the workspace-scoped sessions directory by appending the
/// workspace key to the given sessions root.
#[must_use]
pub fn workspace_sessions_dir(sessions_root: &Path, workdir: &Path) -> PathBuf {
    sessions_root.join(encode_workdir_key(workdir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify_basename(Path::new("/home/user/neo")), "neo");
        assert_eq!(
            slugify_basename(Path::new("/home/user/My Project")),
            "my-project"
        );
    }

    #[test]
    fn slugify_empty_fallback() {
        // A path that is just "/" → basename is empty → "workspace".
        assert_eq!(slugify_basename(Path::new("/")), "workspace");
    }

    #[test]
    fn slugify_special_chars() {
        assert_eq!(
            slugify_basename(Path::new("/x/hello@world!")),
            "hello-world"
        );
    }

    #[test]
    fn encode_key_format() {
        let key = encode_workdir_key(Path::new("/home/user/neo"));
        assert!(key.starts_with("wd_neo_"));
        let hash_part = key.strip_prefix("wd_neo_").unwrap();
        assert_eq!(hash_part.len(), 12);
        assert!(hash_part.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn encode_key_deterministic() {
        let key1 = encode_workdir_key(Path::new("/home/user/neo"));
        let key2 = encode_workdir_key(Path::new("/home/user/neo"));
        assert_eq!(key1, key2);
    }

    #[test]
    fn encode_key_different_paths_same_basename() {
        let key1 = encode_workdir_key(Path::new("/home/user1/neo"));
        let key2 = encode_workdir_key(Path::new("/home/user2/neo"));
        assert_ne!(key1, key2);
    }

    #[cfg(unix)]
    #[test]
    fn workspace_keys_distinguish_different_invalid_utf8_paths() {
        use std::os::unix::ffi::OsStringExt as _;

        let a = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/work-\xFE".to_vec()));
        let b = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/work-\xFF".to_vec()));

        assert_ne!(encode_workdir_key(&a), encode_workdir_key(&b));
    }

    #[cfg(windows)]
    #[test]
    fn wide_path_hash_distinguishes_unpaired_surrogates() {
        let mut a = Sha256::new();
        hash_wide_units_into([0xD800], &mut a);
        let mut b = Sha256::new();
        hash_wide_units_into([0xD801], &mut b);

        assert_ne!(a.finalize(), b.finalize());
    }

    #[test]
    fn workspace_sessions_dir_appends_key() {
        let root = Path::new("/home/user/.neo/sessions");
        let bucket = workspace_sessions_dir(root, Path::new("/home/user/neo"));
        assert!(bucket.starts_with(root));
        assert!(
            bucket
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap()
                .starts_with("wd_neo_")
        );
    }
}

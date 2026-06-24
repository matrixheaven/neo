//! Persistent OAuth token storage.
//!
//! Tokens are stored as plain JSON in a caller-supplied path (the conventional
//! location is `~/.neo/oauth.json`). The file is created with user-only
//! permissions on Unix (`0o600`).

use super::{OAuthError, OAuthTokenSet};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// On-disk layout for the OAuth token store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredStore {
    entries: BTreeMap<String, OAuthTokenSet>,
}

/// Persistent store for OAuth token sets, keyed by an arbitrary string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OAuthStore {
    pub entries: BTreeMap<String, OAuthTokenSet>,
}

impl OAuthStore {
    /// Load the store from `path`.
    ///
    /// Returns an empty store if the file does not exist. Returns an error if
    /// the file exists but cannot be read or parsed.
    ///
    /// # Errors
    ///
    /// Returns `OAuthError::StoreLoad` for I/O errors other than a missing file,
    /// and `OAuthError::StoreParse` for invalid JSON.
    pub fn load(path: &Path) -> Result<Self, OAuthError> {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let stored: StoredStore =
                    serde_json::from_str(&contents).map_err(OAuthError::StoreParse)?;
                Ok(Self {
                    entries: stored.entries,
                })
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(OAuthError::StoreLoad(err)),
        }
    }

    /// Save the store to `path`.
    ///
    /// Creates parent directories as needed. On Unix the file is written with
    /// mode `0o600`; on other platforms the default permissions are used.
    ///
    /// # Errors
    ///
    /// Returns `OAuthError::StoreSave` if the file cannot be written, and
    /// `OAuthError::StoreParse` if the store cannot be serialized to JSON.
    pub fn save(&self, path: &Path) -> Result<(), OAuthError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(OAuthError::StoreSave)?;
        }

        let stored = StoredStore {
            entries: self.entries.clone(),
        };
        let json = serde_json::to_string_pretty(&stored).map_err(OAuthError::StoreParse)?;

        write_store_file(path, &json)?;

        Ok(())
    }

    /// Returns a reference to the token set for `key`, if any.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&OAuthTokenSet> {
        self.entries.get(key)
    }

    /// Inserts or replaces the token set for `key`.
    pub fn set(&mut self, key: &str, token_set: OAuthTokenSet) {
        self.entries.insert(key.to_string(), token_set);
    }

    /// Removes the token set for `key`.
    ///
    /// Returns `true` if a value was present and removed.
    pub fn remove(&mut self, key: &str) -> bool {
        self.entries.remove(key).is_some()
    }
}

#[cfg(unix)]
fn write_store_file(path: &Path, contents: &str) -> Result<(), OAuthError> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(OAuthError::StoreSave)?;

    file.write_all(contents.as_bytes())
        .map_err(OAuthError::StoreSave)?;
    file.flush().map_err(OAuthError::StoreSave)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_store_file(path: &Path, contents: &str) -> Result<(), OAuthError> {
    std::fs::write(path, contents).map_err(OAuthError::StoreSave)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use std::time::SystemTime;

    fn sample_token_set() -> OAuthTokenSet {
        OAuthTokenSet {
            access_token: "access-123".to_string(),
            token_type: "Bearer".to_string(),
            refresh_token: Some("refresh-456".to_string()),
            expires_at: Some(DateTime::from(
                SystemTime::UNIX_EPOCH + std::time::Duration::new(1_700_000_000, 0),
            )),
            scopes: vec!["read".to_string(), "write".to_string()],
        }
    }

    #[test]
    fn load_missing_file_returns_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.json");

        let store = OAuthStore::load(&path).unwrap();
        assert!(store.entries.is_empty());
    }

    #[test]
    fn load_malformed_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth.json");
        std::fs::write(&path, "not json").unwrap();

        let result = OAuthStore::load(&path);
        assert!(matches!(result, Err(OAuthError::StoreParse(_))));
    }

    #[test]
    fn save_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("oauth.json");

        let mut store = OAuthStore::default();
        store.set("linear", sample_token_set());
        store.save(&path).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth.json");

        let mut store = OAuthStore::default();
        store.set("linear", sample_token_set());
        store.save(&path).unwrap();

        let loaded = OAuthStore::load(&path).unwrap();
        assert_eq!(loaded, store);
    }

    #[test]
    fn get_set_remove_operations() {
        let mut store = OAuthStore::default();
        let tokens = sample_token_set();

        assert!(store.get("linear").is_none());

        store.set("linear", tokens.clone());
        assert_eq!(store.get("linear"), Some(&tokens));

        assert!(store.remove("linear"));
        assert!(store.get("linear").is_none());
        assert!(!store.remove("linear"));
    }

    #[test]
    fn remove_returns_false_when_key_missing() {
        let mut store = OAuthStore::default();
        assert!(!store.remove("nonexistent"));
    }

    #[test]
    #[cfg(unix)]
    fn save_creates_file_with_user_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth.json");

        let mut store = OAuthStore::default();
        store.set("linear", sample_token_set());
        store.save(&path).unwrap();

        let metadata = std::fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn overwrite_existing_entry() {
        let mut store = OAuthStore::default();
        store.set("linear", sample_token_set());

        let mut second = sample_token_set();
        second.access_token = "access-789".to_string();
        store.set("linear", second.clone());

        assert_eq!(store.get("linear"), Some(&second));
        assert_eq!(store.entries.len(), 1);
    }
}

//! Persistent OAuth token storage.
//!
//! Tokens are stored as plain JSON in a caller-supplied path (the conventional
//! location is `~/.neo/oauth.json`). The file is created with user-only
//! permissions on Unix (`0o600`).
//!
//! The on-disk format stores [`rmcp::transport::auth::StoredCredentials`] under
//! server keys such as `mcp:<server_id>`.

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{BufReader, BufWriter, Write},
    path::Path,
};

use rmcp::transport::auth::StoredCredentials;
use serde::{Deserialize, Serialize};

use super::OAuthError;

/// On-disk layout for the OAuth token store.
// `PartialEq`/`Eq` are not derived because `rmcp::transport::auth::StoredCredentials`
// does not implement those traits.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OAuthStore {
    pub entries: BTreeMap<String, StoredCredentials>,
}

impl OAuthStore {
    /// Load the store from `path`.
    ///
    /// Returns an empty store if the file does not exist.
    ///
    /// # Errors
    ///
    /// Returns `OAuthError::StoreLoad` for I/O errors other than a missing file,
    /// and `OAuthError::StoreParse` for malformed JSON or non-canonical store
    /// shape.
    pub fn load(path: &Path) -> Result<Self, OAuthError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(OAuthError::StoreLoad)?;
        let reader = BufReader::new(file);
        serde_json::from_reader(reader).map_err(|err| OAuthError::StoreParse(err.to_string()))
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
            fs::create_dir_all(parent).map_err(OAuthError::StoreSave)?;
        }

        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)
                .map_err(OAuthError::StoreSave)?
        };

        #[cfg(not(unix))]
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(OAuthError::StoreSave)?;

        let writer = BufWriter::new(&file);
        serde_json::to_writer_pretty(writer, self)
            .map_err(|err| OAuthError::StoreParse(err.to_string()))?;
        file.flush().map_err(OAuthError::StoreSave)?;
        Ok(())
    }

    /// Returns a reference to the stored credentials for `key`, if any.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&StoredCredentials> {
        self.entries.get(key)
    }

    /// Inserts or replaces the stored credentials for `key`.
    pub fn set(&mut self, key: &str, credentials: StoredCredentials) {
        self.entries.insert(key.to_string(), credentials);
    }

    /// Removes the stored credentials for `key`.
    ///
    /// Returns `true` if a value was present and removed.
    #[must_use]
    pub fn remove(&mut self, key: &str) -> bool {
        self.entries.remove(key).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rmcp::transport::auth::OAuthTokenResponse;
    use serde_json::{Value, json};

    fn sample_token_response() -> OAuthTokenResponse {
        let mut value = serde_json::Map::new();
        value.insert("access_token".to_owned(), json!("access-123"));
        value.insert("token_type".to_owned(), json!("Bearer"));
        value.insert("refresh_token".to_owned(), json!("refresh-456"));
        value.insert("expires_in".to_owned(), json!(3600u64));
        value.insert("scope".to_owned(), json!("read write"));
        serde_json::from_value(Value::Object(value)).unwrap()
    }

    fn sample_credentials() -> StoredCredentials {
        StoredCredentials::new(
            String::new(),
            Some(sample_token_response()),
            vec!["read".to_owned(), "write".to_owned()],
            Some(u64::try_from(Utc::now().timestamp()).unwrap_or(0)),
        )
    }

    fn access_token(credentials: &StoredCredentials) -> Option<String> {
        let value = serde_json::to_value(credentials.token_response.as_ref()?).ok()?;
        value
            .get("access_token")
            .and_then(|v| v.as_str())
            .map(String::from)
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
        store.set("linear", sample_credentials());
        store.save(&path).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth.json");

        let mut store = OAuthStore::default();
        store.set("linear", sample_credentials());
        store.save(&path).unwrap();

        let loaded = OAuthStore::load(&path).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert!(loaded.get("linear").is_some());
        assert_eq!(
            access_token(loaded.get("linear").unwrap()),
            Some("access-123".to_owned())
        );
    }

    #[test]
    fn get_set_remove_operations() {
        let mut store = OAuthStore::default();
        let credentials = sample_credentials();

        assert!(store.get("linear").is_none());

        store.set("linear", credentials.clone());
        assert!(store.get("linear").is_some());
        assert_eq!(
            access_token(store.get("linear").unwrap()),
            Some("access-123".to_owned())
        );

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
        store.set("linear", sample_credentials());
        store.save(&path).unwrap();

        let metadata = std::fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn overwrite_existing_entry() {
        let mut store = OAuthStore::default();
        store.set("linear", sample_credentials());

        let mut second = sample_credentials();
        if let Some(ref mut token_response) = second.token_response {
            let mut value = serde_json::to_value(&*token_response).unwrap();
            value["access_token"] = json!("access-789");
            *token_response = serde_json::from_value(value).unwrap();
        }
        store.set("linear", second.clone());

        assert_eq!(
            access_token(store.get("linear").unwrap()),
            Some("access-789".to_owned())
        );
        assert_eq!(store.entries.len(), 1);
    }

    #[test]
    fn load_rejects_oauth_token_set_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth.json");

        std::fs::write(
            &path,
            r#"{
  "entries": {
    "mcp:linear": {
      "access_token": "access-123",
      "token_type": "Bearer",
      "refresh_token": "refresh-456",
      "expires_at": "2023-11-14T22:13:20Z",
      "scopes": ["read", "write"]
    }
  }
}"#,
        )
        .unwrap();

        let result = OAuthStore::load(&path);
        assert!(matches!(result, Err(OAuthError::StoreParse(_))));
    }
}

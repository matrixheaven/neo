//! Persistent OAuth token storage.
//!
//! Tokens are stored as plain JSON in a caller-supplied path (the conventional
//! location is `~/.neo/oauth.json`). The file is created with user-only
//! permissions on Unix (`0o600`).
//!
//! The on-disk format stores [`rmcp::transport::auth::StoredCredentials`] under
//! server keys such as `mcp:<server_id>`. Legacy stores that used Neo's own
//! [`OAuthTokenSet`] layout are transparently migrated on first load.

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{BufReader, BufWriter, Write},
    path::Path,
};

use chrono::Utc;
use rmcp::transport::auth::{OAuthTokenResponse, StoredCredentials};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{OAuthError, OAuthTokenSet};

/// On-disk layout for the OAuth token store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OAuthStore {
    pub entries: BTreeMap<String, StoredCredentials>,
}

/// Legacy on-disk layout, kept for one-time migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyOAuthStore {
    entries: BTreeMap<String, OAuthTokenSet>,
}

impl OAuthStore {
    /// Load the store from `path`.
    ///
    /// Returns an empty store if the file does not exist. If the file uses the
    /// legacy Neo `OAuthTokenSet` format, it is migrated in-memory to the new
    /// `StoredCredentials` format. The migrated data is not written back until
    /// [`Self::save`] is called.
    ///
    /// # Errors
    ///
    /// Returns `OAuthError::StoreLoad` for I/O errors other than a missing file,
    /// and `OAuthError::StoreParse` for JSON that is neither the current nor the
    /// legacy format.
    pub fn load(path: &Path) -> Result<Self, OAuthError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(OAuthError::StoreLoad)?;
        let reader = BufReader::new(file);
        match serde_json::from_reader(reader) {
            Ok(store) => Ok(store),
            Err(_) => Self::migrate_legacy(path),
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
            fs::create_dir_all(parent).map_err(OAuthError::StoreSave)?;
        }

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(OAuthError::StoreSave)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }

        let writer = BufWriter::new(&file);
        serde_json::to_writer_pretty(writer, self).map_err(OAuthError::StoreParse)?;
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

    /// Compatibility accessor that returns the legacy [`OAuthTokenSet`] view of
    /// the credentials stored under `key`, if any.
    #[must_use]
    pub fn get_token(&self, key: &str) -> Option<OAuthTokenSet> {
        self.get(key).and_then(token_set_from_credentials)
    }

    /// Compatibility setter that stores an [`OAuthTokenSet`] as
    /// [`StoredCredentials`] under `key`.
    pub fn set_token(&mut self, key: &str, token_set: &OAuthTokenSet) {
        if let Ok(credentials) = credentials_from_token_set(token_set) {
            self.set(key, credentials);
        }
    }

    /// Compatibility remover for the credentials stored under `key`.
    ///
    /// Returns `true` if a value was present and removed.
    #[must_use]
    pub fn remove_token(&mut self, key: &str) -> bool {
        self.remove(key)
    }

    /// Migrate a legacy store at `path` to the current format.
    fn migrate_legacy(path: &Path) -> Result<Self, OAuthError> {
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(OAuthError::StoreLoad)?;
        let legacy: LegacyOAuthStore =
            serde_json::from_reader(BufReader::new(file)).map_err(OAuthError::StoreParse)?;
        let mut store = Self::default();
        for (key, token_set) in legacy.entries {
            store.set(&key, credentials_from_token_set(&token_set)?);
        }
        Ok(store)
    }
}

/// Convert a Neo [`OAuthTokenSet`] into an rmcp [`StoredCredentials`].
fn credentials_from_token_set(token_set: &OAuthTokenSet) -> Result<StoredCredentials, OAuthError> {
    let expires_in = token_set.expires_at.map(|expires_at| {
        let seconds = (expires_at - Utc::now()).num_seconds();
        u64::try_from(seconds).unwrap_or(0)
    });

    let mut value = serde_json::Map::new();
    value.insert("access_token".to_owned(), json!(token_set.access_token));
    value.insert("token_type".to_owned(), json!(token_set.token_type));
    if let Some(ref refresh_token) = token_set.refresh_token {
        value.insert("refresh_token".to_owned(), json!(refresh_token));
    }
    if let Some(secs) = expires_in {
        value.insert("expires_in".to_owned(), json!(secs));
    }
    if !token_set.scopes.is_empty() {
        value.insert("scope".to_owned(), json!(token_set.scopes.join(" ")));
    }

    let token_response: OAuthTokenResponse =
        serde_json::from_value(Value::Object(value)).map_err(OAuthError::StoreParse)?;

    // Preserve the original expiration moment by backdating the receive time.
    let token_received_at = if let Some((expires_at, secs)) = token_set.expires_at.zip(expires_in) {
        let secs_i64 = i64::try_from(secs).unwrap_or(i64::MAX);
        u64::try_from((expires_at - chrono::Duration::seconds(secs_i64)).timestamp()).unwrap_or(0)
    } else {
        u64::try_from(Utc::now().timestamp()).unwrap_or(0)
    };

    Ok(StoredCredentials::new(
        String::new(),
        Some(token_response),
        token_set.scopes.clone(),
        Some(token_received_at),
    ))
}

/// Convert rmcp [`StoredCredentials`] back into a Neo [`OAuthTokenSet`].
fn token_set_from_credentials(credentials: &StoredCredentials) -> Option<OAuthTokenSet> {
    let token_response = credentials.token_response.as_ref()?;
    let value = serde_json::to_value(token_response).ok()?;

    let access_token = value
        .get("access_token")
        .and_then(|v| v.as_str())?
        .to_owned();
    let token_type = value
        .get("token_type")
        .and_then(|v| v.as_str())
        .unwrap_or("Bearer")
        .to_owned();
    let refresh_token = value
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(String::from);
    let expires_in = value.get("expires_in").and_then(serde_json::Value::as_u64);
    let expires_at = credentials
        .token_received_at
        .zip(expires_in)
        .and_then(|(received, secs)| {
            let timestamp = i64::try_from(received + secs).unwrap_or(i64::MAX);
            chrono::DateTime::from_timestamp(timestamp, 0)
        });
    let scopes = if let Some(scope) = value.get("scope").and_then(|v| v.as_str()) {
        scope.split_whitespace().map(String::from).collect()
    } else {
        credentials.granted_scopes.clone()
    };

    Some(OAuthTokenSet {
        access_token,
        token_type,
        refresh_token,
        expires_at,
        scopes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use std::time::SystemTime;

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
            Some(Utc::now().timestamp() as u64),
        )
    }

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
    fn load_legacy_store_migrates_to_stored_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth.json");

        let legacy = LegacyOAuthStore {
            entries: BTreeMap::from([("mcp:linear".to_owned(), sample_token_set())]),
        };
        std::fs::write(&path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        let store = OAuthStore::load(&path).unwrap();
        let credentials = store.get("mcp:linear").expect("migrated entry");
        assert_eq!(access_token(credentials), Some("access-123".to_owned()));
        assert_eq!(credentials.granted_scopes, vec!["read", "write"]);
    }

    #[test]
    fn compatibility_get_token_returns_oauth_token_set() {
        let mut store = OAuthStore::default();
        store.set_token("linear", &sample_token_set());

        let token = store.get_token("linear").unwrap();
        assert_eq!(token.access_token, "access-123");
        assert_eq!(token.token_type, "bearer");
        assert_eq!(token.refresh_token, Some("refresh-456".to_owned()));
        assert_eq!(token.scopes, vec!["read", "write"]);
    }
}

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(windows)]
use winapi::{
    shared::ntdef::HANDLE,
    um::{
        winbase::FILE_FLAG_BACKUP_SEMANTICS,
        winnt::{
            FILE_ALL_ACCESS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, PSID,
            READ_CONTROL, WRITE_DAC,
        },
    },
};
#[cfg(windows)]
use windows_acl::{
    acl::{ACL, AceType},
    helper::{current_user, name_to_sid, string_to_sid},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{McpOAuthError, McpOAuthIdentity};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpOAuthClientRecord {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uris: Vec<String>,
    pub token_endpoint_auth_method: Option<String>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOAuthTokenRecord {
    pub access_token: String,
    pub token_type: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub token_received_at: u64,
    pub granted_scopes: Vec<String>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpOAuthDiscoveryRecord {
    pub authorization_server_metadata: serde_json::Value,
    pub discovered_at: String,
}

#[derive(Debug, Clone)]
pub struct McpOAuthStore {
    root: PathBuf,
}

impl McpOAuthStore {
    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn server_dir(&self, identity: &McpOAuthIdentity) -> PathBuf {
        self.root.join(&identity.store_key)
    }

    pub fn load_client(
        &self,
        identity: &McpOAuthIdentity,
    ) -> io::Result<Option<McpOAuthClientRecord>> {
        read_optional_json(&self.server_dir(identity).join("client.json"))
    }

    pub fn save_client(
        &self,
        identity: &McpOAuthIdentity,
        client: &McpOAuthClientRecord,
    ) -> Result<(), McpOAuthError> {
        self.prepare_server_dir(identity)?;
        write_json_atomic(&self.server_dir(identity).join("client.json"), client)
    }

    pub fn load_tokens(
        &self,
        identity: &McpOAuthIdentity,
    ) -> io::Result<Option<McpOAuthTokenRecord>> {
        read_optional_json(&self.server_dir(identity).join("tokens.json"))
    }

    pub fn save_tokens(
        &self,
        identity: &McpOAuthIdentity,
        tokens: &McpOAuthTokenRecord,
    ) -> Result<(), McpOAuthError> {
        self.prepare_server_dir(identity)?;
        write_json_atomic(&self.server_dir(identity).join("tokens.json"), tokens)
    }

    pub fn clear_tokens(&self, identity: &McpOAuthIdentity) -> Result<(), McpOAuthError> {
        remove_optional(&self.server_dir(identity).join("tokens.json"))
    }

    pub fn load_discovery(
        &self,
        identity: &McpOAuthIdentity,
    ) -> io::Result<Option<McpOAuthDiscoveryRecord>> {
        read_optional_json(&self.server_dir(identity).join("discovery.json"))
    }

    pub fn save_discovery(
        &self,
        identity: &McpOAuthIdentity,
        discovery: &McpOAuthDiscoveryRecord,
    ) -> Result<(), McpOAuthError> {
        self.prepare_server_dir(identity)?;
        write_json_atomic(&self.server_dir(identity).join("discovery.json"), discovery)
    }

    fn prepare_server_dir(&self, identity: &McpOAuthIdentity) -> Result<(), McpOAuthError> {
        create_private_dir_chain(&self.root)?;
        create_private_dir_chain(&self.server_dir(identity))
    }
}

fn read_optional_json<T>(path: &Path) -> io::Result<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to parse {}: {err}", path.display()),
            )
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn write_json_atomic<T>(path: &Path, value: &T) -> Result<(), McpOAuthError>
where
    T: Serialize,
{
    let parent = path.parent().ok_or_else(|| {
        store_error(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no parent directory: {}", path.display()),
        ))
    })?;
    create_private_dir_chain(parent)?;

    let temp_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("oauth"),
        Uuid::new_v4()
    ));

    let write_result = write_temp_file(&temp_path, value).and_then(|()| {
        fs::rename(&temp_path, path)
            .map_err(|err| {
                store_error(io::Error::other(format!(
                    "failed to rename {} to {}: {err}",
                    temp_path.display(),
                    path.display()
                )))
            })
            .and_then(|()| sync_parent_dir(parent))
    });

    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }

    write_result
}

fn create_private_dir_chain(path: &Path) -> Result<(), McpOAuthError> {
    if path.exists() {
        chmod_dir_private(path)?;
        return Ok(());
    }

    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        create_private_dir_chain(parent)?;
    }

    match fs::create_dir(path) {
        Ok(()) => chmod_dir_private(path),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => chmod_dir_private(path),
        Err(err) => Err(store_error(io::Error::other(format!(
            "failed to create {}: {err}",
            path.display()
        )))),
    }
}

#[cfg(unix)]
fn sync_parent_dir(parent: &Path) -> Result<(), McpOAuthError> {
    OpenOptions::new()
        .read(true)
        .open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|err| {
            store_error(io::Error::other(format!(
                "failed to sync parent directory {}: {err}",
                parent.display()
            )))
        })
}

#[cfg(not(unix))]
fn sync_parent_dir(_parent: &Path) -> Result<(), McpOAuthError> {
    Ok(())
}

fn write_temp_file<T>(temp_path: &Path, value: &T) -> Result<(), McpOAuthError>
where
    T: Serialize,
{
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp_path)
        .map_err(|err| {
            store_error(io::Error::other(format!(
                "failed to create {}: {err}",
                temp_path.display()
            )))
        })?;
    chmod_file_private(temp_path)?;

    serde_json::to_writer_pretty(&mut file, value).map_err(|err| {
        store_error(io::Error::other(format!(
            "failed to write {}: {err}",
            temp_path.display()
        )))
    })?;
    file.write_all(b"\n").map_err(|err| {
        store_error(io::Error::other(format!(
            "failed to write {}: {err}",
            temp_path.display()
        )))
    })?;
    file.flush().map_err(|err| {
        store_error(io::Error::other(format!(
            "failed to flush {}: {err}",
            temp_path.display()
        )))
    })?;
    file.sync_all().map_err(|err| {
        store_error(io::Error::other(format!(
            "failed to sync {}: {err}",
            temp_path.display()
        )))
    })
}

fn remove_optional(path: &Path) -> Result<(), McpOAuthError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(store_error(io::Error::other(format!(
            "failed to remove {}: {err}",
            path.display()
        )))),
    }
}

#[cfg(unix)]
fn chmod_dir_private(path: &Path) -> Result<(), McpOAuthError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|err| {
        store_error(io::Error::other(format!(
            "failed to chmod {}: {err}",
            path.display()
        )))
    })
}

#[cfg(not(unix))]
fn chmod_dir_private(path: &Path) -> Result<(), McpOAuthError> {
    chmod_windows_private(path, true)
}

#[cfg(unix)]
fn chmod_file_private(path: &Path) -> Result<(), McpOAuthError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|err| {
        store_error(io::Error::other(format!(
            "failed to chmod {}: {err}",
            path.display()
        )))
    })
}

#[cfg(not(unix))]
fn chmod_file_private(path: &Path) -> Result<(), McpOAuthError> {
    chmod_windows_private(path, false)
}

#[cfg(windows)]
fn chmod_windows_private(path: &Path, directory: bool) -> Result<(), McpOAuthError> {
    let target = windows_acl_target(path, directory)?;
    let mut acl = ACL::from_file_handle(target.as_raw_handle() as HANDLE, false)
        .map_err(|code| windows_acl_error("read ACL", path, code))?;
    replace_windows_dacl(&mut acl, directory, path)
}

#[cfg(not(any(unix, windows)))]
fn chmod_windows_private(_path: &Path, _directory: bool) -> Result<(), McpOAuthError> {
    Ok(())
}

#[cfg(windows)]
fn windows_acl_target(path: &Path, directory: bool) -> Result<fs::File, McpOAuthError> {
    let mut options = fs::OpenOptions::new();
    options
        .access_mode(READ_CONTROL | WRITE_DAC)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    if directory {
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    options.open(path).map_err(|err| {
        store_error(io::Error::other(format!(
            "failed to open {} for Windows ACL update: {err}",
            path.display()
        )))
    })
}

#[cfg(windows)]
fn replace_windows_dacl(acl: &mut ACL, directory: bool, path: &Path) -> Result<(), McpOAuthError> {
    for entry in acl
        .all()
        .map_err(|code| windows_acl_error("enumerate ACL", path, code))?
        .into_iter()
        .filter(|entry| is_windows_access_ace(entry.entry_type))
    {
        let sid = string_to_sid(&entry.string_sid)
            .map_err(|code| windows_acl_error("parse existing ACL SID", path, code))?;
        acl.remove_entry(
            sid.as_ptr() as PSID,
            Some(entry.entry_type),
            Some(entry.flags),
        )
        .map_err(|code| windows_acl_error("remove existing ACL entry", path, code))?;
    }

    for sid in private_windows_acl_sids(path)? {
        acl.allow(sid.as_ptr() as PSID, directory, FILE_ALL_ACCESS)
            .map_err(|code| windows_acl_error("grant private ACL entry", path, code))?;
    }
    Ok(())
}

#[cfg(windows)]
fn is_windows_access_ace(entry_type: AceType) -> bool {
    matches!(
        entry_type,
        AceType::AccessAllow
            | AceType::AccessAllowCallback
            | AceType::AccessAllowObject
            | AceType::AccessAllowCallbackObject
            | AceType::AccessDeny
            | AceType::AccessDenyCallback
            | AceType::AccessDenyObject
            | AceType::AccessDenyCallbackObject
    )
}

#[cfg(windows)]
fn private_windows_acl_sids(path: &Path) -> Result<Vec<Vec<u8>>, McpOAuthError> {
    let user = current_user().ok_or_else(|| {
        store_error(io::Error::other(format!(
            "failed to identify current Windows user for {}",
            path.display()
        )))
    })?;
    Ok(vec![
        name_to_sid(&user, None)
            .map_err(|code| windows_acl_error("resolve user SID", path, code))?,
        string_to_sid("S-1-5-18")
            .map_err(|code| windows_acl_error("resolve SYSTEM SID", path, code))?,
        string_to_sid("S-1-5-32-544")
            .map_err(|code| windows_acl_error("resolve Administrators SID", path, code))?,
    ])
}

#[cfg(windows)]
fn windows_acl_error(operation: &str, path: &Path, code: u32) -> McpOAuthError {
    store_error(io::Error::other(format!(
        "failed to {operation} for {}: Windows error {code}",
        path.display()
    )))
}

#[cfg(test)]
fn private_windows_acl_trustees() -> [&'static str; 3] {
    ["current_user", "S-1-5-18", "S-1-5-32-544"]
}

#[allow(clippy::needless_pass_by_value)]
fn store_error(err: io::Error) -> McpOAuthError {
    McpOAuthError::Store(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::mcp::oauth::{McpOAuthIdentity, McpOAuthTransportKind};
    use std::fs;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    fn identity() -> McpOAuthIdentity {
        McpOAuthIdentity::new(
            "linear",
            "https://mcp.example.com/sse?workspace=neo",
            McpOAuthTransportKind::Sse,
        )
        .unwrap()
    }

    fn token_record() -> McpOAuthTokenRecord {
        McpOAuthTokenRecord {
            access_token: "access-token".to_owned(),
            token_type: Some("Bearer".to_owned()),
            refresh_token: Some("refresh-token".to_owned()),
            expires_in: Some(3600),
            token_received_at: 1_717_171_717,
            granted_scopes: vec!["read".to_owned(), "write".to_owned()],
            raw: serde_json::json!({"access_token": "access-token"}),
        }
    }

    fn client_record() -> McpOAuthClientRecord {
        McpOAuthClientRecord {
            client_id: "client-id".to_owned(),
            client_secret: Some("client-secret".to_owned()),
            redirect_uris: vec!["http://127.0.0.1:14500/callback".to_owned()],
            token_endpoint_auth_method: Some("client_secret_post".to_owned()),
            raw: serde_json::json!({"client_id": "client-id"}),
        }
    }

    fn discovery_record() -> McpOAuthDiscoveryRecord {
        McpOAuthDiscoveryRecord {
            authorization_server_metadata: serde_json::json!({
                "authorization_endpoint": "https://auth.example.com/authorize",
                "token_endpoint": "https://auth.example.com/token",
                "registration_endpoint": "https://auth.example.com/register",
                "issuer": "https://auth.example.com"
            }),
            discovered_at: "2026-06-29T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn round_trips_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let store = McpOAuthStore::new(dir.path().to_path_buf());
        let identity = identity();
        let tokens = token_record();

        store.save_tokens(&identity, &tokens).unwrap();

        assert_eq!(store.load_tokens(&identity).unwrap(), Some(tokens));
    }

    #[test]
    fn clear_tokens_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = McpOAuthStore::new(dir.path().to_path_buf());
        let identity = identity();

        store.clear_tokens(&identity).unwrap();
        store.save_tokens(&identity, &token_record()).unwrap();
        store.clear_tokens(&identity).unwrap();
        store.clear_tokens(&identity).unwrap();

        assert!(store.load_tokens(&identity).unwrap().is_none());
    }

    #[test]
    fn round_trips_client() {
        let dir = tempfile::tempdir().unwrap();
        let store = McpOAuthStore::new(dir.path().to_path_buf());
        let identity = identity();
        let client = client_record();

        store.save_client(&identity, &client).unwrap();

        assert_eq!(store.load_client(&identity).unwrap(), Some(client));
    }

    #[test]
    fn round_trips_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let store = McpOAuthStore::new(dir.path().to_path_buf());
        let identity = identity();
        let discovery = discovery_record();

        store.save_discovery(&identity, &discovery).unwrap();

        let loaded = store.load_discovery(&identity).unwrap().unwrap();
        assert_eq!(
            loaded.authorization_server_metadata,
            discovery.authorization_server_metadata
        );
        assert_eq!(loaded.discovered_at, discovery.discovered_at);
    }

    #[test]
    fn malformed_json_returns_invalid_data() {
        let dir = tempfile::tempdir().unwrap();
        let store = McpOAuthStore::new(dir.path().to_path_buf());
        let identity = identity();
        let server_dir = store.server_dir(&identity);
        fs::create_dir_all(&server_dir).unwrap();
        fs::write(server_dir.join("tokens.json"), b"{not json").unwrap();

        let err = store.load_tokens(&identity).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn writes_private_server_dir_and_json_file_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let store = McpOAuthStore::new(dir.path().join("credentials").join("mcp"));
        let identity = identity();

        store.save_tokens(&identity, &token_record()).unwrap();

        let credentials_dir_mode = fs::metadata(dir.path().join("credentials"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let server_dir_mode = fs::metadata(store.server_dir(&identity))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let store_root_mode = fs::metadata(store.root()).unwrap().permissions().mode() & 0o777;
        let token_file_mode = fs::metadata(store.server_dir(&identity).join("tokens.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(credentials_dir_mode, 0o700);
        assert_eq!(store_root_mode, 0o700);
        assert_eq!(server_dir_mode, 0o700);
        assert_eq!(token_file_mode, 0o600);
    }

    #[cfg(windows)]
    #[test]
    fn writes_private_windows_server_dir_and_json_file_acl() {
        let dir = tempfile::tempdir().unwrap();
        let store = McpOAuthStore::new(dir.path().join("credentials").join("mcp"));
        let identity = identity();

        store.save_tokens(&identity, &token_record()).unwrap();

        assert_private_windows_acl(store.root(), true);
        assert_private_windows_acl(&store.server_dir(&identity), true);
        assert_private_windows_acl(&store.server_dir(&identity).join("tokens.json"), false);
    }

    #[test]
    fn successful_write_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = McpOAuthStore::new(dir.path().to_path_buf());
        let identity = identity();

        store.save_tokens(&identity, &token_record()).unwrap();

        let temp_files: Vec<_> = fs::read_dir(store.server_dir(&identity))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(
            temp_files.is_empty(),
            "temp files left behind: {temp_files:?}"
        );
    }

    #[test]
    fn windows_private_acl_trustees_are_only_owner_system_and_admins() {
        let trustees = private_windows_acl_trustees();

        assert_eq!(trustees, ["current_user", "S-1-5-18", "S-1-5-32-544"]);
        assert!(!trustees.contains(&"S-1-1-0"));
        assert!(!trustees.contains(&"S-1-5-11"));
        assert!(!trustees.contains(&"S-1-5-32-545"));
    }

    #[cfg(windows)]
    fn assert_private_windows_acl(path: &Path, directory: bool) {
        let target = windows_acl_target(path, directory).unwrap();
        let acl = ACL::from_file_handle(target.as_raw_handle() as HANDLE, false).unwrap();
        let entries = acl.all().unwrap();
        let access_sids: std::collections::BTreeSet<_> = entries
            .iter()
            .filter(|entry| is_windows_access_ace(entry.entry_type))
            .map(|entry| entry.string_sid.as_str())
            .collect();

        assert!(
            !access_sids.contains("S-1-1-0"),
            "Everyone must not have access ACEs on {}: {access_sids:?}",
            path.display()
        );
        assert!(
            !access_sids.contains("S-1-5-11"),
            "Authenticated Users must not have access ACEs on {}: {access_sids:?}",
            path.display()
        );
        assert!(
            !access_sids.contains("S-1-5-32-545"),
            "Users must not have access ACEs on {}: {access_sids:?}",
            path.display()
        );
        assert!(
            access_sids.contains("S-1-5-18"),
            "SYSTEM must retain access to {}: {access_sids:?}",
            path.display()
        );
        assert!(
            access_sids.contains("S-1-5-32-544"),
            "Administrators must retain access to {}: {access_sids:?}",
            path.display()
        );
    }
}

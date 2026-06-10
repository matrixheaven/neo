use std::{fs, path::Path};

use anyhow::Context as _;
use neo_sdk::CloudClient;
use serde::{Deserialize, Serialize};

use crate::config::{self, AppConfig};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuthFile {
    server_url: String,
    user_id: String,
    device_id: String,
    access_token: String,
    device_token: String,
    token_type: String,
}

impl From<neo_cloud_protocol::BootstrapResponse> for AuthFile {
    fn from(response: neo_cloud_protocol::BootstrapResponse) -> Self {
        Self {
            server_url: String::new(),
            user_id: response.user_id,
            device_id: response.device_id,
            access_token: response.access_token,
            device_token: response.device_token,
            token_type: response.token_type,
        }
    }
}

pub async fn login_cloud(config: &AppConfig, server: &str) -> anyhow::Result<String> {
    let server_url = server.trim_end_matches('/').to_owned();
    anyhow::ensure!(!server_url.is_empty(), "--server must not be empty");
    let auth_file = config::cloud_auth_file(config)?;
    let client = CloudClient::new(&server_url);
    let existing = read_auth(&auth_file)?;
    let response = if let Some(auth) = existing.filter(|auth| auth.server_url == server_url) {
        match client
            .login_with_device_token(&auth.device_id, &auth.device_token)
            .await
        {
            Ok(response) => response,
            Err(_) => client.bootstrap(&device_name(config)).await?,
        }
    } else {
        client.bootstrap(&device_name(config)).await?
    };
    let mut auth = AuthFile::from(response);
    auth.server_url = server_url.clone();
    write_auth(&auth_file, &auth)?;
    Ok(format!(
        "logged in to {server_url} as {}\nauth file: {}\n",
        auth.user_id,
        auth_file.display()
    ))
}

pub fn logout(config: &AppConfig) -> anyhow::Result<String> {
    let auth_file = config::cloud_auth_file(config)?;
    match fs::remove_file(&auth_file) {
        Ok(()) => Ok("logged out\n".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok("not logged in\n".to_owned())
        }
        Err(error) => Err(error)
            .with_context(|| format!("failed to remove auth file {}", auth_file.display())),
    }
}

pub fn auth_status(config: &AppConfig) -> anyhow::Result<String> {
    let auth_file = config::cloud_auth_file(config)?;
    let Some(auth) = read_auth(&auth_file)? else {
        return Ok(format!(
            "not logged in\nauth file: {}\n",
            auth_file.display()
        ));
    };
    Ok(format!(
        "logged in to {} as {}\nauth file: {}\n",
        auth.server_url,
        auth.user_id,
        auth_file.display()
    ))
}

pub async fn sync_push(config: &AppConfig) -> anyhow::Result<String> {
    let auth_file = config::cloud_auth_file(config)?;
    let auth = require_auth(&auth_file)?;
    let client = CloudClient::new(&auth.server_url);
    let status = client
        .push_profile(&auth.access_token, config::cloud_profile(config)?)
        .await?;
    Ok(format!("profile pushed: revision {}\n", status.revision))
}

pub async fn sync_pull(config: &AppConfig) -> anyhow::Result<String> {
    let auth_file = config::cloud_auth_file(config)?;
    let auth = require_auth(&auth_file)?;
    let client = CloudClient::new(&auth.server_url);
    let pulled = client.pull_profile(&auth.access_token).await?;
    config::apply_cloud_profile_to_global_config(&pulled.profile, &auth_file)?;
    Ok(format!("profile pulled: revision {}\n", pulled.revision))
}

pub async fn sync_status(config: &AppConfig) -> anyhow::Result<String> {
    let auth_file = config::cloud_auth_file(config)?;
    let auth = require_auth(&auth_file)?;
    let client = CloudClient::new(&auth.server_url);
    let status = client.profile_status(&auth.access_token).await?;
    Ok(format!("remote revision {}\n", status.revision))
}

fn require_auth(auth_file: &Path) -> anyhow::Result<AuthFile> {
    read_auth(auth_file)?
        .with_context(|| format!("not logged in; run `neo login cloud --server <url>` first"))
}

fn read_auth(path: &Path) -> anyhow::Result<Option<AuthFile>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read auth file {}", path.display()))?;
    serde_json::from_str(&content)
        .map(Some)
        .with_context(|| format!("failed to parse auth file {}", path.display()))
}

fn write_auth(path: &Path, auth: &AuthFile) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create auth directory {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(auth)?;
    fs::write(path, content)
        .with_context(|| format!("failed to write auth file {}", path.display()))
}

fn device_name(config: &AppConfig) -> String {
    config
        .project_dir
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("neo-device")
        .to_owned()
}

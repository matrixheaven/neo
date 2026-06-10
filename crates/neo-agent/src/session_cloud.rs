use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use neo_agent_core::{
    AgentEvent, AgentMessage,
    session::{JsonlSessionWriter, SessionMetadataStore, validate_session_id},
};
use neo_sdk::{CloudClient, CloudSharePayload};

use crate::{cloud_commands, config, config::AppConfig, session_commands};

pub async fn share(session_ref: &str, public: bool, config: &AppConfig) -> anyhow::Result<String> {
    let session_id = session_commands::resolve_session_id(session_ref, config)?;
    let jsonl = fs::read_to_string(config.sessions_dir.join(format!("{session_id}.jsonl")))
        .with_context(|| format!("failed to read session {session_id}"))?;
    let auth_file = config::cloud_auth_file(config)?;
    let auth = cloud_commands::require_auth(&auth_file)?;
    let client = CloudClient::new(&auth.server_url);
    let record = client
        .import_session(
            &auth.access_token,
            &session_id,
            jsonl,
            local_session_name(config, &session_id),
            None,
        )
        .await
        .context("failed to push session to cloud")?;
    metadata_store(config).record_cloud_sync(
        &session_id,
        record.id.clone(),
        current_unix_timestamp(),
        record.remote_parent_id.clone(),
    )?;

    let share = client
        .create_share(&auth.access_token, &record.id, public)
        .await
        .context("failed to create cloud share")?;
    metadata_store(config).record_share(
        &session_id,
        record.id.clone(),
        share.record.id.clone(),
        current_unix_timestamp(),
    )?;

    Ok(format!(
        "cloud_id: {}\nshare_id: {}\nvisibility: {}\nhtml_url: {}{}\njson_url: {}{}\n",
        record.id,
        share.record.id,
        if share.record.public {
            "public"
        } else {
            "private"
        },
        auth.server_url,
        share.record.html_url,
        auth.server_url,
        share.record.json_url
    ))
}

pub async fn sync_push(config: &AppConfig) -> anyhow::Result<String> {
    let auth_file = config::cloud_auth_file(config)?;
    let auth = cloud_commands::require_auth(&auth_file)?;
    let client = CloudClient::new(&auth.server_url);
    let mut lines = Vec::new();
    for session in metadata_store(config).list()? {
        let path = config.sessions_dir.join(format!("{}.jsonl", session.id));
        let jsonl = fs::read_to_string(&path)
            .with_context(|| format!("failed to read session {}", path.display()))?;
        let record = client
            .import_session(
                &auth.access_token,
                &session.id,
                jsonl,
                session.name.clone(),
                session.remote_parent_id.clone(),
            )
            .await
            .with_context(|| format!("failed to push session {}", session.id))?;
        metadata_store(config).record_cloud_sync(
            &session.id,
            record.id.clone(),
            current_unix_timestamp(),
            record.remote_parent_id.clone(),
        )?;
        lines.push(format!("pushed {}\tcloud_id={}", session.id, record.id));
    }
    if lines.is_empty() {
        Ok("no sessions\n".to_owned())
    } else {
        Ok(format!("{}\n", lines.join("\n")))
    }
}

pub async fn sync_pull(config: &AppConfig) -> anyhow::Result<String> {
    let auth_file = config::cloud_auth_file(config)?;
    let auth = cloud_commands::require_auth(&auth_file)?;
    let client = CloudClient::new(&auth.server_url);
    let cloud_sessions = client
        .list_sessions(&auth.access_token)
        .await
        .context("failed to list cloud sessions")?;
    let local = metadata_store(config).list().unwrap_or_default();
    let mut lines = Vec::new();
    for record in cloud_sessions {
        if local
            .iter()
            .any(|session| session.cloud_id.as_deref() == Some(record.id.as_str()))
        {
            continue;
        }
        let payload = client
            .get_session(&auth.access_token, &record.id)
            .await
            .with_context(|| format!("failed to fetch cloud session {}", record.id))?;
        let session_id = next_local_session_id(
            config,
            record
                .local_session_id
                .as_deref()
                .unwrap_or(record.id.as_str()),
        )?;
        write_messages_jsonl(config, &session_id, &payload.messages).await?;
        metadata_store(config).record_cloud_sync(
            &session_id,
            payload.record.id.clone(),
            current_unix_timestamp(),
            payload.record.remote_parent_id.clone(),
        )?;
        lines.push(format!(
            "pulled {}\tcloud_id={}",
            session_id, payload.record.id
        ));
    }
    if lines.is_empty() {
        Ok("nothing to pull\n".to_owned())
    } else {
        Ok(format!("{}\n", lines.join("\n")))
    }
}

pub fn sync_status(config: &AppConfig) -> anyhow::Result<String> {
    let sessions = metadata_store(config).list().with_context(|| {
        format!(
            "failed to read sessions directory {}",
            config.sessions_dir.display()
        )
    })?;
    if sessions.is_empty() {
        return Ok("no sessions\n".to_owned());
    }
    let lines = sessions
        .into_iter()
        .map(|session| {
            let mut parts = vec![session.id];
            if let Some(cloud_id) = session.cloud_id {
                parts.push(format!("cloud_id={cloud_id}"));
            } else {
                parts.push("cloud_id=<none>".to_owned());
            }
            if let Some(synced_at) = session.synced_at {
                parts.push(format!("synced_at={synced_at}"));
            }
            if let Some(remote_parent_id) = session.remote_parent_id {
                parts.push(format!("remote_parent_id={remote_parent_id}"));
            }
            if !session.share_ids.is_empty() {
                parts.push(format!("shares={}", session.share_ids.join(",")));
            }
            parts.join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!("{lines}\n"))
}

pub async fn import_share(share_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    let (client, share_id) = cloud_client_from_share_ref(share_ref, config)?;
    let share = client
        .get_share(&share_id)
        .await
        .with_context(|| format!("failed to import share {share_ref}"))?;
    let messages = share_messages(&share)?;
    let session_id = next_local_session_id(config, &format!("imported-{share_id}"))?;
    write_messages_jsonl(config, &session_id, &messages).await?;
    metadata_store(config).record_cloud_sync(
        &session_id,
        share.record.session_id,
        current_unix_timestamp(),
        None,
    )?;
    Ok(format!(
        "session_id: {session_id}\nshare_id: {}\n",
        share.record.id
    ))
}

pub async fn resume_remote(remote_ref: &str, config: &AppConfig) -> anyhow::Result<Option<String>> {
    let auth_file = config::cloud_auth_file(config)?;
    let Ok(auth) = cloud_commands::require_auth(&auth_file) else {
        return Ok(None);
    };
    let client = CloudClient::new(&auth.server_url);
    let cloud_id = if remote_ref.starts_with("sh_") || remote_ref.contains("/shares/") {
        let (_, share_id) = cloud_client_from_share_ref(remote_ref, config)?;
        client
            .get_share(&share_id)
            .await
            .ok()
            .map(|share| share.record.session_id)
    } else if remote_ref.starts_with("cs_") {
        Some(remote_ref.to_owned())
    } else {
        None
    };
    let Some(cloud_id) = cloud_id else {
        return Ok(None);
    };
    let forked = client
        .fork_session(&auth.access_token, &cloud_id)
        .await
        .with_context(|| format!("failed to fork cloud session {cloud_id}"))?;
    let payload = client
        .get_session(&auth.access_token, &forked.id)
        .await
        .with_context(|| format!("failed to fetch forked cloud session {}", forked.id))?;
    let session_id = next_local_session_id(config, &format!("remote-{}", forked.id))?;
    write_messages_jsonl(config, &session_id, &payload.messages).await?;
    metadata_store(config).record_cloud_sync(
        &session_id,
        payload.record.id.clone(),
        current_unix_timestamp(),
        payload.record.remote_parent_id.clone(),
    )?;
    let transcript = session_commands::transcript(&session_id, config).await?;
    Ok(Some(format!(
        "session {session_id}\ncloud_id: {}\nremote_parent_id: {}\n{transcript}",
        payload.record.id,
        payload
            .record
            .remote_parent_id
            .unwrap_or_else(|| cloud_id.to_owned())
    )))
}

fn metadata_store(config: &AppConfig) -> SessionMetadataStore {
    SessionMetadataStore::new(&config.sessions_dir)
}

fn local_session_name(config: &AppConfig, session_id: &str) -> Option<String> {
    metadata_store(config)
        .list()
        .ok()?
        .into_iter()
        .find(|session| session.id == session_id)
        .and_then(|session| session.name)
}

async fn write_messages_jsonl(
    config: &AppConfig,
    session_id: &str,
    messages: &[AgentMessage],
) -> anyhow::Result<()> {
    validate_session_id(session_id)
        .with_context(|| format!("invalid session id {session_id:?}"))?;
    fs::create_dir_all(&config.sessions_dir)?;
    let path = config.sessions_dir.join(format!("{session_id}.jsonl"));
    let mut writer = JsonlSessionWriter::create(&path).await?;
    for message in messages {
        writer
            .append(&AgentEvent::MessageAppended {
                message: message.clone(),
            })
            .await?;
    }
    writer.flush().await?;
    Ok(())
}

fn share_messages(share: &CloudSharePayload) -> anyhow::Result<Vec<AgentMessage>> {
    serde_json::from_value(share.json["messages"].clone()).context("share JSON is missing messages")
}

fn next_local_session_id(config: &AppConfig, base: &str) -> anyhow::Result<String> {
    let base = sanitize_session_id(base);
    for index in 0..1000 {
        let candidate = if index == 0 {
            base.clone()
        } else {
            format!("{base}-{index}")
        };
        validate_session_id(&candidate)
            .with_context(|| format!("invalid generated session id {candidate:?}"))?;
        if !config
            .sessions_dir
            .join(format!("{candidate}.jsonl"))
            .exists()
        {
            return Ok(candidate);
        }
    }
    anyhow::bail!("failed to allocate local session id for {base:?}")
}

fn sanitize_session_id(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim_matches('-');
    if sanitized.is_empty() {
        "session".to_owned()
    } else {
        sanitized.to_owned()
    }
}

fn cloud_client_from_share_ref(
    share_ref: &str,
    config: &AppConfig,
) -> anyhow::Result<(CloudClient, String)> {
    if let Some((base_url, share_id)) = split_share_url(share_ref) {
        return Ok((CloudClient::new(base_url), share_id));
    }
    let auth_file = config::cloud_auth_file(config)?;
    let auth = cloud_commands::require_auth(&auth_file)?;
    Ok((CloudClient::new(auth.server_url), clean_share_id(share_ref)))
}

fn split_share_url(value: &str) -> Option<(String, String)> {
    let scheme_end = value.find("://")?;
    let path_start = value[scheme_end + 3..].find('/')? + scheme_end + 3;
    let base_url = value[..path_start].to_owned();
    let share_id = clean_share_id(value.rsplit('/').next().unwrap_or(value));
    Some((base_url, share_id))
}

fn clean_share_id(value: &str) -> String {
    value
        .trim()
        .trim_end_matches(".html")
        .trim_end_matches(".json")
        .to_owned()
}

fn current_unix_timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:09}Z", duration.as_secs(), duration.subsec_nanos())
}

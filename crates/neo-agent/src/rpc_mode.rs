use std::io::{self, BufRead};

use anyhow::Context;
use neo_agent_core::session::{JsonlSessionReader, SessionMetadataStore, SessionRecord};
use neo_sdk::{
    JsonlCodec, RpcCommandKind, RpcCommandRecord, RpcCommandsResult, RpcError, RpcErrorCode,
    RpcMessage, RpcNotification, RpcRequest, RpcResponse, RpcSessionExportHtmlResult,
    RpcSessionGetResult, RpcSessionRecord, RpcSessionTreeRecord, RpcSessionsListResult,
    RpcSessionsTreeResult,
};
use serde_json::{Value, json};

use crate::{
    config::{self, AppConfig},
    modes::run,
    prompt_templates::{self, PromptTemplateLocation},
    session_commands,
};

pub async fn execute(config: &AppConfig) -> anyhow::Result<String> {
    let mut output = String::new();
    for line in io::stdin().lock().lines() {
        let line = line.context("failed to read RPC stdin")?;
        if line.trim().is_empty() {
            continue;
        }
        let message = match JsonlCodec::decode_line(&line) {
            Ok(message) => message,
            Err(err) => {
                push_rpc_message(
                    &mut output,
                    &RpcMessage::Response(err.to_response("parse-error")),
                )?;
                continue;
            }
        };

        let RpcMessage::Request(request) = message else {
            push_rpc_message(
                &mut output,
                &RpcMessage::Response(RpcResponse::failure(
                    "invalid-message",
                    RpcError::new(
                        RpcErrorCode::InvalidRequest,
                        "neo rpc accepts request messages only",
                        None,
                    ),
                )),
            )?;
            continue;
        };

        handle_request(config, request, &mut output).await?;
    }
    Ok(output)
}

async fn handle_request(
    config: &AppConfig,
    request: RpcRequest,
    output: &mut String,
) -> anyhow::Result<()> {
    match request.method.as_str() {
        "get_state" => push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::success(request.id, state_payload(config))),
        ),
        "get_commands" => handle_get_commands(config, request, output),
        "get_messages" => handle_get_messages(config, request, output).await,
        "sessions.list" => handle_sessions_list(config, request, output),
        "sessions.tree" => handle_sessions_tree(config, request, output),
        "sessions.get" => handle_sessions_get(config, request, output).await,
        "sessions.export_html" => handle_sessions_export_html(config, request, output).await,
        "prompt" => handle_prompt(config, request, output).await,
        unknown => push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::failure(
                request.id,
                RpcError::new(
                    RpcErrorCode::MethodNotFound,
                    format!("unsupported RPC method: {unknown}"),
                    None,
                ),
            )),
        ),
    }
}

fn handle_get_commands(
    config: &AppConfig,
    request: RpcRequest,
    output: &mut String,
) -> anyhow::Result<()> {
    let commands = match prompt_templates::discover_prompt_template_commands(
        &config.project_dir,
        config::global_prompts_dir().as_deref(),
        &config.configured_prompt_templates,
    ) {
        Ok(commands) => commands,
        Err(err) => {
            return push_rpc_message(
                output,
                &RpcMessage::Response(RpcResponse::failure(
                    request.id,
                    RpcError::new(RpcErrorCode::InternalError, err.to_string(), None),
                )),
            );
        }
    };
    let commands = commands
        .into_iter()
        .map(|command| RpcCommandRecord {
            name: format!("/{}", command.template.name),
            kind: RpcCommandKind::PromptTemplate,
            template: command.template.name,
            description: command.template.description,
            argument_hint: command.template.argument_hint,
            location: rpc_prompt_template_location(command.location).to_owned(),
            path: command.template.path.display().to_string(),
        })
        .collect();

    push_rpc_message(
        output,
        &RpcMessage::Response(RpcResponse::success(
            request.id,
            serde_json::to_value(RpcCommandsResult { commands })?,
        )),
    )
}

fn handle_sessions_list(
    config: &AppConfig,
    request: RpcRequest,
    output: &mut String,
) -> anyhow::Result<()> {
    let sessions = match session_store(config).list() {
        Ok(sessions) => sessions,
        Err(err) => {
            return push_rpc_message(
                output,
                &RpcMessage::Response(RpcResponse::failure(
                    request.id,
                    RpcError::new(RpcErrorCode::InternalError, err.to_string(), None),
                )),
            );
        }
    };

    push_rpc_message(
        output,
        &RpcMessage::Response(RpcResponse::success(
            request.id,
            serde_json::to_value(RpcSessionsListResult {
                sessions: sessions.into_iter().map(rpc_session_record).collect(),
            })?,
        )),
    )
}

fn handle_sessions_tree(
    config: &AppConfig,
    request: RpcRequest,
    output: &mut String,
) -> anyhow::Result<()> {
    let sessions = match session_store(config).list() {
        Ok(sessions) => sessions,
        Err(err) => {
            return push_rpc_message(
                output,
                &RpcMessage::Response(RpcResponse::failure(
                    request.id,
                    RpcError::new(RpcErrorCode::InternalError, err.to_string(), None),
                )),
            );
        }
    };
    let tree = session_commands::tree_order_sessions(&sessions)
        .into_iter()
        .map(|tree_record| RpcSessionTreeRecord {
            depth: tree_record.depth,
            record: rpc_session_record(tree_record.record),
        })
        .collect();

    push_rpc_message(
        output,
        &RpcMessage::Response(RpcResponse::success(
            request.id,
            serde_json::to_value(RpcSessionsTreeResult { tree })?,
        )),
    )
}

async fn handle_sessions_get(
    config: &AppConfig,
    request: RpcRequest,
    output: &mut String,
) -> anyhow::Result<()> {
    let Some(session_ref) = request.params.get("session_id").and_then(Value::as_str) else {
        return push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::failure(
                request.id,
                RpcError::new(
                    RpcErrorCode::InvalidParams,
                    "sessions.get params.session_id must be a string",
                    None,
                ),
            )),
        );
    };

    let session_id = match session_commands::resolve_session_id(session_ref, config) {
        Ok(session_id) => session_id,
        Err(err) => {
            return push_rpc_message(
                output,
                &RpcMessage::Response(RpcResponse::failure(
                    request.id,
                    RpcError::new(RpcErrorCode::InvalidParams, err.to_string(), None),
                )),
            );
        }
    };
    let path = config.sessions_dir.join(format!("{session_id}.jsonl"));
    if !path.exists() {
        return push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::failure(
                request.id,
                RpcError::new(
                    RpcErrorCode::InvalidParams,
                    format!("session {session_ref:?} does not exist"),
                    None,
                ),
            )),
        );
    }

    let sessions = match session_store(config).list() {
        Ok(sessions) => sessions,
        Err(err) => {
            return push_rpc_message(
                output,
                &RpcMessage::Response(RpcResponse::failure(
                    request.id,
                    RpcError::new(RpcErrorCode::InternalError, err.to_string(), None),
                )),
            );
        }
    };
    let Some(record) = sessions
        .into_iter()
        .find(|session| session.id == session_id)
    else {
        return push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::failure(
                request.id,
                RpcError::new(
                    RpcErrorCode::InvalidParams,
                    format!("session {session_ref:?} does not exist"),
                    None,
                ),
            )),
        );
    };

    let messages = match JsonlSessionReader::replay_messages(&path).await {
        Ok(messages) => messages,
        Err(err) => {
            return push_rpc_message(
                output,
                &RpcMessage::Response(RpcResponse::failure(
                    request.id,
                    RpcError::new(RpcErrorCode::InternalError, err.to_string(), None),
                )),
            );
        }
    };
    let rpc_record = rpc_session_record(record);
    let messages = messages
        .into_iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()?;

    push_rpc_message(
        output,
        &RpcMessage::Response(RpcResponse::success(
            request.id,
            serde_json::to_value(RpcSessionGetResult {
                record: rpc_record,
                path: path.display().to_string(),
                messages,
            })?,
        )),
    )
}

async fn handle_sessions_export_html(
    config: &AppConfig,
    request: RpcRequest,
    output: &mut String,
) -> anyhow::Result<()> {
    let Some(session_ref) = request.params.get("session_id").and_then(Value::as_str) else {
        return push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::failure(
                request.id,
                RpcError::new(
                    RpcErrorCode::InvalidParams,
                    "sessions.export_html params.session_id must be a string",
                    None,
                ),
            )),
        );
    };

    let session_id = match session_commands::resolve_session_id(session_ref, config) {
        Ok(session_id) => session_id,
        Err(err) => {
            return push_rpc_message(
                output,
                &RpcMessage::Response(RpcResponse::failure(
                    request.id,
                    RpcError::new(RpcErrorCode::InvalidParams, err.to_string(), None),
                )),
            );
        }
    };
    let path = config.sessions_dir.join(format!("{session_id}.jsonl"));
    if !path.exists() {
        return push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::failure(
                request.id,
                RpcError::new(
                    RpcErrorCode::InvalidParams,
                    format!("session {session_ref:?} does not exist"),
                    None,
                ),
            )),
        );
    }

    let html = match session_commands::export_html(&session_id, config).await {
        Ok(html) => html,
        Err(err) => {
            return push_rpc_message(
                output,
                &RpcMessage::Response(RpcResponse::failure(
                    request.id,
                    RpcError::new(RpcErrorCode::InternalError, err.to_string(), None),
                )),
            );
        }
    };

    push_rpc_message(
        output,
        &RpcMessage::Response(RpcResponse::success(
            request.id,
            serde_json::to_value(RpcSessionExportHtmlResult { session_id, html })?,
        )),
    )
}

async fn handle_get_messages(
    config: &AppConfig,
    request: RpcRequest,
    output: &mut String,
) -> anyhow::Result<()> {
    let Some(session_id) = request.params.get("session_id").and_then(Value::as_str) else {
        return push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::failure(
                request.id,
                RpcError::new(
                    RpcErrorCode::InvalidParams,
                    "get_messages params.session_id must be a string",
                    None,
                ),
            )),
        );
    };

    let path = match session_commands::session_path(session_id, config) {
        Ok(path) => path,
        Err(err) => {
            return push_rpc_message(
                output,
                &RpcMessage::Response(RpcResponse::failure(
                    request.id,
                    RpcError::new(RpcErrorCode::InvalidParams, err.to_string(), None),
                )),
            );
        }
    };

    if !path.exists() {
        return push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::failure(
                request.id,
                RpcError::new(
                    RpcErrorCode::InvalidParams,
                    format!("session {session_id:?} does not exist"),
                    None,
                ),
            )),
        );
    }

    let messages = match JsonlSessionReader::replay_messages(&path).await {
        Ok(messages) => messages,
        Err(err) => {
            return push_rpc_message(
                output,
                &RpcMessage::Response(RpcResponse::failure(
                    request.id,
                    RpcError::new(RpcErrorCode::InternalError, err.to_string(), None),
                )),
            );
        }
    };

    push_rpc_message(
        output,
        &RpcMessage::Response(RpcResponse::success(
            request.id,
            json!({
                "session_id": session_id,
                "messages": messages,
            }),
        )),
    )
}

async fn handle_prompt(
    config: &AppConfig,
    request: RpcRequest,
    output: &mut String,
) -> anyhow::Result<()> {
    let Some(message) = request.params.get("message").and_then(Value::as_str) else {
        return push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::failure(
                request.id,
                RpcError::new(
                    RpcErrorCode::InvalidParams,
                    "prompt params.message must be a string",
                    None,
                ),
            )),
        );
    };

    let turn = run::run_prompt(&[message.to_owned()], config).await?;
    for event in &turn.events {
        push_rpc_message(
            output,
            &RpcMessage::Notification(RpcNotification::new(
                "agent.event",
                serde_json::to_value(event)?,
            )),
        )?;
    }
    push_rpc_message(
        output,
        &RpcMessage::Response(RpcResponse::success(
            request.id,
            json!({
                "assistant_text": turn.assistant_text,
                "event_count": turn.events.len(),
            }),
        )),
    )
}

fn session_store(config: &AppConfig) -> SessionMetadataStore {
    SessionMetadataStore::new(&config.sessions_dir)
}

fn rpc_session_record(record: SessionRecord) -> RpcSessionRecord {
    RpcSessionRecord {
        id: record.id,
        name: record.name,
        summary: record.summary,
        parent_id: record.parent_id,
        children: record.children,
    }
}

fn rpc_prompt_template_location(location: PromptTemplateLocation) -> &'static str {
    match location {
        PromptTemplateLocation::Configured => "configured",
        PromptTemplateLocation::Project => "project",
        PromptTemplateLocation::User => "user",
    }
}

fn state_payload(config: &AppConfig) -> Value {
    json!({
        "provider": config.default_provider,
        "model": config.default_model,
        "sessions_dir": config.sessions_dir,
        "session_count": session_count(config),
        "mode": config.defaults.mode,
    })
}

fn session_count(config: &AppConfig) -> usize {
    let Ok(entries) = std::fs::read_dir(&config.sessions_dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension == "jsonl")
        })
        .count()
}

fn push_rpc_message(output: &mut String, message: &RpcMessage) -> anyhow::Result<()> {
    output.push_str(&JsonlCodec::encode(message)?);
    Ok(())
}

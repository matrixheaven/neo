use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use neo_agent_core::rpc::{
    RpcError, RpcErrorCode, RpcMessage, RpcNotification, RpcRequest, RpcResponse, codec::JsonlCodec,
};
use neo_agent_core::session::{
    JsonlSessionReader, SessionMetadataStore, SessionRecord, SessionSummarySource,
};
use neo_agent_core::{AgentEvent, ApprovalAction, ApprovalResponse};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use super::types::{
    RpcCommandKind, RpcCommandRecord, RpcCommandsResult, RpcSessionExportHtmlResult,
    RpcSessionGetResult, RpcSessionRecord, RpcSessionsListResult,
};

use crate::{
    config::{self, AppConfig, workspace_sessions_dir},
    modes::{run, sessions},
    prompt::templates::{self, PromptTemplateLocation},
};

type ApprovalChannelMap =
    Arc<Mutex<HashMap<String, HashMap<String, oneshot::Sender<ApprovalResponse>>>>>;

pub async fn execute(config: &AppConfig) -> anyhow::Result<()> {
    let stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let approval_channels: ApprovalChannelMap = Arc::new(Mutex::new(HashMap::new()));
    for line in stdin.lines() {
        let line = line.context("failed to read RPC stdin")?;
        if line.trim().is_empty() {
            continue;
        }
        let message = match JsonlCodec::decode_line(&line) {
            Ok(message) => message,
            Err(err) => {
                push_rpc_message(
                    &mut stdout,
                    &RpcMessage::Response(err.to_response("parse-error")),
                )?;
                continue;
            }
        };

        let RpcMessage::Request(request) = message else {
            push_rpc_message(
                &mut stdout,
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

        handle_request(config, request, &mut stdout, &approval_channels).await?;
    }
    Ok(())
}

async fn handle_request(
    config: &AppConfig,
    request: RpcRequest,
    output: &mut dyn Write,
    approval_channels: &ApprovalChannelMap,
) -> anyhow::Result<()> {
    match request.method.as_str() {
        "get_state" => push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::success(request.id, state_payload(config))),
        ),
        "get_commands" => handle_get_commands(config, request, output),
        "get_messages" => handle_get_messages(config, request, output).await,
        "sessions.list" => handle_sessions_list(config, request, output),
        "sessions.get" => handle_sessions_get(config, request, output).await,
        "sessions.export_html" => handle_sessions_export_html(config, request, output).await,
        "sessions.export_json" => handle_sessions_export_json(config, request, output).await,
        "set_session_name" => handle_set_session_name(config, request, output),
        "prompt" => handle_prompt(config, request, output, approval_channels).await,
        "approve_tool" => {
            let Some(session_id) =
                request.params.get("session_id").and_then(Value::as_str)
            else {
                return push_rpc_message(
                    output,
                    &RpcMessage::Response(RpcResponse::failure(
                        request.id,
                        RpcError::new(
                            RpcErrorCode::InvalidParams,
                            "session_id required",
                            None,
                        ),
                    )),
                );
            };
            let Some(approval_id) =
                request.params.get("request_id").and_then(Value::as_str)
            else {
                return push_rpc_message(
                    output,
                    &RpcMessage::Response(RpcResponse::failure(
                        request.id,
                        RpcError::new(
                            RpcErrorCode::InvalidParams,
                            "request_id required",
                            None,
                        ),
                    )),
                );
            };
            let Some(action_str) = request.params.get("action").and_then(Value::as_str)
            else {
                return push_rpc_message(
                    output,
                    &RpcMessage::Response(RpcResponse::failure(
                        request.id,
                        RpcError::new(
                            RpcErrorCode::InvalidParams,
                            "action required",
                            None,
                        ),
                    )),
                );
            };

            let action = match action_str {
                "permit_once" => ApprovalAction::PermitOnce,
                "reject" => ApprovalAction::Reject,
                other => {
                    return push_rpc_message(
                        output,
                        &RpcMessage::Response(RpcResponse::failure(
                            request.id,
                            RpcError::new(
                                RpcErrorCode::InvalidParams,
                                format!("unknown action: {other}"),
                                None,
                            ),
                        )),
                    );
                }
            };

            let mut channels = approval_channels.lock().unwrap();
            if let Some(session_approvals) = channels.get_mut(session_id) {
                if let Some(tx) = session_approvals.remove(approval_id) {
                    let _ = tx.send(ApprovalResponse::Selected {
                        request_id: approval_id.to_owned(),
                        action,
                        feedback: None,
                    });
                }
            }

            push_rpc_message(
                output,
                &RpcMessage::Response(RpcResponse::success(
                    request.id,
                    serde_json::json!({"status": "ok"}),
                )),
            )
        }
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
    output: &mut dyn Write,
) -> anyhow::Result<()> {
    let commands = match templates::discover_prompt_template_commands(
        &config.project_dir,
        config::global_prompts_dir().as_deref(),
        &config.prompt_templates,
        config.project_trusted,
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
    output: &mut dyn Write,
) -> anyhow::Result<()> {
    let sessions = match session_store(config).list_recent() {
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

async fn handle_sessions_get(
    config: &AppConfig,
    request: RpcRequest,
    output: &mut dyn Write,
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

    let session_id = match sessions::resolve_session_id(session_ref, config) {
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
    let path = neo_agent_core::session::main_agent_wire_path(
        &workspace_sessions_dir(config).join(&session_id),
    );
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
    output: &mut dyn Write,
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

    let session_id = match sessions::resolve_session_id(session_ref, config) {
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
    let path = neo_agent_core::session::main_agent_wire_path(
        &workspace_sessions_dir(config).join(&session_id),
    );
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

    let html = match sessions::export_html(&session_id, config).await {
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

async fn handle_sessions_export_json(
    config: &AppConfig,
    request: RpcRequest,
    output: &mut dyn Write,
) -> anyhow::Result<()> {
    let Some(session_ref) = request.params.get("session_id").and_then(Value::as_str) else {
        return push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::failure(
                request.id,
                RpcError::new(
                    RpcErrorCode::InvalidParams,
                    "sessions.export_json params.session_id must be a string",
                    None,
                ),
            )),
        );
    };

    if let Err(err) = sessions::resolve_session_id(session_ref, config) {
        return push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::failure(
                request.id,
                RpcError::new(RpcErrorCode::InvalidParams, err.to_string(), None),
            )),
        );
    }

    let artifact = match sessions::export_json_artifact(session_ref, config).await {
        Ok(artifact) => artifact,
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
            serde_json::to_value(artifact)?,
        )),
    )
}

fn handle_set_session_name(
    config: &AppConfig,
    request: RpcRequest,
    output: &mut dyn Write,
) -> anyhow::Result<()> {
    let Some(session_ref) = request.params.get("session_id").and_then(Value::as_str) else {
        return push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::failure(
                request.id,
                RpcError::new(
                    RpcErrorCode::InvalidParams,
                    "set_session_name params.session_id must be a string",
                    None,
                ),
            )),
        );
    };
    let Some(name) = request.params.get("name").and_then(Value::as_str) else {
        return push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::failure(
                request.id,
                RpcError::new(
                    RpcErrorCode::InvalidParams,
                    "set_session_name params.name must be a string",
                    None,
                ),
            )),
        );
    };

    let session_id = match sessions::resolve_session_id(session_ref, config) {
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
    let path = neo_agent_core::session::main_agent_wire_path(
        &workspace_sessions_dir(config).join(&session_id),
    );
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

    match sessions::rename(&session_id, name, config) {
        Ok(_) => push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::success(
                request.id,
                json!({
                    "session_id": session_id,
                    "name": name,
                }),
            )),
        ),
        Err(err) => push_rpc_message(
            output,
            &RpcMessage::Response(RpcResponse::failure(
                request.id,
                RpcError::new(RpcErrorCode::InternalError, err.to_string(), None),
            )),
        ),
    }
}

async fn handle_get_messages(
    config: &AppConfig,
    request: RpcRequest,
    output: &mut dyn Write,
) -> anyhow::Result<()> {
    let Some(session_ref) = request.params.get("session_id").and_then(Value::as_str) else {
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

    let session_id = match sessions::resolve_session_id(session_ref, config) {
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
    let path = neo_agent_core::session::main_agent_wire_path(
        &workspace_sessions_dir(config).join(&session_id),
    );

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
    output: &mut dyn Write,
    approval_channels: &ApprovalChannelMap,
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

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let prompt = vec![message.to_owned()];
    let session_id_param = request.params.get("session_id").and_then(Value::as_str);
    let resolved_sid: Option<String> = match session_id_param {
        Some(sid) => match sessions::resolve_session_id(sid, config) {
            Ok(sid) => Some(sid),
            Err(err) => {
                return push_rpc_message(
                    output,
                    &RpcMessage::Response(RpcResponse::failure(
                        request.id,
                        RpcError::new(RpcErrorCode::InvalidParams, err.to_string(), None),
                    )),
                );
            }
        },
        None => None,
    };
    let sid = resolved_sid.clone().unwrap_or_default();
    let channels = Arc::clone(approval_channels);

    let mut turn: std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<run::PromptTurn>> + '_>> = if let Some(ref sid) = resolved_sid {
        Box::pin(run::run_prompt_in_session_with_event_stream(sid, &prompt, config, event_tx))
    } else {
        Box::pin(run::run_prompt_with_event_stream(&prompt, config, event_tx))
    };
    let mut turn_result: Option<anyhow::Result<run::PromptTurn>> = None;
    loop {
        if let Some(result) = turn_result.take() {
            match event_rx.recv().await {
                Some(Ok(event)) => {
                    handle_approval_event(&event, &sid, &channels);
                    push_rpc_message(
                        output,
                        &RpcMessage::Notification(RpcNotification::new(
                            "agent.event",
                            serde_json::to_value(event)?,
                        )),
                    )?;
                    turn_result = Some(result);
                    continue;
                }
                Some(Err(_)) => {
                    turn_result = Some(result);
                    continue;
                }
                None => match result {
                    Ok(turn) => {
                        return push_rpc_message(
                            output,
                            &RpcMessage::Response(RpcResponse::success(
                                request.id,
                                json!({
                                    "assistant_text": turn.assistant_text,
                                    "event_count": turn.events.len(),
                                }),
                            )),
                        );
                    }
                    Err(err) => {
                        return push_rpc_message(
                            output,
                            &RpcMessage::Response(RpcResponse::failure(
                                request.id,
                                RpcError::new(RpcErrorCode::InternalError, err.to_string(), None),
                            )),
                        );
                    }
                },
            }
        }

        tokio::select! {
            result = event_rx.recv() => {
                match result {
                    Some(Ok(event)) => {
                        handle_approval_event(&event, &sid, &channels);
                        push_rpc_message(
                            output,
                            &RpcMessage::Notification(RpcNotification::new(
                                "agent.event",
                                serde_json::to_value(event)?,
                            )),
                        )?;
                    }
                    Some(Err(_)) => {}
                    None => {}
                }
            }
            result = &mut turn => {
                turn_result = Some(result);
            }
        }
    }
}

fn handle_approval_event(
    event: &AgentEvent,
    sid: &str,
    approval_channels: &ApprovalChannelMap,
) {
    if let AgentEvent::ApprovalRequested { request } = event {
        let (tx, rx) = oneshot::channel();
        {
            let mut channels = approval_channels.lock().unwrap();
            channels
                .entry(sid.to_owned())
                .or_default()
                .insert(request.id.clone(), tx);
        }
        tokio::spawn(async move {
            let _ = rx.await;
        });
    }
}

fn session_store(config: &AppConfig) -> SessionMetadataStore {
    SessionMetadataStore::new(workspace_sessions_dir(config))
}

fn rpc_session_record(record: SessionRecord) -> RpcSessionRecord {
    let summary_record = record.summary_record;
    RpcSessionRecord {
        id: record.id,
        title: record.title,
        title_model: record.title_model,
        title_updated_at: record.title_updated_at,
        workspace: record.workspace,
        last_user_prompt: record.last_user_prompt,
        updated_at: record.updated_at,
        name: record.name,
        summary: record.summary,
        summary_source: summary_record
            .as_ref()
            .map(|summary| rpc_summary_source(summary.source).to_owned()),
        summary_model: summary_record
            .as_ref()
            .and_then(|summary| summary.model.clone()),
        summary_updated_at: summary_record.and_then(|summary| summary.updated_at),
        parent_id: record.parent_id,
        children: record.children,
    }
}

fn rpc_summary_source(source: SessionSummarySource) -> &'static str {
    match source {
        SessionSummarySource::LocalExtractive => "local_extractive",
        SessionSummarySource::ModelGenerated => "model_generated",
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
    let bucket_dir = workspace_sessions_dir(config);
    let Ok(entries) = std::fs::read_dir(&bucket_dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return false;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                return false;
            };
            name.starts_with("session_")
                && neo_agent_core::session::main_agent_wire_path(&path).is_file()
        })
        .count()
}

fn push_rpc_message(output: &mut dyn Write, message: &RpcMessage) -> anyhow::Result<()> {
    output.write_all(JsonlCodec::encode(message)?.as_bytes())?;
    output.flush()?;
    Ok(())
}

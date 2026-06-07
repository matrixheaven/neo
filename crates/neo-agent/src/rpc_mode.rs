use std::io::{self, BufRead};

use anyhow::Context;
use neo_agent_core::session::JsonlSessionReader;
use neo_sdk::{
    JsonlCodec, RpcError, RpcErrorCode, RpcMessage, RpcNotification, RpcRequest, RpcResponse,
};
use serde_json::{Value, json};

use crate::{config::AppConfig, modes::run, session_commands};

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
        "get_messages" => handle_get_messages(config, request, output).await,
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

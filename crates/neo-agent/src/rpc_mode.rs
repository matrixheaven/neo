use std::io::{self, BufRead};

use anyhow::Context;
use neo_sdk::{
    JsonlCodec, RpcError, RpcErrorCode, RpcMessage, RpcNotification, RpcRequest, RpcResponse,
};
use serde_json::{Value, json};

use crate::{config::AppConfig, modes::run};

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
        "is_streaming": false,
        "sessions_dir": config.sessions_dir,
        "mode": config.defaults.mode,
    })
}

fn push_rpc_message(output: &mut String, message: &RpcMessage) -> anyhow::Result<()> {
    output.push_str(&JsonlCodec::encode(message)?);
    Ok(())
}

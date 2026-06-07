use std::sync::Arc;

use anyhow::Context;
use futures::{StreamExt, stream};
use neo_agent_core::session::JsonlSessionWriter;
use neo_agent_core::{AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, Content};
use neo_ai::{
    AiError, AiStreamEvent, ApiKind, ChatMessage, ChatRequest, ContentPart, ModelCapabilities,
    ModelClient, ModelSpec, ProviderId, StopReason,
};

use crate::{config::AppConfig, session_commands};

pub async fn execute(prompt: &[String], config: &AppConfig) -> anyhow::Result<String> {
    let turn = run_prompt(prompt, config).await?;
    let mut output = String::new();
    for event in turn.events {
        output.push_str(&serde_json::to_string(&event)?);
        output.push('\n');
    }
    Ok(output)
}

pub async fn resume(session_id: &str, config: &AppConfig) -> anyhow::Result<String> {
    let transcript = session_commands::transcript(session_id, config).await?;
    Ok(format!("session {session_id}\n{transcript}"))
}

pub fn list_models(config: &AppConfig) -> String {
    format!(
        "models:\n- {}/{} (configured default)\n- fake/fake-agent-model (local deterministic)\n",
        config.default_provider, config.default_model
    )
}

pub fn list_mcp_servers(_config: &AppConfig) -> String {
    "no MCP servers configured\n".to_owned()
}

pub struct PromptTurn {
    pub events: Vec<AgentEvent>,
    pub assistant_text: String,
}

pub async fn run_prompt(prompt: &[String], config: &AppConfig) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let session_path = create_session_path(config).await?;
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;

    let user_message = AgentMessage::user_text(prompt.clone());
    let user_event = AgentEvent::MessageAppended {
        message: user_message.clone(),
    };
    writer.append_event(&user_event).await?;

    let runtime = AgentRuntime::new(
        AgentConfig::for_model(model_spec(config)),
        Arc::new(DeterministicLocalModel),
    );
    let mut context = AgentContext::new();
    let mut events = vec![user_event];
    let mut assistant_text = String::new();
    let turn_events = runtime
        .run_turn(&mut context, user_message)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    for event in turn_events {
        if let AgentEvent::MessageAppended { message } = &event {
            assistant_text.push_str(&message_text(message));
        }
        writer.append_event(&event).await?;
        events.push(event);
    }
    writer.flush().await?;

    Ok(PromptTurn {
        events,
        assistant_text,
    })
}

async fn create_session_path(config: &AppConfig) -> anyhow::Result<std::path::PathBuf> {
    tokio::fs::create_dir_all(&config.sessions_dir)
        .await
        .with_context(|| {
            format!(
                "failed to create sessions directory {}",
                config.sessions_dir.display()
            )
        })?;

    let mut counter = 0_u32;
    loop {
        let suffix = if counter == 0 {
            String::new()
        } else {
            format!("-{counter}")
        };
        let path = config.sessions_dir.join(format!(
            "{}{suffix}.jsonl",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis()
        ));
        if tokio::fs::metadata(&path).await.is_err() {
            return Ok(path);
        }
        counter = counter.saturating_add(1);
    }
}

fn model_spec(config: &AppConfig) -> ModelSpec {
    ModelSpec {
        provider: ProviderId(config.default_provider.clone()),
        model: config.default_model.clone(),
        api: ApiKind::Local,
        capabilities: ModelCapabilities {
            streaming: true,
            tools: true,
            images: false,
            reasoning: false,
            embeddings: false,
            max_context_tokens: None,
        },
    }
}

fn message_text(message: &AgentMessage) -> String {
    let content = match message {
        AgentMessage::System { content }
        | AgentMessage::User { content }
        | AgentMessage::Assistant { content, .. }
        | AgentMessage::ToolResult { content, .. } => content,
    };

    content
        .iter()
        .filter_map(Content::as_text)
        .collect::<Vec<_>>()
        .join("")
}

struct DeterministicLocalModel;

impl ModelClient for DeterministicLocalModel {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        let prompt = last_user_text(&request).unwrap_or_default();
        let text = format!("fake response: {prompt}");
        stream::iter([
            Ok(AiStreamEvent::MessageStart {
                id: "fake_message".to_owned(),
            }),
            Ok(AiStreamEvent::TextDelta { text }),
            Ok(AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            }),
        ])
        .boxed()
    }
}

fn last_user_text(request: &ChatRequest) -> Option<String> {
    request.messages.iter().rev().find_map(|message| {
        let ChatMessage::User { content } = message else {
            return None;
        };
        Some(
            content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    ContentPart::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        )
    })
}

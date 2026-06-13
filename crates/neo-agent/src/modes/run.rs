use std::{
    collections::BTreeMap,
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

use anyhow::Context;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use futures::StreamExt;
use neo_agent_core::session::{JsonlSessionReader, JsonlSessionWriter, SessionMetadataStore};
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, CompactionSettings, Content,
    McpHttpConfig, McpHttpToolAdapter, McpStdioConfig, McpStdioToolAdapter, McpToolAdapter,
    McpToolProvider, PermissionDecision, ToolRegistry,
};
use neo_ai::{
    ApiKind, CredentialResolver, ImageData, ImageGenerationClient, ImageGenerationRequest,
    ModelClient, ModelRegistry, ModelSpec, ProviderRegistry, ProviderSpec, ResolvedCredential,
    providers::openai_images::OpenAiImagesClient,
};
use reqwest::header;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    cli::RunOutput,
    config::{self, AppConfig, McpServerConfig},
    extension_tools, resources, session_commands,
};

const REMOTE_IMAGE_MAX_BYTES: u64 = 20 * 1024 * 1024;
const REMOTE_IMAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

pub async fn execute(
    prompt: &[String],
    config: &AppConfig,
    output: RunOutput,
    session_target: Option<SessionTarget<'_>>,
    session_name: Option<&str>,
    no_session: bool,
) -> anyhow::Result<String> {
    let turn = if no_session {
        run_prompt_ephemeral(prompt, config).await?
    } else if let Some(session_target) = session_target {
        run_prompt_with_session_target(session_target, prompt, config).await?
    } else {
        run_prompt(prompt, config).await?
    };
    if !no_session {
        apply_session_name(config, &turn.session_id, session_name)?;
    }
    if matches!(output, RunOutput::Json) {
        return stable_json_output(&turn, config);
    }

    let mut output = String::new();
    for event in turn.events {
        output.push_str(&serde_json::to_string(&event)?);
        output.push('\n');
    }
    Ok(output)
}

pub async fn resume(session_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    let session_id = session_commands::resolve_session_id(session_ref, config)?;
    let transcript = session_commands::transcript(&session_id, config).await?;
    Ok(format!("session {session_id}\n{transcript}"))
}

fn stable_json_output(turn: &PromptTurn, config: &AppConfig) -> anyhow::Result<String> {
    let mut output = String::new();
    write_json_line(
        &mut output,
        &json!({
            "type": "session",
            "version": 1,
            "id": turn.session_id,
            "timestamp": current_unix_timestamp(),
            "cwd": config.project_dir,
        }),
    )?;

    let mut state = StableJsonState::default();
    for event in &turn.events {
        for value in state.map_event(event) {
            write_json_line(&mut output, &value)?;
        }
    }
    Ok(output)
}

fn write_json_line(output: &mut String, value: &Value) -> anyhow::Result<()> {
    output.push_str(&serde_json::to_string(value)?);
    output.push('\n');
    Ok(())
}

#[derive(Debug, Default)]
struct StableJsonState {
    assistant_content: Vec<AssistantContentState>,
    active_text_index: Option<usize>,
    active_thinking_index: Option<usize>,
    assistant_message_id: Option<String>,
    assistant_stop_reason: Option<neo_agent_core::StopReason>,
    messages: Vec<Value>,
    tool_results: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AssistantContentState {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
        redacted: bool,
    },
}

impl StableJsonState {
    fn map_event(&mut self, event: &AgentEvent) -> Vec<Value> {
        if let Some(value) = self.map_lifecycle_event(event) {
            return vec![value];
        }
        if let Some(value) = self.map_tool_execution_event(event) {
            return vec![value];
        }
        self.map_other_event(event)
    }

    fn map_lifecycle_event(&mut self, event: &AgentEvent) -> Option<Value> {
        match event {
            AgentEvent::RunStarted { .. } => Some(json!({ "type": "agent_start" })),
            AgentEvent::TurnStarted { turn } => Some(json!({
                "type": "turn_start",
                "turn": turn,
            })),
            AgentEvent::MessageStarted { turn, id } => Some(self.map_message_started(*turn, id)),
            AgentEvent::ThinkingStarted { turn, id: _ } => Some(self.map_thinking_started(*turn)),
            AgentEvent::ThinkingDelta { turn, text } => Some(self.map_thinking_delta(*turn, text)),
            AgentEvent::ThinkingFinished {
                turn,
                signature,
                redacted,
            } => Some(self.map_thinking_finished(*turn, signature.as_ref(), *redacted)),
            AgentEvent::TextDelta { turn, text } => Some(self.map_text_delta(*turn, text)),
            AgentEvent::MessageFinished {
                turn,
                id: _,
                stop_reason,
            } => {
                self.assistant_stop_reason = Some(*stop_reason);
                Some(json!({
                    "type": "message_end",
                    "turn": turn,
                    "message": self.assistant_message(),
                }))
            }
            AgentEvent::TurnFinished { turn, stop_reason } => Some(json!({
                "type": "turn_end",
                "turn": turn,
                "stopReason": stable_stop_reason(*stop_reason),
                "message": self.assistant_message(),
                "toolResults": self.tool_results,
            })),
            AgentEvent::RunFinished { turn, stop_reason } => Some(json!({
                "type": "agent_end",
                "turn": turn,
                "stopReason": stable_stop_reason(*stop_reason),
                "messages": self.messages,
            })),
            _ => None,
        }
    }

    fn map_tool_execution_event(&mut self, event: &AgentEvent) -> Option<Value> {
        match event {
            AgentEvent::ToolExecutionStarted {
                turn,
                id,
                name,
                arguments,
            } => Some(json!({
                "type": "tool_execution_start",
                "turn": turn,
                "toolCallId": id,
                "toolName": name,
                "args": arguments,
            })),
            AgentEvent::ToolExecutionUpdate {
                turn,
                id,
                name,
                partial_result,
            } => Some(json!({
                "type": "tool_execution_update",
                "turn": turn,
                "toolCallId": id,
                "toolName": name,
                "partialResult": partial_result,
            })),
            AgentEvent::ToolExecutionFinished {
                turn,
                id,
                name,
                result,
            } => {
                let result_message = json!({
                    "role": "tool",
                    "toolCallId": id,
                    "toolName": name,
                    "content": result.content,
                    "isError": result.is_error,
                });
                push_unique(&mut self.tool_results, result_message);
                Some(json!({
                    "type": "tool_execution_end",
                    "turn": turn,
                    "toolCallId": id,
                    "toolName": name,
                    "result": result,
                    "isError": result.is_error,
                }))
            }
            _ => None,
        }
    }

    fn map_other_event(&mut self, event: &AgentEvent) -> Vec<Value> {
        match event {
            AgentEvent::MessageAppended { message } => {
                push_unique(&mut self.messages, stable_message(message));
                Vec::new()
            }
            AgentEvent::Error { turn, message } => vec![json!({
                "type": "error",
                "turn": turn,
                "message": message,
            })],
            AgentEvent::QueueDrained { kind, count } => vec![json!({
                "type": "queue_update",
                "kind": format!("{kind:?}").to_lowercase(),
                "count": count,
            })],
            AgentEvent::CompactionStarted {
                reason,
                tokens_before,
                message_count,
            } => vec![json!({
                "type": "compaction_start",
                "reason": stable_compaction_reason(*reason),
                "tokensBefore": tokens_before,
                "messageCount": message_count,
            })],
            AgentEvent::CompactionProgress { phase, percent } => vec![json!({
                "type": "compaction_update",
                "phase": stable_compaction_phase(*phase),
                "percent": percent,
            })],
            AgentEvent::CompactionApplied { summary } => vec![json!({
                "type": "compaction_end",
                "reason": "threshold",
                "result": summary,
                "aborted": false,
                "willRetry": false,
            })],
            _ => Vec::new(),
        }
    }

    fn map_message_started(&mut self, turn: u32, id: &str) -> Value {
        self.assistant_content.clear();
        self.active_text_index = None;
        self.active_thinking_index = None;
        self.assistant_message_id = Some(id.to_owned());
        self.assistant_stop_reason = None;
        json!({
            "type": "message_start",
            "turn": turn,
            "message": self.assistant_message(),
        })
    }

    fn map_thinking_started(&mut self, turn: u32) -> Value {
        let content_index = self.push_thinking_content();
        json!({
            "type": "message_update",
            "turn": turn,
            "message": self.assistant_message(),
            "assistantMessageEvent": {
                "type": "thinking_start",
                "contentIndex": content_index,
                "partial": self.content_part(content_index),
            },
        })
    }

    fn map_thinking_delta(&mut self, turn: u32, text: &str) -> Value {
        let content_index = self.ensure_active_thinking_content();
        if let Some(AssistantContentState::Thinking { thinking, .. }) =
            self.assistant_content.get_mut(content_index)
        {
            thinking.push_str(text);
        }
        json!({
            "type": "message_update",
            "turn": turn,
            "message": self.assistant_message(),
            "assistantMessageEvent": {
                "type": "thinking_delta",
                "contentIndex": content_index,
                "delta": text,
                "partial": self.content_part(content_index),
            },
        })
    }

    fn map_thinking_finished(
        &mut self,
        turn: u32,
        signature: Option<&String>,
        redacted: bool,
    ) -> Value {
        let content_index = self.ensure_active_thinking_content();
        if let Some(AssistantContentState::Thinking {
            signature: state_signature,
            redacted: state_redacted,
            ..
        }) = self.assistant_content.get_mut(content_index)
        {
            *state_signature = signature.cloned();
            *state_redacted = redacted;
        }
        let content = self
            .assistant_content
            .get(content_index)
            .and_then(AssistantContentState::thinking_text)
            .unwrap_or_default();
        let partial = self.content_part(content_index);
        self.active_thinking_index = None;
        json!({
            "type": "message_update",
            "turn": turn,
            "message": self.assistant_message(),
            "assistantMessageEvent": {
                "type": "thinking_end",
                "contentIndex": content_index,
                "content": content,
                "partial": partial,
            },
        })
    }

    fn map_text_delta(&mut self, turn: u32, text: &str) -> Value {
        let content_index = self.ensure_active_text_content();
        if let Some(AssistantContentState::Text { text: state_text }) =
            self.assistant_content.get_mut(content_index)
        {
            state_text.push_str(text);
        }
        json!({
            "type": "message_update",
            "turn": turn,
            "message": self.assistant_message(),
            "assistantMessageEvent": {
                "type": "text_delta",
                "contentIndex": content_index,
                "delta": text,
                "partial": self.content_part(content_index),
            },
        })
    }

    fn assistant_message(&self) -> Value {
        json!({
            "role": "assistant",
            "id": self.assistant_message_id,
            "content": self.assistant_content(),
            "toolCalls": [],
            "stopReason": self.assistant_stop_reason.map(stable_stop_reason),
        })
    }

    fn assistant_content(&self) -> Vec<Value> {
        self.assistant_content
            .iter()
            .map(AssistantContentState::to_json)
            .collect()
    }

    fn content_part(&self, index: usize) -> Value {
        self.assistant_content
            .get(index)
            .map_or(Value::Null, AssistantContentState::to_json)
    }

    fn push_thinking_content(&mut self) -> usize {
        self.assistant_content
            .push(AssistantContentState::Thinking {
                thinking: String::new(),
                signature: None,
                redacted: false,
            });
        let index = self.assistant_content.len() - 1;
        self.active_thinking_index = Some(index);
        self.active_text_index = None;
        index
    }

    fn ensure_active_thinking_content(&mut self) -> usize {
        if let Some(index) = self.active_thinking_index
            && matches!(
                self.assistant_content.get(index),
                Some(AssistantContentState::Thinking { .. })
            )
        {
            return index;
        }
        self.push_thinking_content()
    }

    fn ensure_active_text_content(&mut self) -> usize {
        if let Some(index) = self.active_text_index
            && matches!(
                self.assistant_content.get(index),
                Some(AssistantContentState::Text { .. })
            )
        {
            return index;
        }
        self.assistant_content.push(AssistantContentState::Text {
            text: String::new(),
        });
        let index = self.assistant_content.len() - 1;
        self.active_text_index = Some(index);
        index
    }
}

impl AssistantContentState {
    fn to_json(&self) -> Value {
        match self {
            Self::Text { text } => json!({
                "type": "text",
                "text": text,
            }),
            Self::Thinking {
                thinking,
                signature,
                redacted,
            } => json!({
                "type": "thinking",
                "thinking": thinking,
                "thinkingSignature": signature,
                "redacted": redacted,
            }),
        }
    }

    fn thinking_text(&self) -> Option<String> {
        match self {
            Self::Thinking { thinking, .. } => Some(thinking.clone()),
            Self::Text { .. } => None,
        }
    }
}

fn push_unique(values: &mut Vec<Value>, value: Value) {
    if values.last() != Some(&value) {
        values.push(value);
    }
}

fn stable_message(message: &AgentMessage) -> Value {
    match message {
        AgentMessage::System { content } => json!({
            "role": "system",
            "content": stable_content(content),
        }),
        AgentMessage::User { content } => json!({
            "role": "user",
            "content": stable_content(content),
        }),
        AgentMessage::Assistant {
            content,
            tool_calls,
            stop_reason,
        } => json!({
            "role": "assistant",
            "content": stable_content(content),
            "toolCalls": tool_calls,
            "stopReason": stable_stop_reason(*stop_reason),
        }),
        AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            is_error,
        } => json!({
            "role": "tool",
            "toolCallId": tool_call_id,
            "toolName": tool_name,
            "content": stable_content(content),
            "isError": is_error,
        }),
    }
}

fn stable_content(content: &[Content]) -> Vec<Value> {
    content
        .iter()
        .map(|part| match part {
            Content::Text { text } => json!({
                "type": "text",
                "text": text,
            }),
            Content::Thinking {
                text,
                signature,
                redacted,
            } => json!({
                "type": "thinking",
                "thinking": text,
                "thinkingSignature": signature,
                "redacted": redacted,
            }),
            Content::Image { mime_type, data } => json!({
                "type": "image",
                "mimeType": mime_type,
                "data": data,
            }),
        })
        .collect()
}

fn stable_stop_reason(stop_reason: neo_agent_core::StopReason) -> &'static str {
    match stop_reason {
        neo_agent_core::StopReason::EndTurn => "end_turn",
        neo_agent_core::StopReason::ToolUse => "tool_use",
        neo_agent_core::StopReason::MaxTokens => "max_tokens",
        neo_agent_core::StopReason::Cancelled => "cancelled",
        neo_agent_core::StopReason::Error => "error",
    }
}

fn stable_compaction_reason(reason: neo_agent_core::CompactionReason) -> &'static str {
    match reason {
        neo_agent_core::CompactionReason::Threshold => "threshold",
    }
}

fn stable_compaction_phase(phase: neo_agent_core::CompactionPhase) -> &'static str {
    match phase {
        neo_agent_core::CompactionPhase::Estimating => "estimating",
        neo_agent_core::CompactionPhase::SelectingBoundary => "selecting_boundary",
        neo_agent_core::CompactionPhase::Summarizing => "summarizing",
        neo_agent_core::CompactionPhase::Applying => "applying",
    }
}

fn current_unix_timestamp() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:09}Z", duration.as_secs(), duration.subsec_nanos())
}

pub fn list_models_filtered(config: &AppConfig, search: Option<&str>) -> anyhow::Result<String> {
    list_models_with_search_and_options(config, search, false)
}

pub fn list_models_with_options(config: &AppConfig, json_output: bool) -> anyhow::Result<String> {
    list_models_with_search_and_options(config, None, json_output)
}

fn list_models_with_search_and_options(
    config: &AppConfig,
    search: Option<&str>,
    json_output: bool,
) -> anyhow::Result<String> {
    let providers = provider_registry_for_config(config);
    let models = model_registry_for_config(config)?;
    let search = search.map(str::trim).filter(|search| !search.is_empty());
    let listed_models = models
        .list()
        .into_iter()
        .filter(|model| model_matches_search(&models, model, search))
        .collect::<Vec<_>>();
    if let Some(search) = search
        && listed_models.is_empty()
    {
        return Ok(format!("no models matching \"{search}\"\n"));
    }
    if json_output {
        return list_models_json(config, &providers, &models, &listed_models);
    }

    let mut out = "models:\n".to_owned();
    if search.is_none() {
        let _ = writeln!(
            out,
            "- {}/{} (configured default)",
            config.default_provider, config.default_model
        );
    }
    for model in listed_models {
        let marker =
            if model.provider.0 == config.default_provider && model.model == config.default_model {
                " default"
            } else {
                ""
            };
        let display = model_display_suffix(&models, &model)
            .map(|display| format!(" - {display}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "- {}/{} ({:?}{marker}){display}",
            model.provider.0, model.model, model.api
        );
    }
    out.push_str("providers:\n");
    for provider in providers.list() {
        let status = provider_credential_status_for_config(config, &provider.id)
            .map_or("unknown", credential_status_label);
        let _ = writeln!(out, "- {} ({:?}, {status})", provider.id, provider.api);
    }
    Ok(out)
}

pub async fn generate_image(
    config: &AppConfig,
    prompt: &str,
    model: &str,
    output: &Path,
    size: &str,
) -> anyhow::Result<String> {
    let (provider_id, model_id) = parse_provider_model(model)?;
    let registry = model_registry_for_config(config)?;
    let model = registry
        .list()
        .into_iter()
        .find(|candidate| candidate.provider.0 == provider_id && candidate.model == model_id)
        .with_context(|| format!("unknown image model {model}; run `neo models list --json`"))?;
    anyhow::ensure!(
        registry.supports_image_generation(&model.provider.0, &model.model),
        "model {}/{} does not advertise image generation support",
        model.provider.0,
        model.model
    );
    anyhow::ensure!(
        matches!(
            model.api,
            ApiKind::OpenAiResponses | ApiKind::OpenAiCompatible | ApiKind::OpenAiChatCompletions
        ),
        "image generation currently requires an OpenAI-style image endpoint"
    );

    let provider = provider_with_invocation_overrides(config, &model.provider.0)
        .with_context(|| format!("provider {} is not registered", model.provider.0))?;
    let credential = resolve_provider_credential(config, &provider)
        .with_context(|| format!("missing credentials for provider {}", provider.id))?;
    let base_url = provider
        .base_url
        .as_deref()
        .with_context(|| format!("provider {} does not define a base URL", provider.id))?;
    let client = OpenAiImagesClient::new(base_url, credential.secret());
    let response = client
        .generate_image(ImageGenerationRequest {
            model,
            prompt: prompt.to_owned(),
            size: size.to_owned(),
        })
        .await
        .map_err(anyhow::Error::from)?;
    let image = response
        .images
        .into_iter()
        .next()
        .context("provider returned no images")?;
    let bytes = match image.data {
        ImageData::Base64(value) => BASE64_STANDARD
            .decode(value)
            .context("provider returned invalid base64 image data")?,
        ImageData::Url(url) => {
            if !config.tui.fetch_remote_images {
                anyhow::bail!(
                    "provider returned an image URL ({url}); enable tui.fetch_remote_images = true to allow Neo to fetch remote image outputs"
                );
            }
            fetch_remote_image(&url).await?
        }
    };
    let output = workspace_safe_output_path(&config.project_dir, output)?;
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(&output, bytes)
        .with_context(|| format!("failed to write image to {}", output.display()))?;
    Ok(format!("wrote image to {}\n", output.display()))
}

async fn fetch_remote_image(url: &str) -> anyhow::Result<Vec<u8>> {
    let parsed = reqwest::Url::parse(url).context("provider returned an invalid image URL")?;
    anyhow::ensure!(
        matches!(parsed.scheme(), "http" | "https"),
        "remote image URL must use http or https"
    );
    let client = reqwest::Client::builder()
        .timeout(REMOTE_IMAGE_FETCH_TIMEOUT)
        .build()
        .context("failed to initialize remote image fetch client")?;
    let response = client
        .get(parsed)
        .send()
        .await
        .context("failed to fetch remote image URL")?;
    let status = response.status();
    anyhow::ensure!(
        status.is_success(),
        "remote image fetch failed with HTTP status {}",
        status.as_u16()
    );
    if let Some(length) = response.content_length() {
        anyhow::ensure!(
            length <= REMOTE_IMAGE_MAX_BYTES,
            "remote image response is larger than {REMOTE_IMAGE_MAX_BYTES} bytes"
        );
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let media_type = content_type
        .split_once(';')
        .map_or(content_type.as_str(), |(media_type, _)| media_type)
        .trim()
        .to_ascii_lowercase();
    anyhow::ensure!(
        media_type.starts_with("image/"),
        "remote image response content-type {content_type} is not allowed"
    );
    let bytes = response
        .bytes()
        .await
        .context("failed to read remote image response")?;
    anyhow::ensure!(
        u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= REMOTE_IMAGE_MAX_BYTES,
        "remote image response is larger than {REMOTE_IMAGE_MAX_BYTES} bytes"
    );
    Ok(bytes.to_vec())
}

fn workspace_safe_output_path(project_dir: &Path, output: &Path) -> anyhow::Result<PathBuf> {
    let project_dir = project_dir.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize project dir {}",
            project_dir.display()
        )
    })?;
    let output = if output.is_absolute() {
        output.to_path_buf()
    } else {
        project_dir.join(output)
    };
    let parent = output.parent().unwrap_or(&project_dir);
    let canonical_parent = nearest_existing_ancestor(parent)?
        .canonicalize()
        .with_context(|| format!("failed to resolve output directory {}", parent.display()))?;
    anyhow::ensure!(
        canonical_parent.starts_with(&project_dir),
        "image output path must stay inside workspace {}",
        project_dir.display()
    );
    Ok(output)
}

fn nearest_existing_ancestor(path: &Path) -> anyhow::Result<&Path> {
    path.ancestors()
        .find(|ancestor| ancestor.exists())
        .context("failed to find an existing output directory ancestor")
}

fn parse_provider_model(model: &str) -> anyhow::Result<(&str, &str)> {
    let (provider, model) = model
        .split_once('/')
        .with_context(|| format!("model must be qualified as provider/model, got {model:?}"))?;
    let provider = provider.trim();
    let model = model.trim();
    anyhow::ensure!(
        !provider.is_empty() && !model.is_empty(),
        "model must be qualified as provider/model"
    );
    Ok((provider, model))
}

fn list_models_json(
    config: &AppConfig,
    providers: &ProviderRegistry,
    registry: &ModelRegistry,
    models: &[ModelSpec],
) -> anyhow::Result<String> {
    let models = models
        .iter()
        .map(|model| {
            json!({
                "provider": model.provider.0,
                "model": model.model,
                "api": format!("{:?}", model.api),
                "default": model.provider.0 == config.default_provider && model.model == config.default_model,
                "context_window": model.capabilities.max_context_tokens,
                "capabilities": {
                    "streaming": model.capabilities.streaming,
                    "tools": model.capabilities.tools,
                    "images": model.capabilities.images,
                    "reasoning": model.capabilities.reasoning,
                    "embeddings": model.capabilities.embeddings,
                    "image_generation": registry.supports_image_generation(&model.provider.0, &model.model),
                }
            })
        })
        .collect::<Vec<_>>();
    let providers = providers
        .list()
        .into_iter()
        .map(|provider| {
            let status = provider_credential_status_for_config(config, &provider.id)
                .map_or("unknown", credential_status_label);
            json!({
                "id": provider.id,
                "api": format!("{:?}", provider.api),
                "status": status,
            })
        })
        .collect::<Vec<_>>();
    Ok(format!(
        "{}\n",
        serde_json::to_string(&json!({
            "models": models,
            "providers": providers,
        }))?
    ))
}

const fn credential_status_label(configured: bool) -> &'static str {
    if configured {
        "configured"
    } else {
        "missing credentials"
    }
}

fn model_matches_search(registry: &ModelRegistry, model: &ModelSpec, search: Option<&str>) -> bool {
    let Some(search) = search else {
        return true;
    };
    let display = model_display_suffix(registry, model).unwrap_or_default();
    let haystack = format!("{} {} {}", model.provider.0, model.model, display);
    fuzzy_match(&haystack, search)
}

fn model_display_suffix(registry: &ModelRegistry, model: &ModelSpec) -> Option<String> {
    let metadata = registry.display_metadata(&model.provider.0, &model.model)?;
    match (
        metadata.provider_name.as_deref(),
        metadata.model_name.as_deref(),
    ) {
        (Some(provider), Some(model)) => Some(format!("{provider} / {model}")),
        (Some(provider), None) => Some(provider.to_owned()),
        (None, Some(model)) => Some(model.to_owned()),
        (None, None) => None,
    }
}

fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.to_lowercase();
    let needle = needle.to_lowercase();
    if haystack.contains(&needle) {
        return true;
    }
    let mut chars = haystack.chars();
    needle
        .chars()
        .all(|needle_char| chars.any(|candidate| candidate == needle_char))
}

fn provider_registry_for_config(config: &AppConfig) -> ProviderRegistry {
    let mut registry = ProviderRegistry::production();
    apply_configured_provider_overrides(&mut registry, config);
    if let Some(env_name) = &config.api_key_env
        && let Some(mut provider) = registry.get(&config.default_provider).cloned()
    {
        provider.api_key_env_vars = vec![env_name.clone()];
        registry.register(provider);
    }
    registry
}

fn provider_with_invocation_overrides(
    config: &AppConfig,
    provider_id: &str,
) -> Option<ProviderSpec> {
    let registry = provider_registry_for_config(config);
    let mut provider = registry.get(provider_id).cloned()?;
    if let Some(base_url) = &config.api_base {
        provider.base_url = Some(base_url.clone());
    }
    if let Some(env_name) = &config.api_key_env {
        provider.api_key_env_vars = vec![env_name.clone()];
    }
    Some(provider)
}

fn provider_credential_status_for_config(config: &AppConfig, provider_id: &str) -> Option<bool> {
    let provider = provider_with_invocation_overrides(config, provider_id)?;
    let env = env::vars().collect::<BTreeMap<_, _>>();
    let ambient_authenticated = provider.ambient_auth_env_vars.iter().any(|group| {
        group
            .iter()
            .all(|key| env.get(key).is_some_and(|value| !value.is_empty()))
    });
    let key_authenticated = resolve_provider_credential_from_env(config, &provider, &env).is_some();
    Some(ambient_authenticated || key_authenticated)
}

fn resolve_provider_credential(
    config: &AppConfig,
    provider: &ProviderSpec,
) -> Option<ResolvedCredential> {
    resolve_provider_credential_from_env(config, provider, &env::vars().collect())
}

fn resolve_provider_credential_from_env(
    config: &AppConfig,
    provider: &ProviderSpec,
    env: &BTreeMap<String, String>,
) -> Option<ResolvedCredential> {
    CredentialResolver::new(&provider.id)
        .with_cli_api_key(config.api_key.clone())
        .with_env(provider.api_key_env_vars.iter().map(String::as_str), env)
        .with_auth_file_credentials(BTreeMap::new())
        .resolve()
}

fn apply_configured_provider_overrides(registry: &mut ProviderRegistry, config: &AppConfig) {
    for (provider_id, provider_config) in &config.providers {
        let Some(mut provider) = registry.get(provider_id).cloned() else {
            continue;
        };
        if let Some(base_url) = &provider_config.api_base {
            provider.base_url = Some(base_url.clone());
        }
        if let Some(env_name) = &provider_config.api_key_env {
            provider.api_key_env_vars = vec![env_name.clone()];
        }
        registry.register(provider);
    }
}

pub fn list_mcp_servers(config: &AppConfig) -> String {
    if config.mcp.servers.is_empty() {
        return "no MCP servers configured\n".to_owned();
    }

    let mut out = String::new();
    for server in &config.mcp.servers {
        let state = if server.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let args = if server.args.is_empty() {
            String::new()
        } else {
            format!(" {}", server.args.join(" "))
        };
        let endpoint = if matches!(server.transport.as_str(), "http" | "sse") {
            server.url.as_deref().unwrap_or("")
        } else {
            server.command.as_deref().unwrap_or("")
        };
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}{}",
            server.id, state, server.transport, endpoint, args
        );
    }
    out
}

pub async fn list_mcp_tools(config: &AppConfig, server_id: &str) -> anyhow::Result<String> {
    let server = enabled_mcp_server(config, server_id)?;
    let adapter = mcp_adapter_for_server(server)?;
    let provider = McpToolProvider::discover_dyn(&server.id, adapter)
        .await
        .with_context(|| format!("failed to discover MCP tools from {server_id}"))?;
    let specs = provider.specs();
    if specs.is_empty() {
        return Ok("no MCP tools\n".to_owned());
    }

    let mut out = String::new();
    for spec in specs {
        let input_schema = serde_json::to_string(&spec.input_schema)?;
        let _ = writeln!(out, "{}\t{}\t{}", spec.name, spec.description, input_schema);
    }
    Ok(out)
}

pub fn add_mcp_server(
    server_id: String,
    transport: String,
    command: Option<String>,
    url: Option<String>,
    args: Vec<String>,
    env: Vec<String>,
    headers: Vec<String>,
) -> anyhow::Result<String> {
    config::upsert_mcp_server(&McpServerConfig {
        id: server_id,
        enabled: true,
        transport,
        command,
        url,
        args,
        env: key_value_pairs(env, "--env")?,
        headers: key_value_pairs(headers, "--header")?,
    })
}

pub async fn mcp_server_health(config: &AppConfig, server_id: &str) -> anyhow::Result<String> {
    let server = enabled_mcp_server(config, server_id)?;
    let adapter = mcp_adapter_for_server(server)?;
    let tools = adapter
        .list_tools()
        .await
        .with_context(|| format!("failed to probe MCP server {server_id}"))?;
    Ok(format!(
        "{server_id}\thealthy\t{} {}\n",
        tools.len(),
        if tools.len() == 1 { "tool" } else { "tools" }
    ))
}

pub fn start_mcp_server(config: &AppConfig, server_id: &str) -> anyhow::Result<String> {
    let server = enabled_mcp_server(config, server_id)?;
    anyhow::ensure!(
        server.transport == "stdio",
        "MCP server {server_id} uses {} transport; only stdio servers can be started locally",
        server.transport
    );
    let command = server
        .command
        .as_deref()
        .with_context(|| format!("missing MCP command for {server_id}"))?;
    let mut process = std::process::Command::new("sh");
    process
        .arg("-c")
        .arg("tail -f /dev/null | \"$0\" \"$@\"")
        .arg(command)
        .args(&server.args)
        .envs(&server.env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        process.process_group(0);
    }
    let child = process
        .spawn()
        .with_context(|| format!("failed to start MCP server {server_id}"))?;
    let child_pid = child.id();
    let reported_pid = wait_for_mcp_server_pid(server, Duration::from_secs(2));
    let mut state = read_mcp_process_state(config)?;
    state.servers.retain(|record| record.id != server_id);
    state.servers.push(McpProcessRecord {
        id: server_id.to_owned(),
        child_pid,
        #[cfg(unix)]
        process_group_id: Some(child_pid),
        #[cfg(not(unix))]
        process_group_id: None,
        server_pid: reported_pid,
    });
    write_mcp_process_state(config, &state)?;
    Ok(format!("started MCP server {server_id}\tpid={child_pid}\n"))
}

pub fn stop_mcp_server(config: &AppConfig, server_id: &str) -> anyhow::Result<String> {
    let mut state = read_mcp_process_state(config)?;
    let Some(index) = state
        .servers
        .iter()
        .position(|record| record.id == server_id)
    else {
        anyhow::bail!("MCP server {server_id} is not running");
    };
    let record = state.servers.remove(index);
    terminate_mcp_process(&record);
    write_mcp_process_state(config, &state)?;
    Ok(format!("stopped MCP server {server_id}\n"))
}

pub async fn list_mcp_resources(config: &AppConfig, server_id: &str) -> anyhow::Result<String> {
    let server = enabled_mcp_server(config, server_id)?;
    let adapter = mcp_adapter_for_server(server)?;
    let resources = adapter
        .list_resources()
        .await
        .with_context(|| format!("failed to list MCP resources from {server_id}"))?;
    if resources.is_empty() {
        return Ok("no MCP resources\n".to_owned());
    }
    let mut out = String::new();
    for resource in resources {
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}",
            resource.uri,
            resource.name,
            resource.mime_type.unwrap_or_default(),
            resource.description.unwrap_or_default()
        );
    }
    Ok(out)
}

pub async fn read_mcp_resource(
    config: &AppConfig,
    server_id: &str,
    uri: &str,
) -> anyhow::Result<String> {
    let server = enabled_mcp_server(config, server_id)?;
    let adapter = mcp_adapter_for_server(server)?;
    let resource = adapter
        .read_resource(uri)
        .await
        .with_context(|| format!("failed to read MCP resource {uri} from {server_id}"))?;
    if resource.contents.is_empty() {
        return Ok("no MCP resource content\n".to_owned());
    }
    let mut out = String::new();
    for content in resource.contents {
        let _ = writeln!(
            out,
            "{}\t{}",
            content.uri,
            content.mime_type.unwrap_or_default()
        );
        if let Some(text) = content.text {
            out.push_str(&text);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        } else if let Some(blob) = content.blob {
            out.push_str(&blob);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    Ok(out)
}

pub async fn watch_mcp_resource(
    config: &AppConfig,
    server_id: &str,
    uri: &str,
    count: usize,
) -> anyhow::Result<String> {
    anyhow::ensure!(count > 0, "MCP resource watch count must be greater than 0");
    let server = enabled_mcp_server(config, server_id)?;
    let adapter = mcp_adapter_for_server(server)?;
    adapter
        .subscribe_resource(uri)
        .await
        .with_context(|| format!("failed to subscribe to MCP resource {uri} from {server_id}"))?;

    let mut out = String::new();
    let mut watch_result = Ok(());
    for _ in 0..count {
        match adapter.next_resource_update().await {
            Ok(update) => {
                let _ = writeln!(out, "{}", update.uri);
            }
            Err(err) => {
                watch_result = Err(err);
                break;
            }
        }
    }

    let unsubscribe_result = adapter.unsubscribe_resource(uri).await;
    if let Err(err) = watch_result {
        return Err(anyhow::Error::from(err).context(format!(
            "failed while watching MCP resource {uri} from {server_id}"
        )));
    }
    unsubscribe_result
        .with_context(|| format!("failed to unsubscribe from MCP resource {uri} on {server_id}"))?;
    Ok(out)
}

pub struct PromptTurn {
    pub session_id: String,
    pub events: Vec<AgentEvent>,
    pub assistant_text: String,
}

#[derive(Debug, Clone, Copy)]
pub enum SessionTarget<'a> {
    ExactId(&'a str),
    Existing(&'a str),
    Latest,
    Fork(&'a str),
}

pub struct PromptApprovalRequest {
    pub id: String,
    pub decision_tx: oneshot::Sender<PermissionDecision>,
}

pub async fn run_prompt(prompt: &[String], config: &AppConfig) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let session_path = create_session_path(config).await?;
    let session_id = session_id_from_path(&session_path)?;
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    let mut writer = SessionEventWriter::jsonl(&mut writer);
    let (user_message, events) = append_user_event(prompt, &mut writer).await?;
    let runtime = runtime_for_config(config, None).await?;
    finish_prompt_turn(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        events,
        session_id,
    )
    .await
}

pub async fn run_prompt_ephemeral(
    prompt: &[String],
    config: &AppConfig,
) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let mut writer = SessionEventWriter::memory();
    let (user_message, events) = append_user_event(prompt, &mut writer).await?;
    let runtime = runtime_for_config(config, None).await?;
    finish_prompt_turn(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        events,
        "ephemeral".to_owned(),
    )
    .await
}

pub async fn run_prompt_with_session_target(
    session_target: SessionTarget<'_>,
    prompt: &[String],
    config: &AppConfig,
) -> anyhow::Result<PromptTurn> {
    match session_target {
        SessionTarget::ExactId(session_id) => {
            run_prompt_with_exact_session_id(session_id, prompt, config).await
        }
        SessionTarget::Existing(session_ref) => {
            let session_id = session_commands::resolve_session_id(session_ref, config)?;
            run_prompt_in_session(&session_id, prompt, config).await
        }
        SessionTarget::Latest => {
            let session_id = latest_session_id(config)?;
            run_prompt_in_session(&session_id, prompt, config).await
        }
        SessionTarget::Fork(session_ref) => {
            let parent_id = session_commands::resolve_session_id(session_ref, config)?;
            let child = SessionMetadataStore::new(&config.sessions_dir)
                .fork(&parent_id, None)
                .with_context(|| format!("failed to fork session {session_ref}"))?;
            run_prompt_in_session(&child.id, prompt, config).await
        }
    }
}

pub fn apply_session_name(
    config: &AppConfig,
    session_id: &str,
    session_name: Option<&str>,
) -> anyhow::Result<()> {
    let Some(session_name) = session_name else {
        return Ok(());
    };
    SessionMetadataStore::new(&config.sessions_dir)
        .rename(session_id, session_name.to_owned())
        .with_context(|| format!("failed to name session {session_id}"))?;
    Ok(())
}

async fn run_prompt_with_exact_session_id(
    session_id: &str,
    prompt: &[String],
    config: &AppConfig,
) -> anyhow::Result<PromptTurn> {
    let session_path = exact_session_path(session_id, config).await?;
    if tokio::fs::metadata(&session_path).await.is_ok() {
        return run_prompt_in_session(session_id, prompt, config).await;
    }

    let prompt = prompt.join(" ");
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    let mut writer = SessionEventWriter::jsonl(&mut writer);
    let (user_message, events) = append_user_event(prompt, &mut writer).await?;
    let runtime = runtime_for_config(config, None).await?;
    finish_prompt_turn(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        events,
        session_id.to_owned(),
    )
    .await
}

#[allow(dead_code)]
pub async fn run_prompt_in_session(
    session_id: &str,
    prompt: &[String],
    config: &AppConfig,
) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let session_path = session_commands::session_path(session_id, config)?;
    let context = JsonlSessionReader::replay_context(&session_path)
        .await
        .with_context(|| format!("failed to replay session {}", session_path.display()))?;
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .with_context(|| format!("failed to append session {}", session_path.display()))?;
    let mut writer = SessionEventWriter::jsonl(&mut writer);
    let (user_message, events) = append_user_event(prompt, &mut writer).await?;
    let runtime = runtime_for_config(config, None).await?;
    finish_prompt_turn(
        user_message,
        context,
        &mut writer,
        runtime,
        events,
        session_id.to_owned(),
    )
    .await
}

pub async fn run_prompt_streaming(
    prompt: &[String],
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    approval_tx: mpsc::UnboundedSender<PromptApprovalRequest>,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    cancel_token: CancellationToken,
) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let session_path = create_session_path(config).await?;
    let session_id = session_id_from_path(&session_path)?;
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    if let Some(session_id_tx) = session_id_tx {
        let _ = session_id_tx.send(session_id.clone());
    }
    let (user_message, events) = append_user_event_jsonl(prompt, &mut writer).await?;
    let runtime = runtime_for_config(config, Some(approval_tx)).await?;
    let streaming = StreamingTurnIo {
        event_tx,
        session_id,
        cancel_token,
    };
    finish_prompt_turn_streaming(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        events,
        streaming,
    )
    .await
}

pub async fn run_prompt_in_session_streaming(
    session_id: &str,
    prompt: &[String],
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    approval_tx: mpsc::UnboundedSender<PromptApprovalRequest>,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    cancel_token: CancellationToken,
) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let session_path = session_commands::session_path(session_id, config)?;
    let context = JsonlSessionReader::replay_context(&session_path)
        .await
        .with_context(|| format!("failed to replay session {}", session_path.display()))?;
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .with_context(|| format!("failed to append session {}", session_path.display()))?;
    if let Some(session_id_tx) = session_id_tx {
        let _ = session_id_tx.send(session_id.to_owned());
    }
    let (user_message, events) = append_user_event_jsonl(prompt, &mut writer).await?;
    let runtime = runtime_for_config(config, Some(approval_tx)).await?;
    let streaming = StreamingTurnIo {
        event_tx,
        session_id: session_id.to_owned(),
        cancel_token,
    };
    finish_prompt_turn_streaming(
        user_message,
        context,
        &mut writer,
        runtime,
        events,
        streaming,
    )
    .await
}

async fn runtime_for_config(
    config: &AppConfig,
    approval_tx: Option<mpsc::UnboundedSender<PromptApprovalRequest>>,
) -> anyhow::Result<AgentRuntime> {
    let model = resolve_model(config)?;
    let client = resolve_model_client(config, &model)?;
    let tools = tool_registry_for_config(config).await?;
    Ok(AgentRuntime::with_tools(
        agent_config_for_app(model, config, approval_tx)?,
        client,
        tools,
    ))
}

#[cfg(test)]
async fn run_prompt_with_runtime(
    prompt: String,
    context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
) -> anyhow::Result<PromptTurn> {
    let mut writer = SessionEventWriter::jsonl(writer);
    let (user_message, events) = append_user_event(prompt, &mut writer).await?;
    finish_prompt_turn(
        user_message,
        context,
        &mut writer,
        runtime,
        events,
        "test-session".to_owned(),
    )
    .await
}

async fn append_user_event(
    prompt: String,
    writer: &mut SessionEventWriter<'_>,
) -> anyhow::Result<(AgentMessage, Vec<AgentEvent>)> {
    let user_message = AgentMessage::user_text(prompt);
    let user_event = AgentEvent::MessageAppended {
        message: user_message.clone(),
    };
    writer.append_event(&user_event).await?;
    writer.flush().await?;
    Ok((user_message, vec![user_event]))
}

async fn append_user_event_jsonl(
    prompt: String,
    writer: &mut JsonlSessionWriter,
) -> anyhow::Result<(AgentMessage, Vec<AgentEvent>)> {
    let mut writer = SessionEventWriter::jsonl(writer);
    append_user_event(prompt, &mut writer).await
}

async fn finish_prompt_turn(
    user_message: AgentMessage,
    mut context: AgentContext,
    writer: &mut SessionEventWriter<'_>,
    runtime: AgentRuntime,
    mut events: Vec<AgentEvent>,
    session_id: String,
) -> anyhow::Result<PromptTurn> {
    let mut assistant_text = String::new();
    let turn_events = runtime
        .run_turn(&mut context, user_message.clone())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    for event in turn_events {
        let is_duplicate_user_message = matches!(
            &event,
            AgentEvent::MessageAppended { message } if message == &user_message
        );
        if is_duplicate_user_message {
            events.push(event);
            continue;
        }
        if let AgentEvent::MessageAppended { message } = &event
            && matches!(message, AgentMessage::Assistant { .. })
        {
            assistant_text.push_str(&message_text(message));
        }
        writer.append_event(&event).await?;
        events.push(event);
    }
    writer.flush().await?;

    Ok(PromptTurn {
        session_id,
        events,
        assistant_text,
    })
}

enum SessionEventWriter<'a> {
    Jsonl(&'a mut JsonlSessionWriter),
    Memory,
}

impl<'a> SessionEventWriter<'a> {
    fn jsonl(writer: &'a mut JsonlSessionWriter) -> Self {
        Self::Jsonl(writer)
    }

    fn memory() -> Self {
        Self::Memory
    }

    async fn append_event(&mut self, event: &AgentEvent) -> anyhow::Result<()> {
        match self {
            Self::Jsonl(writer) => writer
                .append_event(event)
                .await
                .map_err(anyhow::Error::from),
            Self::Memory => Ok(()),
        }
    }

    async fn flush(&mut self) -> anyhow::Result<()> {
        match self {
            Self::Jsonl(writer) => writer.flush().await.map_err(anyhow::Error::from),
            Self::Memory => Ok(()),
        }
    }
}

struct StreamingTurnIo {
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    session_id: String,
    cancel_token: CancellationToken,
}

async fn finish_prompt_turn_streaming(
    user_message: AgentMessage,
    mut context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
    initial_events: Vec<AgentEvent>,
    streaming: StreamingTurnIo,
) -> anyhow::Result<PromptTurn> {
    let mut events = Vec::new();
    for event in initial_events {
        let _ = streaming.event_tx.send(Ok(event.clone()));
        events.push(event);
    }

    let mut assistant_text = String::new();
    let mut stream =
        runtime.run_turn_with_cancel(&mut context, user_message.clone(), streaming.cancel_token);
    while let Some(event) = stream.next().await {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                let message = error.to_string();
                let _ = streaming
                    .event_tx
                    .send(Err(anyhow::anyhow!(message.clone())));
                anyhow::bail!(message);
            }
        };
        let is_duplicate_user_message = matches!(
            &event,
            AgentEvent::MessageAppended { message } if message == &user_message
        );
        if !is_duplicate_user_message {
            if let AgentEvent::MessageAppended { message } = &event
                && matches!(message, AgentMessage::Assistant { .. })
            {
                assistant_text.push_str(&message_text(message));
            }
            writer.append_event(&event).await?;
        }
        let _ = streaming.event_tx.send(Ok(event.clone()));
        events.push(event);
    }
    writer.flush().await?;

    Ok(PromptTurn {
        session_id: streaming.session_id,
        events,
        assistant_text,
    })
}

fn agent_config_for_app(
    model: ModelSpec,
    config: &AppConfig,
    approval_tx: Option<mpsc::UnboundedSender<PromptApprovalRequest>>,
) -> anyhow::Result<AgentConfig> {
    let mut agent_config = AgentConfig::for_model(model)
        .with_tool_permission_policy(config.permissions.clone())
        .with_queue_modes(
            config.runtime.steering_queue_mode,
            config.runtime.follow_up_queue_mode,
        )
        .with_tool_execution_mode(config.runtime.tool_execution_mode)
        .with_workspace_root(&config.project_dir)?;
    agent_config.temperature = config.runtime.temperature;
    agent_config.max_tokens = config.runtime.max_tokens;
    agent_config.reasoning_effort = config.runtime.reasoning_effort;
    agent_config.replay_reasoning = config.runtime.replay_reasoning;
    if let Some(system_prompt) = resources::load_system_prompt(
        &config.project_dir,
        config.system_prompt.as_deref(),
        &config.append_system_prompt,
        &config.skill_paths,
        config.no_skills,
        config.no_context_files,
        config.project_trusted,
    )? {
        agent_config = agent_config.with_system_prompt(system_prompt);
    }
    if let Some(compaction) = &config.runtime.compaction {
        let model_max_context_tokens = agent_config.model.capabilities.max_context_tokens;
        agent_config = agent_config.with_compaction(CompactionSettings {
            enabled: compaction.enabled,
            max_estimated_tokens: effective_compaction_max_estimated_tokens(
                compaction.max_estimated_tokens,
                model_max_context_tokens,
            ),
            keep_recent_messages: compaction.keep_recent_messages,
        });
    }
    if config.approve {
        agent_config = agent_config.with_approval_handler(|_| PermissionDecision::Allow);
    } else if config.no_approve {
        agent_config = agent_config.with_approval_handler(|_| PermissionDecision::Deny);
    } else if let Some(approval_tx) = approval_tx {
        agent_config = agent_config.with_async_approval_handler(move |request| {
            let approval_tx = approval_tx.clone();
            async move {
                let (decision_tx, decision_rx) = oneshot::channel();
                let id = request.id.clone();
                if approval_tx
                    .send(PromptApprovalRequest { id, decision_tx })
                    .is_err()
                {
                    return PermissionDecision::Deny;
                }
                decision_rx.await.unwrap_or(PermissionDecision::Deny)
            }
        });
    }
    Ok(agent_config)
}

const LEGACY_DEFAULT_COMPACTION_MAX_ESTIMATED_TOKENS: usize = 32_000;
const MODEL_CONTEXT_COMPACTION_NUMERATOR: usize = 4;
const MODEL_CONTEXT_COMPACTION_DENOMINATOR: usize = 5;

fn effective_compaction_max_estimated_tokens(
    configured_max_estimated_tokens: usize,
    model_max_context_tokens: Option<u32>,
) -> usize {
    if configured_max_estimated_tokens != LEGACY_DEFAULT_COMPACTION_MAX_ESTIMATED_TOKENS {
        return configured_max_estimated_tokens;
    }
    let Some(model_max_context_tokens) = model_max_context_tokens else {
        return configured_max_estimated_tokens;
    };
    let model_threshold = model_max_context_tokens as usize * MODEL_CONTEXT_COMPACTION_NUMERATOR
        / MODEL_CONTEXT_COMPACTION_DENOMINATOR;
    configured_max_estimated_tokens.max(model_threshold)
}

async fn tool_registry_for_config(config: &AppConfig) -> anyhow::Result<ToolRegistry> {
    let filters = &config.tool_filters;
    let mut registry = if filters.no_tools && filters.allow.is_empty()
        || (filters.no_builtin_tools && filters.allow.is_empty())
    {
        ToolRegistry::new()
    } else {
        ToolRegistry::with_builtin_tools()
    };
    if !(filters.no_tools && filters.allow.is_empty()) {
        extension_tools::register_enabled_extension_tools(
            &mut registry,
            &extension_tools::default_extension_root(&config.project_dir),
            &extension_tools::default_extension_state_path(&config.project_dir),
            &config.extension_paths,
            config.no_extensions,
        )
        .await?;
        for server in config.mcp.servers.iter().filter(|server| server.enabled) {
            register_mcp_server(&mut registry, server).await?;
        }
    }
    apply_tool_filters(&mut registry, config);
    Ok(registry)
}

fn apply_tool_filters(registry: &mut ToolRegistry, config: &AppConfig) {
    let filters = &config.tool_filters;
    if filters.no_tools && filters.allow.is_empty() {
        return;
    }
    if !filters.allow.is_empty() {
        let allowed = filters.allow.iter().cloned().collect();
        registry.retain_named(&allowed);
    }
    if !filters.exclude.is_empty() {
        let excluded = filters.exclude.iter().cloned().collect();
        registry.remove_named(&excluded);
    }
}

async fn register_mcp_server(
    registry: &mut ToolRegistry,
    server: &McpServerConfig,
) -> anyhow::Result<()> {
    let adapter = mcp_adapter_for_server(server)?;
    let provider = McpToolProvider::discover_dyn(&server.id, adapter)
        .await
        .with_context(|| format!("failed to discover MCP tools from {}", server.id))?;
    provider.register_into(registry);
    Ok(())
}

fn enabled_mcp_server<'a>(
    config: &'a AppConfig,
    server_id: &str,
) -> anyhow::Result<&'a McpServerConfig> {
    let server = config
        .mcp
        .servers
        .iter()
        .find(|server| server.id == server_id)
        .with_context(|| format!("MCP server {server_id} is not configured"))?;
    anyhow::ensure!(server.enabled, "MCP server {server_id} is disabled");
    Ok(server)
}

fn mcp_adapter_for_server(server: &McpServerConfig) -> anyhow::Result<Arc<dyn McpToolAdapter>> {
    match server.transport.as_str() {
        "stdio" => {
            let command = server
                .command
                .clone()
                .with_context(|| format!("missing MCP command for {}", server.id))?;
            Ok(Arc::new(McpStdioToolAdapter::new(McpStdioConfig {
                command,
                args: server.args.clone(),
                env: server
                    .env
                    .iter()
                    .fold(BTreeMap::new(), |mut env, (key, value)| {
                        env.insert(key.clone(), value.clone());
                        env
                    }),
            })))
        }
        "http" | "sse" => {
            let url = server
                .url
                .clone()
                .with_context(|| format!("missing MCP url for {}", server.id))?;
            Ok(Arc::new(McpHttpToolAdapter::new(McpHttpConfig {
                url,
                headers: server.headers.iter().fold(
                    BTreeMap::new(),
                    |mut headers, (key, value)| {
                        headers.insert(key.clone(), value.clone());
                        headers
                    },
                ),
            })))
        }
        other => anyhow::bail!("unsupported MCP transport for {}: {other}", server.id),
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct McpProcessState {
    #[serde(default)]
    servers: Vec<McpProcessRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpProcessRecord {
    id: String,
    child_pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    server_pid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process_group_id: Option<u32>,
}

fn key_value_pairs(values: Vec<String>, flag: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let mut pairs = BTreeMap::new();
    for value in values {
        let Some((key, value)) = value.split_once('=') else {
            anyhow::bail!("{flag} values must use KEY=VALUE");
        };
        let key = key.trim();
        anyhow::ensure!(!key.is_empty(), "{flag} key must not be empty");
        pairs.insert(key.to_owned(), value.trim().to_owned());
    }
    Ok(pairs)
}

fn mcp_state_path(config: &AppConfig) -> PathBuf {
    config.project_dir.join(".neo/mcp-state.toml")
}

fn read_mcp_process_state(config: &AppConfig) -> anyhow::Result<McpProcessState> {
    let path = mcp_state_path(config);
    let content = fs::read_to_string(&path).unwrap_or_default();
    if content.trim().is_empty() {
        return Ok(McpProcessState::default());
    }
    toml::from_str(&content)
        .with_context(|| format!("failed to parse MCP process state {}", path.display()))
}

fn write_mcp_process_state(config: &AppConfig, state: &McpProcessState) -> anyhow::Result<()> {
    let path = mcp_state_path(config);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        toml::to_string_pretty(state)
            .with_context(|| format!("failed to serialize MCP process state {}", path.display()))?,
    )?;
    Ok(())
}

fn wait_for_mcp_server_pid(server: &McpServerConfig, timeout: Duration) -> Option<String> {
    let pid_file = server.env.get("MCP_PID_FILE").map(PathBuf::from)?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(pid) = fs::read_to_string(&pid_file) {
            let pid = pid.trim();
            if !pid.is_empty() {
                return Some(pid.to_owned());
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

fn terminate_mcp_process(record: &McpProcessRecord) {
    #[cfg(unix)]
    let target = record
        .process_group_id
        .map_or_else(|| record.child_pid.to_string(), |pgid| format!("-{pgid}"));
    #[cfg(not(unix))]
    let target = record.child_pid.to_string();

    let _ = std::process::Command::new("kill")
        .args(["-TERM", &target])
        .status();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !process_exists(&target) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::process::Command::new("kill")
        .args(["-KILL", &target])
        .status();
}

fn process_exists(target: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", target])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
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

async fn exact_session_path(
    session_id: &str,
    config: &AppConfig,
) -> anyhow::Result<std::path::PathBuf> {
    neo_agent_core::session::validate_session_id(session_id)
        .with_context(|| format!("invalid session id {session_id:?}"))?;
    tokio::fs::create_dir_all(&config.sessions_dir)
        .await
        .with_context(|| {
            format!(
                "failed to create sessions directory {}",
                config.sessions_dir.display()
            )
        })?;
    Ok(config.sessions_dir.join(format!("{session_id}.jsonl")))
}

fn session_id_from_path(path: &Path) -> anyhow::Result<String> {
    path.file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_owned)
        .with_context(|| format!("invalid session path {}", path.display()))
}

fn latest_session_id(config: &AppConfig) -> anyhow::Result<String> {
    let mut latest: Option<(std::time::SystemTime, String)> = None;
    let entries = std::fs::read_dir(&config.sessions_dir).with_context(|| {
        format!(
            "failed to read sessions directory {}",
            config.sessions_dir.display()
        )
    })?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("jsonl") {
            continue;
        }
        let Ok(session_id) = session_id_from_path(&path) else {
            continue;
        };
        if neo_agent_core::session::validate_session_id(&session_id).is_err() {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let should_replace = latest.as_ref().is_none_or(|(latest_modified, latest_id)| {
            modified > *latest_modified || (modified == *latest_modified && session_id > *latest_id)
        });
        if should_replace {
            latest = Some((modified, session_id));
        }
    }

    latest
        .map(|(_, session_id)| session_id)
        .with_context(|| format!("no sessions found in {}", config.sessions_dir.display()))
}

fn resolve_model(config: &AppConfig) -> anyhow::Result<ModelSpec> {
    let registry = model_registry_for_config(config)?;
    let models = registry.list();
    let candidates = if matches!(config.model_selection, config::ModelSelection::Explicit) {
        models
    } else {
        config::scoped_models(models.iter(), &config.model_scope)
    };
    if !config.model_scope.is_empty() && candidates.is_empty() {
        anyhow::bail!(
            "no models match --models {}; run `neo --list-models` for supported catalog entries",
            config.model_scope.join(",")
        );
    }
    candidates
        .into_iter()
        .find(|model| {
            model.provider.0 == config.default_provider && model.model == config.default_model
        })
        .with_context(|| {
            format!(
                "unknown model {}/{}; run `neo models list` for supported catalog entries",
                config.default_provider, config.default_model
            )
        })
}

pub(crate) fn model_registry_for_config(config: &AppConfig) -> anyhow::Result<ModelRegistry> {
    let mut registry = ModelRegistry::seeded();
    for path in &config.model_catalogs {
        registry
            .load_catalog_path(path)
            .map_err(anyhow::Error::from)?;
    }
    Ok(registry)
}

fn resolve_model_client(
    config: &AppConfig,
    model: &ModelSpec,
) -> anyhow::Result<Arc<dyn ModelClient>> {
    const RESOLVED_API_KEY_ENV: &str = "__NEO_RESOLVED_API_KEY";
    let mut registry = ProviderRegistry::production();
    apply_configured_provider_overrides(&mut registry, config);
    if let Some(mut provider) = provider_with_invocation_overrides(config, &model.provider.0) {
        let credential = resolve_provider_credential(config, &provider);
        let mut env = env::vars().collect::<BTreeMap<_, _>>();
        if let Some(credential) = credential {
            provider.api_key_env_vars = vec![RESOLVED_API_KEY_ENV.to_owned()];
            env.insert(
                RESOLVED_API_KEY_ENV.to_owned(),
                credential.secret().to_owned(),
            );
        }
        registry.register(provider);
        return registry
            .resolver_from(env)
            .resolve(model)
            .map_err(anyhow::Error::from);
    }
    registry
        .resolver()
        .resolve(model)
        .map_err(anyhow::Error::from)
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

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use neo_agent_core::{
        AgentConfig, AgentEvent, AgentMessage, ApprovalRequest, CompactionSettings, Content,
        PermissionDecision, PermissionOperation, PermissionPolicy, QueueMode,
        StopReason as AgentStopReason, ToolExecutionMode,
        session::{JsonlSessionReader, JsonlSessionWriter},
    };
    use neo_ai::{
        AiStreamEvent, ApiKind, ChatMessage, ContentPart, ModelCapabilities, ModelSpec, ProviderId,
        StopReason, providers::fake::FakeModelClient,
    };

    use super::{
        PromptApprovalRequest, StableJsonState, agent_config_for_app, run_prompt_with_runtime,
    };
    use crate::config::{
        AppConfig, Defaults, McpConfig, ModelSelection, RuntimeCompactionConfig, RuntimeConfig,
        ToolFilterConfig, TuiConfig,
    };

    #[test]
    fn agent_config_for_app_applies_runtime_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_base: None,
            api_key: None,
            api_key_env: None,
            providers: BTreeMap::new(),
            model_catalogs: Vec::new(),
            model_scope: Vec::new(),
            model_selection: ModelSelection::Default,
            sessions_dir: temp.path().join(".neo/sessions"),
            permissions: PermissionPolicy::default(),
            defaults: Defaults {
                mode: "print".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: Some(0.35),
                max_tokens: Some(512),
                reasoning_effort: Some(neo_ai::ReasoningEffort::High),
                replay_reasoning: true,
                steering_queue_mode: QueueMode::OneAtATime,
                follow_up_queue_mode: QueueMode::OneAtATime,
                tool_execution_mode: ToolExecutionMode::Sequential,
                compaction: Some(RuntimeCompactionConfig {
                    enabled: true,
                    max_estimated_tokens: 16_000,
                    keep_recent_messages: 24,
                }),
            },
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            approve: false,
            no_approve: false,
            prompt_templates: Vec::new(),
            skill_paths: Vec::new(),
            extension_paths: Vec::new(),
            no_extensions: false,
            configured_prompt_templates: Vec::new(),
            no_prompt_templates: false,
            no_skills: false,
            no_context_files: false,
            offline: false,
            system_prompt: None,
            append_system_prompt: Vec::new(),
            tool_filters: ToolFilterConfig::default(),
            project_trusted: true,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::OpenAiResponses,
            capabilities: ModelCapabilities::tool_chat(),
        };

        let agent_config = agent_config_for_app(model, &config, None).expect("agent config");

        assert_eq!(agent_config.temperature, Some(0.35));
        assert_eq!(agent_config.max_tokens, Some(512));
        assert_eq!(
            agent_config.reasoning_effort,
            Some(neo_ai::ReasoningEffort::High)
        );
        assert_eq!(agent_config.steering_queue_mode, QueueMode::OneAtATime);
        assert_eq!(agent_config.follow_up_queue_mode, QueueMode::OneAtATime);
        assert_eq!(
            agent_config.tool_execution_mode,
            ToolExecutionMode::Sequential
        );
        assert_eq!(
            agent_config.compaction,
            Some(CompactionSettings {
                enabled: true,
                max_estimated_tokens: 16_000,
                keep_recent_messages: 24,
            })
        );
        assert!(agent_config.workspace_root.is_some());
    }

    #[test]
    fn stable_json_maps_compaction_lifecycle_events() {
        let mut state = StableJsonState::default();

        assert_eq!(
            state.map_event(&AgentEvent::CompactionStarted {
                reason: neo_agent_core::CompactionReason::Threshold,
                tokens_before: 12_345,
                message_count: 8,
            }),
            vec![serde_json::json!({
                "type": "compaction_start",
                "reason": "threshold",
                "tokensBefore": 12_345,
                "messageCount": 8,
            })]
        );
        assert_eq!(
            state.map_event(&AgentEvent::CompactionProgress {
                phase: neo_agent_core::CompactionPhase::Summarizing,
                percent: 70,
            }),
            vec![serde_json::json!({
                "type": "compaction_update",
                "phase": "summarizing",
                "percent": 70,
            })]
        );
        assert_eq!(
            state.map_event(&AgentEvent::CompactionApplied {
                summary: neo_agent_core::CompactionSummary {
                    summary: "Older context summarized.".to_owned(),
                    tokens_before: 12_345,
                    first_kept_message_index: 4,
                },
            }),
            vec![serde_json::json!({
                "type": "compaction_end",
                "reason": "threshold",
                "result": {
                    "summary": "Older context summarized.",
                    "tokens_before": 12_345,
                    "first_kept_message_index": 4,
                },
                "aborted": false,
                "willRetry": false,
            })]
        );
    }

    #[test]
    fn agent_config_for_app_scales_legacy_default_compaction_to_model_context_window() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "large-context-model".to_owned(),
            default_provider: "anthropic".to_owned(),
            api_base: None,
            api_key: None,
            api_key_env: None,
            providers: BTreeMap::new(),
            model_catalogs: Vec::new(),
            model_scope: Vec::new(),
            model_selection: ModelSelection::Default,
            sessions_dir: temp.path().join(".neo/sessions"),
            permissions: PermissionPolicy::default(),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
                replay_reasoning: true,
                steering_queue_mode: QueueMode::All,
                follow_up_queue_mode: QueueMode::All,
                tool_execution_mode: ToolExecutionMode::Parallel,
                compaction: Some(RuntimeCompactionConfig {
                    enabled: true,
                    max_estimated_tokens: 32_000,
                    keep_recent_messages: 20,
                }),
            },
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            approve: false,
            no_approve: false,
            prompt_templates: Vec::new(),
            skill_paths: Vec::new(),
            extension_paths: Vec::new(),
            no_extensions: false,
            configured_prompt_templates: Vec::new(),
            no_prompt_templates: false,
            no_skills: false,
            no_context_files: false,
            offline: false,
            system_prompt: None,
            append_system_prompt: Vec::new(),
            tool_filters: ToolFilterConfig::default(),
            project_trusted: true,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("anthropic".to_owned()),
            model: "large-context-model".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat().with_max_context_tokens(1_000_000),
        };

        let agent_config = agent_config_for_app(model, &config, None).expect("agent config");

        assert_eq!(
            agent_config.compaction,
            Some(CompactionSettings {
                enabled: true,
                max_estimated_tokens: 800_000,
                keep_recent_messages: 20,
            })
        );
    }

    #[test]
    fn agent_config_for_app_keeps_explicit_custom_compaction_threshold() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "large-context-model".to_owned(),
            default_provider: "anthropic".to_owned(),
            api_base: None,
            api_key: None,
            api_key_env: None,
            providers: BTreeMap::new(),
            model_catalogs: Vec::new(),
            model_scope: Vec::new(),
            model_selection: ModelSelection::Default,
            sessions_dir: temp.path().join(".neo/sessions"),
            permissions: PermissionPolicy::default(),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
                replay_reasoning: true,
                steering_queue_mode: QueueMode::All,
                follow_up_queue_mode: QueueMode::All,
                tool_execution_mode: ToolExecutionMode::Parallel,
                compaction: Some(RuntimeCompactionConfig {
                    enabled: true,
                    max_estimated_tokens: 12_000,
                    keep_recent_messages: 16,
                }),
            },
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            approve: false,
            no_approve: false,
            prompt_templates: Vec::new(),
            skill_paths: Vec::new(),
            extension_paths: Vec::new(),
            no_extensions: false,
            configured_prompt_templates: Vec::new(),
            no_prompt_templates: false,
            no_skills: false,
            no_context_files: false,
            offline: false,
            system_prompt: None,
            append_system_prompt: Vec::new(),
            tool_filters: ToolFilterConfig::default(),
            project_trusted: true,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("anthropic".to_owned()),
            model: "large-context-model".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat().with_max_context_tokens(1_000_000),
        };

        let agent_config = agent_config_for_app(model, &config, None).expect("agent config");

        assert_eq!(
            agent_config.compaction,
            Some(CompactionSettings {
                enabled: true,
                max_estimated_tokens: 12_000,
                keep_recent_messages: 16,
            })
        );
    }

    #[tokio::test]
    async fn agent_config_for_app_async_approval_channel_waits_for_ui_decision() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_base: None,
            api_key: None,
            api_key_env: None,
            providers: BTreeMap::new(),
            model_catalogs: Vec::new(),
            model_scope: Vec::new(),
            model_selection: ModelSelection::Default,
            sessions_dir: temp.path().join(".neo/sessions"),
            permissions: PermissionPolicy::default(),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            approve: false,
            no_approve: false,
            prompt_templates: Vec::new(),
            skill_paths: Vec::new(),
            extension_paths: Vec::new(),
            no_extensions: false,
            configured_prompt_templates: Vec::new(),
            no_prompt_templates: false,
            no_skills: false,
            no_context_files: false,
            offline: false,
            system_prompt: None,
            append_system_prompt: Vec::new(),
            tool_filters: ToolFilterConfig::default(),
            project_trusted: true,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::OpenAiResponses,
            capabilities: ModelCapabilities::tool_chat(),
        };
        let (approval_tx, mut approval_rx) = tokio::sync::mpsc::unbounded_channel();
        let agent_config =
            agent_config_for_app(model, &config, Some(approval_tx)).expect("agent config");
        let handler = agent_config
            .async_approval_handler
            .expect("async approval handler");

        let decision = tokio::spawn(handler(ApprovalRequest {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: PermissionOperation::Tool,
            subject: "write".to_owned(),
            arguments: serde_json::json!({"path": "approved.txt"}),
        }));
        let PromptApprovalRequest { id, decision_tx } =
            approval_rx.recv().await.expect("approval waiter");

        assert_eq!(id, "tool-1");
        decision_tx
            .send(PermissionDecision::Allow)
            .expect("send decision");
        assert_eq!(
            decision.await.expect("approval task joins"),
            PermissionDecision::Allow
        );
    }

    #[tokio::test]
    async fn run_prompt_with_runtime_appends_continuation_to_existing_session_context() {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_path = temp.path().join("alpha.jsonl");
        let mut seed = JsonlSessionWriter::create(&session_path)
            .await
            .expect("create session");
        seed.append_event(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("hello"),
        })
        .await
        .expect("append user");
        seed.append_event(&AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("hi back")],
                Vec::new(),
                AgentStopReason::EndTurn,
            ),
        })
        .await
        .expect("append assistant");
        seed.append_event(&AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: AgentStopReason::EndTurn,
        })
        .await
        .expect("append turn finish");
        seed.flush().await.expect("flush seed");

        let context = JsonlSessionReader::replay_context(&session_path)
            .await
            .expect("replay context");
        let fake = FakeModelClient::new(vec![
            AiStreamEvent::MessageStart {
                id: "msg-2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "continued answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);
        let runtime =
            super::AgentRuntime::new(AgentConfig::for_model(fake_model()), Arc::new(fake.clone()));
        let mut writer = JsonlSessionWriter::open_append(&session_path)
            .await
            .expect("append session");

        let turn = run_prompt_with_runtime("continue".to_owned(), context, &mut writer, runtime)
            .await
            .expect("run continuation");

        assert_eq!(turn.assistant_text, "continued answer");
        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        let contents = requests[0]
            .messages
            .iter()
            .map(chat_message_text)
            .collect::<Vec<_>>();
        assert_eq!(contents, vec!["hello", "hi back", "continue"]);

        let messages = JsonlSessionReader::replay_messages(&session_path)
            .await
            .expect("replay appended messages");
        assert_eq!(messages.len(), 4);
        assert!(matches!(
            &messages[2],
            AgentMessage::User { content } if content[0].as_text() == Some("continue")
        ));
        assert!(matches!(
            &messages[3],
            AgentMessage::Assistant { content, .. }
                if content[0].as_text() == Some("continued answer")
        ));
    }

    fn fake_model() -> ModelSpec {
        ModelSpec {
            provider: ProviderId("test-provider".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::Local,
            capabilities: ModelCapabilities::tool_chat(),
        }
    }

    fn chat_message_text(message: &ChatMessage) -> String {
        let content = match message {
            ChatMessage::System { content }
            | ChatMessage::User { content }
            | ChatMessage::Assistant { content, .. }
            | ChatMessage::ToolResult { content, .. } => content,
        };
        content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                ContentPart::Thinking { .. } | ContentPart::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

use std::collections::BTreeMap;

use futures::{StreamExt, future, stream};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};

use super::common::error::{ProviderError, error_body_excerpt};
use super::common::helpers::{reject_images, rounded_f64, token_usage_from};

use crate::{
    AiError, AiStreamEvent, ChatMessage, ChatRequest, ContentPart, ImageData, ModelClient,
    ReasoningEffort, StopReason, TokenUsage, ToolSpec,
};

#[derive(Clone)]
pub struct AnthropicMessagesClient {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl AnthropicMessagesClient {
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            client: reqwest::Client::new(),
        }
    }

    async fn open_response(&self, request: ChatRequest) -> Result<reqwest::Response, AiError> {
        let attempts = request.options.retries.unwrap_or(0).saturating_add(1);
        let mut last_error = None;

        for attempt in 0..attempts {
            match self.open_response_once(&request).await {
                Ok(response) => return Ok(response),
                Err(err) if attempt + 1 < attempts && err.is_retryable() => {
                    last_error = Some(err);
                }
                Err(err) => return Err(err.into_ai_error()),
            }
        }

        Err(last_error.map_or_else(
            || AiError::Stream("provider request failed without an error".to_owned()),
            ProviderError::into_ai_error,
        ))
    }

    async fn open_response_once(
        &self,
        request: &ChatRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let url = format!("{}/messages", self.base_url);
        let body = request_body(request)?;
        let mut builder = self
            .client
            .post(url)
            .headers(headers(&self.api_key, &request.options.headers)?)
            .json(&body);

        if let Some(timeout) = request.options.timeout {
            builder = builder.timeout(timeout);
        }

        let response = builder.send().await.map_err(ProviderError::Transport)?;
        let status = response.status();
        if !status.is_success() {
            let status = status.as_u16();
            let body = response
                .text()
                .await
                .unwrap_or_else(|err| format!("failed to read error body: {err}"));
            return Err(ProviderError::HttpStatus {
                status,
                body: Some(error_body_excerpt(&body)),
            });
        }

        Ok(response)
    }
}

impl ModelClient for AnthropicMessagesClient {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        let client = self.clone();
        stream::once(async move { client.open_response(request).await })
            .flat_map(|result| match result {
                Ok(response) => stream_response(response),
                Err(err) => stream::iter(vec![Err(err)]).boxed(),
            })
            .boxed()
    }
}

fn headers(
    api_key: &str,
    extra_headers: &BTreeMap<String, String>,
) -> Result<HeaderMap, ProviderError> {
    let mut headers = HeaderMap::new();
    let api_key = HeaderValue::from_str(api_key)
        .map_err(|err| ProviderError::Header(format!("invalid x-api-key header: {err}")))?;
    headers.insert(HeaderName::from_static("x-api-key"), api_key);
    headers.insert(
        HeaderName::from_static("anthropic-version"),
        HeaderValue::from_static("2023-06-01"),
    );

    for (name, value) in extra_headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|err| ProviderError::Header(format!("invalid header name {name}: {err}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|err| ProviderError::Header(format!("invalid header value {name}: {err}")))?;
        headers.insert(name, value);
    }

    Ok(headers)
}

fn request_body(request: &ChatRequest) -> Result<Value, ProviderError> {
    let mut body = json!({
        "model": request.model.model,
        "stream": true,
        // Anthropic requires `max_tokens`. When neither the user nor the model
        // catalog supplied one, use a coding-agent-friendly default rather than
        // the chat-era 4096 which truncates long edits/plans mid-stream.
        "max_tokens": request.options.max_tokens.unwrap_or(32_000),
        "messages": message_bodies(&request.messages, request.options.replay_reasoning)?,
    });

    let system = request
        .messages
        .iter()
        .filter_map(|message| match message {
            ChatMessage::System { content } => Some(content_text(content, "system")),
            _ => None,
        })
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(request.tools.iter().map(tool_body).collect());
    }
    if let Some(reasoning_effort) = request.options.reasoning_effort {
        body["thinking"] = json!({
            "type": "enabled",
            "budget_tokens": thinking_budget_tokens(reasoning_effort),
            "display": "summarized",
        });
    } else if let Some(temperature) = request.options.temperature {
        body["temperature"] = json!(rounded_f64(temperature));
    }
    if !request.options.metadata.is_empty()
        && let Some(user_id) = request.options.metadata.get("user_id")
    {
        body["metadata"] = json!({ "user_id": user_id });
    }
    Ok(body)
}

const fn thinking_budget_tokens(effort: ReasoningEffort) -> u32 {
    match effort {
        ReasoningEffort::Minimal | ReasoningEffort::Low => 1_024,
        ReasoningEffort::Medium => 2_048,
        ReasoningEffort::High => 8_192,
        ReasoningEffort::XHigh => 16_384,
    }
}

fn message_body(
    message: &ChatMessage,
    replay_reasoning: bool,
) -> Option<Result<Value, ProviderError>> {
    match message {
        ChatMessage::System { .. } => None,
        ChatMessage::User { content } => Some(user_content(content).map(|content| {
            json!({
                "role": "user",
                "content": content,
            })
        })),
        ChatMessage::Assistant {
            content,
            tool_calls,
        } => Some(
            assistant_content(content, tool_calls, replay_reasoning).map(|content| {
                json!({
                    "role": "assistant",
                    "content": content,
                })
            }),
        ),
        ChatMessage::ToolResult {
            tool_call_id,
            content,
            is_error,
        } => Some(
            tool_result_block(tool_call_id, content, *is_error).map(|block| {
                json!({
                    "role": "user",
                    "content": [block],
                })
            }),
        ),
    }
}

fn message_bodies(
    messages: &[ChatMessage],
    replay_reasoning: bool,
) -> Result<Vec<Value>, ProviderError> {
    let mut bodies = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        if let ChatMessage::ToolResult { .. } = &messages[index] {
            let mut content = Vec::new();
            while let Some(ChatMessage::ToolResult {
                tool_call_id,
                content: result_content,
                is_error,
            }) = messages.get(index)
            {
                content.push(tool_result_block(tool_call_id, result_content, *is_error)?);
                index += 1;
            }
            bodies.push(json!({
                "role": "user",
                "content": content,
            }));
            continue;
        }

        if let Some(body) = message_body(&messages[index], replay_reasoning) {
            bodies.push(body?);
        }
        index += 1;
    }
    Ok(bodies)
}

fn tool_result_block(
    tool_call_id: &str,
    content: &[ContentPart],
    is_error: bool,
) -> Result<Value, ProviderError> {
    content_text(content, "tool result").map(|content| {
        json!({
            "type": "tool_result",
            "tool_use_id": tool_call_id,
            "content": content,
            "is_error": is_error,
        })
    })
}

fn user_content(content: &[ContentPart]) -> Result<Value, ProviderError> {
    let mut parts = Vec::new();
    for part in content {
        parts.push(content_part_body(part)?);
    }
    Ok(Value::Array(parts))
}

fn assistant_content(
    content: &[ContentPart],
    tool_calls: &[crate::ToolCall],
    replay_reasoning: bool,
) -> Result<Value, ProviderError> {
    let mut parts = Vec::new();
    for part in content {
        match part {
            ContentPart::Thinking {
                text,
                signature,
                redacted: true,
            } if replay_reasoning => {
                let Some(signature) = signature.as_deref().filter(|value| !value.is_empty()) else {
                    return Err(ProviderError::Stream(
                        "Anthropic redacted thinking replay requires a signature".to_owned(),
                    ));
                };
                parts.push(json!({ "type": "redacted_thinking", "data": signature }));
                let _ = text;
            }
            ContentPart::Thinking {
                text,
                signature: Some(signature),
                redacted: false,
            } if replay_reasoning && !text.is_empty() && !signature.is_empty() => {
                parts.push(json!({
                    "type": "thinking",
                    "thinking": text,
                    "signature": signature,
                }));
            }
            ContentPart::Text { text }
            | ContentPart::Thinking {
                text,
                signature: None,
                redacted: false,
            } if !text.is_empty() => {
                parts.push(json!({ "type": "text", "text": text }));
            }
            ContentPart::Image { .. } => {
                return Err(ProviderError::Stream(
                    "Anthropic image content is only supported in user messages, not assistant messages"
                        .to_owned(),
                ));
            }
            ContentPart::Text { .. } | ContentPart::Thinking { .. } => {}
        }
    }
    for tool_call in tool_calls {
        parts.push(json!({
            "type": "tool_use",
            "id": tool_call.id,
            "name": tool_call.name,
            "input": tool_call.arguments,
        }));
    }
    Ok(Value::Array(parts))
}

fn content_part_body(part: &ContentPart) -> Result<Value, ProviderError> {
    Ok(match part {
        ContentPart::Text { text } | ContentPart::Thinking { text, .. } => json!({
            "type": "text",
            "text": text,
        }),
        ContentPart::Image { mime_type, data } => match data {
            ImageData::Base64(data) => json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": mime_type,
                    "data": data,
                },
            }),
            ImageData::Url(_) => {
                return Err(ProviderError::Stream(
                    "Anthropic image URL content is unsupported; provide base64 image data"
                        .to_owned(),
                ));
            }
        },
    })
}

fn content_text(content: &[ContentPart], role: &str) -> Result<String, ProviderError> {
    reject_images(content, "Anthropic", role)?;
    Ok(text_content(content))
}

fn text_content(content: &[ContentPart]) -> String {
    super::collect_text_content(content, false)
}

fn tool_body(tool: &ToolSpec) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.input_schema,
    })
}

enum StreamChunk {
    Data(Result<Vec<u8>, reqwest::Error>),
    End,
}

fn stream_response(
    response: reqwest::Response,
) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
    response
        .bytes_stream()
        .map(|chunk| StreamChunk::Data(chunk.map(|bytes| bytes.to_vec())))
        .chain(stream::once(async { StreamChunk::End }))
        .scan(IncrementalSse::default(), |state, chunk| {
            future::ready(Some(match chunk {
                StreamChunk::Data(Ok(bytes)) => state.push_chunk(&bytes),
                StreamChunk::Data(Err(err)) => {
                    vec![Err(AiError::Stream(format!("transport error: {err}")))]
                }
                StreamChunk::End => state.finish(),
            }))
        })
        .flat_map(stream::iter)
        .boxed()
}

#[derive(Default)]
struct IncrementalSse {
    buffer: Vec<u8>,
    parser: ParseState,
    saw_done: bool,
    stopped: bool,
}

impl IncrementalSse {
    fn push_chunk(&mut self, bytes: &[u8]) -> Vec<Result<AiStreamEvent, AiError>> {
        if self.stopped {
            return Vec::new();
        }

        self.buffer.extend_from_slice(bytes);
        let mut out = Vec::new();

        while let Some((index, delimiter_len)) = find_frame_end(&self.buffer) {
            let frame = self
                .buffer
                .drain(..index + delimiter_len)
                .collect::<Vec<_>>();
            match parse_sse_frame(&frame) {
                Ok(Some(payload)) if payload == "[DONE]" => {
                    self.saw_done = true;
                    self.stopped = true;
                    out.extend(self.finish());
                    break;
                }
                Ok(Some(payload)) => {
                    if let Err(err) = self.ingest_payload(&payload, &mut out) {
                        self.stopped = true;
                        out.push(Err(err));
                        break;
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    self.stopped = true;
                    out.push(Err(err));
                    break;
                }
            }
        }

        out
    }

    fn ingest_payload(
        &mut self,
        payload: &str,
        out: &mut Vec<Result<AiStreamEvent, AiError>>,
    ) -> Result<(), AiError> {
        let value = serde_json::from_str::<Value>(payload)
            .map_err(|err| AiError::Stream(format!("invalid SSE JSON: {err}")))?;
        self.parser.ingest(&value);
        out.extend(self.parser.drain_events().into_iter().map(Ok));
        Ok(())
    }

    fn finish(&mut self) -> Vec<Result<AiStreamEvent, AiError>> {
        if self.parser.is_finished() {
            return Vec::new();
        }

        self.stopped = true;
        if !self.saw_done && !self.parser.saw_terminal() {
            return vec![Err(AiError::Stream("missing SSE done marker".to_owned()))];
        }

        self.parser.finish_events().map_or_else(
            |err| vec![Err(err.into_ai_error())],
            |events| events.into_iter().map(Ok).collect(),
        )
    }
}

fn find_frame_end(buffer: &[u8]) -> Option<(usize, usize)> {
    buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (index, 2))
        .or_else(|| {
            buffer
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| (index, 4))
        })
}

fn parse_sse_frame(frame: &[u8]) -> Result<Option<String>, AiError> {
    let text = std::str::from_utf8(frame)
        .map_err(|err| AiError::Stream(format!("invalid SSE UTF-8: {err}")))?;
    let data = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");
    Ok((!data.is_empty()).then_some(data))
}

struct ParseState {
    events: Vec<AiStreamEvent>,
    started: bool,
    tool_args: BTreeMap<String, String>,
    block_tool_ids: BTreeMap<u64, String>,
    thinking_blocks: BTreeMap<u64, ThinkingBlock>,
    last_stop_reason: StopReason,
    usage: Option<TokenUsage>,
    terminal: bool,
    finished: bool,
}

#[derive(Default)]
struct ThinkingBlock {
    signature: Option<String>,
}

impl Default for ParseState {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            started: false,
            tool_args: BTreeMap::new(),
            block_tool_ids: BTreeMap::new(),
            thinking_blocks: BTreeMap::new(),
            last_stop_reason: StopReason::EndTurn,
            usage: None,
            terminal: false,
            finished: false,
        }
    }
}

impl ParseState {
    fn ingest(&mut self, value: &Value) {
        match value.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                let id = value
                    .get("message")
                    .and_then(|message| message.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("message")
                    .to_owned();
                self.ensure_started(id);
            }
            Some("content_block_start") => self.ingest_block_start(value),
            Some("content_block_delta") => self.ingest_block_delta(value),
            Some("content_block_stop") => self.ingest_block_stop(value),
            Some("message_delta") => self.ingest_message_delta(value),
            Some("message_stop") => {
                self.terminal = true;
            }
            Some("error") => {
                self.last_stop_reason = StopReason::Error;
                self.terminal = true;
            }
            _ => {}
        }
    }

    fn drain_events(&mut self) -> Vec<AiStreamEvent> {
        std::mem::take(&mut self.events)
    }

    const fn is_finished(&self) -> bool {
        self.finished
    }

    const fn saw_terminal(&self) -> bool {
        self.terminal
    }

    fn ensure_started(&mut self, id: String) {
        if self.started {
            return;
        }
        self.events.push(AiStreamEvent::MessageStart { id });
        self.started = true;
    }

    fn ingest_block_start(&mut self, value: &Value) {
        let block = value.get("content_block").unwrap_or(&Value::Null);
        let index = value.get("index").and_then(Value::as_u64).unwrap_or(0);
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => {
                self.ensure_started("message".to_owned());
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_owned();
                self.block_tool_ids.insert(index, id.clone());
                if let Some(name) = block.get("name").and_then(Value::as_str) {
                    self.events.push(AiStreamEvent::ToolCallStart {
                        id,
                        name: name.to_owned(),
                    });
                }
            }
            Some("thinking") => {
                self.start_thinking_block(index);
                if let Some(text) = block.get("thinking").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    self.events.push(AiStreamEvent::ThinkingDelta {
                        text: text.to_owned(),
                    });
                }
            }
            _ => {}
        }
    }

    fn ingest_block_delta(&mut self, value: &Value) {
        let delta = value.get("delta").unwrap_or(&Value::Null);
        match delta.get("type").and_then(Value::as_str) {
            Some("text_delta") => {
                self.ensure_started("message".to_owned());
                if let Some(text) = delta.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    self.events.push(AiStreamEvent::TextDelta {
                        text: text.to_owned(),
                    });
                }
            }
            Some("thinking_delta") => {
                let index = value.get("index").and_then(Value::as_u64).unwrap_or(0);
                self.start_thinking_block(index);
                if let Some(text) = delta.get("thinking").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    self.events.push(AiStreamEvent::ThinkingDelta {
                        text: text.to_owned(),
                    });
                }
            }
            Some("signature_delta") => {
                let index = value.get("index").and_then(Value::as_u64).unwrap_or(0);
                self.start_thinking_block(index);
                if let Some(signature) = delta.get("signature").and_then(Value::as_str) {
                    self.thinking_blocks
                        .get_mut(&index)
                        .expect("thinking block should exist")
                        .signature = Some(signature.to_owned());
                }
            }
            Some("input_json_delta") => {
                let index = value.get("index").and_then(Value::as_u64).unwrap_or(0);
                let id = self
                    .block_tool_ids
                    .get(&index)
                    .cloned()
                    .unwrap_or_else(|| format!("tool-{index}"));
                if let Some(fragment) = delta.get("partial_json").and_then(Value::as_str) {
                    self.tool_args
                        .entry(id.clone())
                        .or_default()
                        .push_str(fragment);
                    self.events.push(AiStreamEvent::ToolCallArgsDelta {
                        id,
                        json_fragment: fragment.to_owned(),
                    });
                }
            }
            _ => {}
        }
    }

    fn ingest_block_stop(&mut self, value: &Value) {
        let index = value.get("index").and_then(Value::as_u64).unwrap_or(0);
        if let Some(block) = self.thinking_blocks.remove(&index) {
            self.events.push(AiStreamEvent::ThinkingEnd {
                signature: block.signature,
                redacted: false,
            });
        }
    }

    fn ingest_message_delta(&mut self, value: &Value) {
        if let Some(reason) = value
            .get("delta")
            .and_then(|delta| delta.get("stop_reason"))
            .and_then(Value::as_str)
        {
            self.last_stop_reason = stop_reason(reason);
        }
        self.usage = value
            .get("usage")
            .and_then(|v| token_usage_from(v, "input_tokens", "output_tokens"))
            .or(self.usage.clone());
    }

    fn finish_events(&mut self) -> Result<Vec<AiStreamEvent>, ProviderError> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;

        let unfinished_thinking = std::mem::take(&mut self.thinking_blocks);
        for block in unfinished_thinking.into_values() {
            self.events.push(AiStreamEvent::ThinkingEnd {
                signature: block.signature,
                redacted: false,
            });
        }

        for (id, arguments) in &self.tool_args {
            let parsed = serde_json::from_str(arguments)
                .map_err(|err| ProviderError::Stream(format!("invalid tool arguments: {err}")))?;
            self.events.push(AiStreamEvent::ToolCallEnd {
                id: id.clone(),
                arguments: parsed,
            });
        }

        if self.started {
            self.events.push(AiStreamEvent::MessageEnd {
                stop_reason: self.last_stop_reason.clone(),
                usage: self.usage.clone(),
            });
        }

        Ok(self.drain_events())
    }

    fn start_thinking_block(&mut self, index: u64) {
        self.ensure_started("message".to_owned());
        if self.thinking_blocks.contains_key(&index) {
            return;
        }

        self.thinking_blocks.insert(index, ThinkingBlock::default());
        self.events.push(AiStreamEvent::ThinkingStart {
            id: format!("thinking:{index}"),
        });
    }
}

fn stop_reason(reason: &str) -> StopReason {
    match reason {
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        _ => StopReason::EndTurn,
    }
}

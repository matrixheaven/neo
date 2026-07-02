use std::collections::{BTreeMap, BTreeSet};

use futures::{StreamExt, future, stream};
use reqwest::header::HeaderMap;
use serde_json::{Value, json};

use super::common::error::{ProviderError, parse_retry_after};
use super::common::helpers::{reject_images, rounded_f64, token_usage_from};
use super::common::sse::{StreamChunk, find_frame_end, parse_sse_frame};

use crate::{
    AiError, AiStreamEvent, ChatMessage, ChatRequest, ContentPart, ImageData, ModelClient,
    ReasoningEffort, StopReason, TokenUsage, ToolSpec,
};

#[derive(Clone)]
pub struct GoogleGenerativeAiClient {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl GoogleGenerativeAiClient {
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            client: reqwest::Client::new(),
        }
    }

    async fn open_response(&self, request: ChatRequest) -> Result<reqwest::Response, AiError> {
        super::common::http::open_response(&request, |req| Box::pin(self.open_response_once(req)))
            .await
    }

    async fn open_response_once(
        &self,
        request: &ChatRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let url = request_url(&self.base_url, &request.model.model, &self.api_key)?;
        let body = request_body(request)?;
        let mut builder = self
            .client
            .post(url)
            .headers(headers(&request.options.headers)?)
            .json(&body);

        if let Some(timeout) = request.options.timeout {
            builder = builder.timeout(timeout);
        }

        let response = builder.send().await.map_err(ProviderError::Transport)?;
        let status = response.status();
        if !status.is_success() {
            let status = status.as_u16();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(parse_retry_after);
            return Err(ProviderError::HttpStatus {
                status,
                body: None,
                retry_after,
            });
        }

        Ok(response)
    }
}

impl ModelClient for GoogleGenerativeAiClient {
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

fn request_url(base_url: &str, model: &str, api_key: &str) -> Result<reqwest::Url, ProviderError> {
    let model = model.strip_prefix("models/").unwrap_or(model);
    let mut url = reqwest::Url::parse(&format!("{base_url}/models/{model}:streamGenerateContent"))
        .map_err(|err| ProviderError::Url(format!("invalid Google Generative AI URL: {err}")))?;
    url.query_pairs_mut()
        .append_pair("alt", "sse")
        .append_pair("key", api_key);
    Ok(url)
}

fn headers(extra_headers: &BTreeMap<String, String>) -> Result<HeaderMap, ProviderError> {
    let mut headers = HeaderMap::new();
    super::common::http::inject_extra_headers(&mut headers, extra_headers)?;
    Ok(headers)
}

fn request_body(request: &ChatRequest) -> Result<Value, ProviderError> {
    let mut body = json!({
        "contents": request
            .messages
            .iter()
            .filter_map(|message| content_body(message, request.options.replay_reasoning))
            .collect::<Result<Vec<_>, _>>()?,
    });

    let system = request
        .messages
        .iter()
        .filter_map(|message| match message {
            ChatMessage::System { content } => Some(text_parts(content)),
            _ => None,
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if !system.is_empty() {
        body["systemInstruction"] = json!({ "parts": system });
    }

    if !request.tools.is_empty() {
        body["tools"] = json!([{ "functionDeclarations": request.tools.iter().map(tool_body).collect::<Vec<_>>() }]);
    }

    let mut generation_config = serde_json::Map::new();
    if let Some(temperature) = request.options.temperature {
        generation_config.insert("temperature".to_owned(), json!(rounded_f64(temperature)));
    }
    if let Some(max_tokens) = request.options.max_tokens {
        generation_config.insert("maxOutputTokens".to_owned(), json!(max_tokens));
    }
    if let Some(reasoning_effort) = request.options.reasoning_effort {
        generation_config.insert(
            "thinkingConfig".to_owned(),
            json!({
                "includeThoughts": true,
                "thinkingBudget": thinking_budget_tokens(reasoning_effort),
            }),
        );
    }
    if !generation_config.is_empty() {
        body["generationConfig"] = Value::Object(generation_config);
    }

    Ok(body)
}

const fn thinking_budget_tokens(effort: ReasoningEffort) -> i32 {
    match effort {
        ReasoningEffort::Minimal | ReasoningEffort::Low => 1_024,
        ReasoningEffort::Medium => 2_048,
        ReasoningEffort::High => 8_192,
        ReasoningEffort::XHigh => 16_384,
    }
}

fn content_body(
    message: &ChatMessage,
    replay_reasoning: bool,
) -> Option<Result<Value, ProviderError>> {
    match message {
        ChatMessage::System { .. } => None,
        ChatMessage::User { content } => Some(content_parts(content, true).map(|parts| {
            json!({
                "role": "user",
                "parts": parts,
            })
        })),
        ChatMessage::Assistant {
            content,
            tool_calls,
        } => {
            let tool_calls = tool_calls.clone();
            Some(
                content_parts(content, replay_reasoning).and_then(move |mut parts| {
                    for tool_call in &tool_calls {
                        let args = serde_json::from_str::<serde_json::Value>(
                            &tool_call.raw_arguments,
                        )
                        .map_err(|err| {
                            ProviderError::Stream(format!(
                                "invalid raw tool arguments for Google replay tool call '{}': {err}",
                                tool_call.id
                            ))
                        })?;
                        parts.push(json!({
                            "functionCall": {
                                "name": tool_call.name,
                                "args": args,
                            },
                        }));
                    }
                    Ok(json!({
                        "role": "model",
                        "parts": parts,
                    }))
                }),
            )
        }
        ChatMessage::ToolResult {
            tool_call_id,
            content,
            is_error,
        } => Some(
            reject_images(content, "Google Generative AI", "tool result").map(|()| {
                json!({
                    "role": "function",
                    "parts": [{
                        "functionResponse": {
                            "name": tool_call_id,
                            "response": {
                                "result": content_text(content),
                                "is_error": is_error,
                            },
                        },
                    }],
                })
            }),
        ),
    }
}

fn content_parts(
    content: &[ContentPart],
    replay_reasoning: bool,
) -> Result<Vec<Value>, ProviderError> {
    let mut parts = Vec::new();
    for part in content {
        if let Some(part) = content_part(part, replay_reasoning) {
            parts.push(part?);
        }
    }
    Ok(parts)
}

fn content_part(
    part: &ContentPart,
    replay_reasoning: bool,
) -> Option<Result<Value, ProviderError>> {
    match part {
        ContentPart::Thinking { .. } if !replay_reasoning => None,
        ContentPart::Text { text } => Some(Ok(json!({ "text": text }))),
        ContentPart::Thinking {
            text,
            signature,
            redacted,
        } => {
            if *redacted {
                return Some(Err(ProviderError::Unsupported(
                    "Google Generative AI cannot replay redacted thinking blocks".to_owned(),
                )));
            }
            let mut part = json!({ "text": text, "thought": true });
            if let Some(signature) = signature
                && !signature.is_empty()
            {
                part["thoughtSignature"] = json!(signature);
            }
            Some(Ok(part))
        }
        ContentPart::Image { mime_type, data } => Some(match data {
            ImageData::Base64(data) => Ok(json!({
                "inlineData": {
                    "mimeType": mime_type,
                    "data": data,
                },
            })),
            ImageData::Url(_) => Err(ProviderError::Unsupported(
                "Google Generative AI image URL content is unsupported; provide base64 image data"
                    .to_owned(),
            )),
        }),
    }
}

fn text_parts(content: &[ContentPart]) -> Result<Vec<Value>, ProviderError> {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } | ContentPart::Thinking { text, .. } => {
                Ok(json!({ "text": text }))
            }
            ContentPart::Image { .. } => Err(ProviderError::Unsupported(
                "Google Generative AI image content is only supported in user/model messages, not system messages"
                    .to_owned(),
            )),
        })
        .collect()
}

fn content_text(content: &[ContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            ContentPart::Thinking { .. } | ContentPart::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_body(tool: &ToolSpec) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "parameters": crate::tool_schema::normalize_tool_schema(&tool.input_schema),
    })
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
                    vec![Err(AiError::Stream {
                        message: format!("transport error: {err}"),
                    })]
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
        let value = serde_json::from_str::<Value>(payload).map_err(|err| AiError::Stream {
            message: format!("invalid SSE JSON: {err}"),
        })?;
        self.parser.ingest(&value)?;
        out.extend(self.parser.drain_events().into_iter().map(Ok));
        Ok(())
    }

    fn finish(&mut self) -> Vec<Result<AiStreamEvent, AiError>> {
        if self.parser.is_finished() {
            return Vec::new();
        }

        self.stopped = true;
        self.parser.finish_events().into_iter().map(Ok).collect()
    }
}

struct ParseState {
    events: Vec<AiStreamEvent>,
    started: bool,
    tool_args: BTreeMap<String, Value>,
    open_tool_ids: BTreeSet<String>,
    next_thought_index: u64,
    last_stop_reason: StopReason,
    usage: Option<TokenUsage>,
    finished: bool,
}

impl Default for ParseState {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            started: false,
            tool_args: BTreeMap::new(),
            open_tool_ids: BTreeSet::new(),
            next_thought_index: 0,
            last_stop_reason: StopReason::EndTurn,
            usage: None,
            finished: false,
        }
    }
}

impl ParseState {
    fn ingest(&mut self, value: &Value) -> Result<(), AiError> {
        self.usage = value
            .get("usageMetadata")
            .and_then(|v| token_usage_from(v, "promptTokenCount", "candidatesTokenCount"))
            .or(self.usage.clone());

        for candidate in value
            .get("candidates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.ingest_candidate(candidate)?;
        }
        Ok(())
    }

    fn drain_events(&mut self) -> Vec<AiStreamEvent> {
        std::mem::take(&mut self.events)
    }

    const fn is_finished(&self) -> bool {
        self.finished
    }

    fn ensure_started(&mut self) {
        if self.started {
            return;
        }
        self.events.push(AiStreamEvent::MessageStart {
            id: "google-generative-ai".to_owned(),
        });
        self.started = true;
    }

    fn ingest_candidate(&mut self, candidate: &Value) -> Result<(), AiError> {
        if let Some(parts) = candidate
            .get("content")
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
        {
            for part in parts {
                self.ingest_part(part)?;
            }
        }

        if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
            self.last_stop_reason = if self.tool_args.is_empty() {
                stop_reason(reason)
            } else {
                StopReason::ToolUse
            };
        }

        Ok(())
    }

    fn ingest_part(&mut self, part: &Value) -> Result<(), AiError> {
        if part
            .get("thought")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            self.ingest_thought_part(part);
            return Ok(());
        }

        if let Some(text) = part.get("text").and_then(Value::as_str)
            && !text.is_empty()
        {
            self.ensure_started();
            self.events.push(AiStreamEvent::TextDelta {
                text: text.to_owned(),
            });
        }

        if let Some(function_call) = part.get("functionCall") {
            self.ingest_function_call(function_call)?;
        }

        Ok(())
    }

    fn ingest_thought_part(&mut self, part: &Value) {
        self.ensure_started();
        let id = format!("google-thought:{}", self.next_thought_index);
        self.next_thought_index = self.next_thought_index.saturating_add(1);
        self.events.push(AiStreamEvent::ThinkingStart { id });
        if let Some(text) = part.get("text").and_then(Value::as_str)
            && !text.is_empty()
        {
            self.events.push(AiStreamEvent::ThinkingDelta {
                text: text.to_owned(),
            });
        }
        self.events.push(AiStreamEvent::ThinkingEnd {
            signature: part
                .get("thoughtSignature")
                .and_then(Value::as_str)
                .map(str::to_owned),
            redacted: false,
        });
    }

    fn ingest_function_call(&mut self, function_call: &Value) -> Result<(), AiError> {
        let name = function_call
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("function")
            .to_owned();
        let args = function_call
            .get("args")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let fragment = serde_json::to_string(&args).map_err(|err| AiError::Stream {
            message: format!("invalid tool arguments: {err}"),
        })?;

        self.ensure_started();
        if self.open_tool_ids.insert(name.clone()) {
            self.events.push(AiStreamEvent::ToolCallStart {
                id: name.clone(),
                name: name.clone(),
            });
        }
        self.tool_args.insert(name.clone(), args);
        self.events.push(AiStreamEvent::ToolCallArgsDelta {
            id: name,
            json_fragment: fragment,
        });
        Ok(())
    }

    fn finish_events(&mut self) -> Vec<AiStreamEvent> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;

        for (id, arguments) in &self.tool_args {
            self.events.push(AiStreamEvent::ToolCallEnd {
                id: id.clone(),
                raw_arguments: arguments.to_string(),
            });
        }

        if self.started {
            self.events.push(AiStreamEvent::MessageEnd {
                stop_reason: self.last_stop_reason.clone(),
                usage: self.usage.clone(),
            });
        }

        self.drain_events()
    }
}

fn stop_reason(reason: &str) -> StopReason {
    match reason {
        "MAX_TOKENS" => StopReason::MaxTokens,
        "SAFETY" | "RECITATION" | "SPII" | "MALFORMED_FUNCTION_CALL" => StopReason::Error,
        _ => StopReason::EndTurn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolCall;

    #[test]
    fn assistant_replay_rejects_invalid_raw_tool_arguments() {
        let result = content_body(
            &ChatMessage::Assistant {
                content: Vec::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_owned(),
                    name: "read".to_owned(),
                    raw_arguments: r#"{"path":"Cargo"#.to_owned(),
                }],
            },
            false,
        )
        .expect("assistant message should produce content");

        let err = result.unwrap_err();
        assert!(
            matches!(err, ProviderError::Stream(message) if message.contains("invalid raw tool arguments"))
        );
    }
}

use futures::{StreamExt, future, stream};
use serde_json::{Value, json};

use crate::providers::common::error::{ProviderError, parse_retry_after};
use crate::providers::common::helpers::{reject_images, rounded_f64, token_usage_from};
use crate::providers::common::sse::{StreamChunk, find_frame_end, parse_sse_frame};

use crate::tool_assembly::{StreamingToolCallAssembler, ToolCallAssemblyEvent, ToolCallChunk};
use crate::{
    AiError, AiStreamEvent, CacheRetention, ChatMessage, ChatRequest, ContentPart, ModelClient,
    ReasoningEffort, StopReason, TokenUsage, ToolSpec,
};

const EMPTY_STRUCTURED_TOOL_CALLS_MESSAGE: &str =
    "Provider reported tool calls but emitted no structured tool calls";
const DEFAULT_REASONING_KEY: &str = "reasoning_content";
const KNOWN_REASONING_KEYS: &[&str] = &["reasoning_content", "reasoning_details", "reasoning"];

#[derive(Clone)]
pub struct OpenAiCompatibleClient {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiCompatibleClient {
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            client: reqwest::Client::new(),
        }
    }

    async fn open_response(&self, request: ChatRequest) -> Result<reqwest::Response, AiError> {
        crate::providers::common::http::open_response(&request, |req| {
            Box::pin(self.open_response_once(req))
        })
        .await
    }

    async fn open_response_once(
        &self,
        request: &ChatRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = request_body(request)?;
        let mut builder = self
            .client
            .post(url)
            .headers(super::headers(
                &self.api_key,
                &request.options.headers,
                request.options.session_id.as_deref(),
            )?)
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
            let body = response
                .text()
                .await
                .map(|text| crate::providers::common::error::error_body_excerpt(&text))
                .ok();
            return Err(ProviderError::HttpStatus {
                status,
                body,
                retry_after,
            });
        }

        Ok(response)
    }
}

impl ModelClient for OpenAiCompatibleClient {
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

fn request_body(request: &ChatRequest) -> Result<Value, ProviderError> {
    let mut body = json!({
        "model": request.model.model,
        "stream": true,
        "messages": request
            .messages
            .iter()
            .map(message_body)
            .collect::<Result<Vec<_>, _>>()?,
    });

    if !request.tools.is_empty() {
        body["tools"] = Value::Array(request.tools.iter().map(tool_body).collect());
    }
    if let Some(temperature) = request.options.temperature {
        body["temperature"] = json!(rounded_f64(temperature));
    }
    if let Some(max_tokens) = request.options.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if let Some(reasoning_effort) = request.options.reasoning_effort {
        body["reasoning_effort"] = json!(openai_reasoning_effort(reasoning_effort)?);
    } else if request_replays_reasoning(request) {
        body["reasoning_effort"] = json!(openai_reasoning_effort(ReasoningEffort::Medium)?);
    }
    if !request.options.metadata.is_empty() {
        body["metadata"] = json!(request.options.metadata.as_map());
    }
    if let Some(session_id) = &request.options.session_id {
        body["prompt_cache_key"] = json!(session_id);
    }
    match request.options.cache {
        CacheRetention::None => {}
        CacheRetention::Short => {
            body["prompt_cache_retention"] = json!("1h");
        }
        CacheRetention::Long => {
            body["prompt_cache_retention"] = json!("24h");
        }
    }

    Ok(body)
}

fn message_body(message: &ChatMessage) -> Result<Value, ProviderError> {
    match message {
        ChatMessage::System { content } => message_with_text_content("system", content),
        ChatMessage::User { content } => Ok(json!({
            "role": "user",
            "content": user_content(content),
        })),
        ChatMessage::Assistant {
            content,
            tool_calls,
        } => assistant_message_body(content, tool_calls),
        ChatMessage::ToolResult {
            tool_call_id,
            content,
            is_error: _,
        } => tool_result_message_body(tool_call_id, content),
    }
}

fn message_with_text_content(role: &str, content: &[ContentPart]) -> Result<Value, ProviderError> {
    let content = content_text(content, role)?;
    Ok(json!({
        "role": role,
        "content": content,
    }))
}

fn assistant_message_body(
    content: &[ContentPart],
    tool_calls: &[crate::ToolCall],
) -> Result<Value, ProviderError> {
    let text = content_text(content, "assistant")?;
    let mut body = json!({
        "role": "assistant",
        "content": text,
    });
    if !tool_calls.is_empty() {
        body["tool_calls"] = json!(tool_calls.iter().map(tool_call_body).collect::<Vec<_>>());
    }
    let reasoning = reasoning_text(content);
    if !reasoning.is_empty() {
        body[DEFAULT_REASONING_KEY] = json!(reasoning);
    }
    Ok(body)
}

fn openai_reasoning_effort(effort: ReasoningEffort) -> Result<&'static str, ProviderError> {
    match effort {
        ReasoningEffort::Low => Ok("low"),
        ReasoningEffort::Medium => Ok("medium"),
        ReasoningEffort::High => Ok("high"),
        ReasoningEffort::Minimal | ReasoningEffort::XHigh => {
            Err(ProviderError::Unsupported(format!(
                "OpenAI-compatible provider type 'openai' supports reasoning_effort low, medium, or high; got {}",
                effort.as_str()
            )))
        }
    }
}

fn tool_call_body(tool_call: &crate::ToolCall) -> Value {
    json!({
        "id": tool_call.id,
        "type": "function",
        "function": {
            "name": tool_call.name,
            "arguments": tool_call.raw_arguments,
        },
    })
}

fn tool_result_message_body(
    tool_call_id: &str,
    content: &[ContentPart],
) -> Result<Value, ProviderError> {
    let content = content_text(content, "tool result")?;
    Ok(json!({
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": content,
    }))
}

fn content_text(content: &[ContentPart], role: &str) -> Result<String, ProviderError> {
    reject_images(content, "OpenAI-compatible", role)?;
    Ok(text_content(content))
}

fn text_content(content: &[ContentPart]) -> String {
    crate::providers::collect_text_content(content, false)
}

fn reasoning_text(content: &[ContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Thinking {
                text,
                redacted: false,
                ..
            } => Some(text.as_str()),
            ContentPart::Text { .. } | ContentPart::Thinking { .. } | ContentPart::Image { .. } => {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn request_replays_reasoning(request: &ChatRequest) -> bool {
    request.messages.iter().any(|message| match message {
        ChatMessage::System { content }
        | ChatMessage::User { content }
        | ChatMessage::Assistant { content, .. }
        | ChatMessage::ToolResult { content, .. } => !reasoning_text(content).is_empty(),
    })
}

fn user_content(content: &[ContentPart]) -> Value {
    if content
        .iter()
        .any(|part| matches!(part, ContentPart::Image { .. }))
    {
        Value::Array(content.iter().map(content_part_body).collect())
    } else {
        json!(text_content(content))
    }
}

fn content_part_body(part: &ContentPart) -> Value {
    match part {
        ContentPart::Text { text } | ContentPart::Thinking { text, .. } => json!({
            "type": "text",
            "text": text,
        }),
        ContentPart::Image { mime_type, data } => json!({
            "type": "image_url",
            "image_url": {
                "url": super::image_url(mime_type, data),
            },
        }),
    }
}

fn tool_body(tool: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": crate::tool_schema::normalize_tool_schema(&tool.input_schema),
        },
    })
}

pub fn normalize_openai_chat_sse(body: &str) -> Result<Vec<AiStreamEvent>, AiError> {
    parse_sse_events(body).map_err(ProviderError::into_ai_error)
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
    saw_done: bool,
    done: bool,
}

impl IncrementalSse {
    fn push_chunk(&mut self, bytes: &[u8]) -> Vec<Result<AiStreamEvent, AiError>> {
        if self.done {
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
                    self.done = true;
                    out.extend(self.finish());
                    break;
                }
                Ok(Some(payload)) => {
                    if let Err(err) = self.ingest_payload(&payload, &mut out) {
                        self.done = true;
                        out.push(Err(err));
                        break;
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    self.done = true;
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
        self.parser.ingest(&value);
        out.extend(self.parser.drain_events().into_iter().map(Ok));
        Ok(())
    }

    fn finish(&mut self) -> Vec<Result<AiStreamEvent, AiError>> {
        if self.parser.is_finished() {
            return Vec::new();
        }

        self.done = true;
        if !self.saw_done && !self.parser.saw_finish_reason() {
            return vec![Err(AiError::Stream {
                message: "missing SSE done marker".to_owned(),
            })];
        }

        match self.drain_trailing_payload_events() {
            Ok(events) if !events.is_empty() => return self.finish_after_trailing_events(events),
            Ok(_) => {}
            Err(err) => return vec![Err(err)],
        }

        self.parser.finish_events().map_or_else(
            |err| vec![Err(err.into_ai_error())],
            |events| events.into_iter().map(Ok).collect(),
        )
    }

    fn drain_trailing_payload_events(
        &mut self,
    ) -> Result<Vec<Result<AiStreamEvent, AiError>>, AiError> {
        let Some(payload) = parse_sse_frame(&self.buffer)? else {
            return Ok(Vec::new());
        };
        if payload == "[DONE]" {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        self.ingest_payload(&payload, &mut out)?;
        Ok(out)
    }

    fn finish_after_trailing_events(
        &mut self,
        mut events: Vec<Result<AiStreamEvent, AiError>>,
    ) -> Vec<Result<AiStreamEvent, AiError>> {
        match self.parser.finish_events() {
            Ok(finished) => events.extend(finished.into_iter().map(Ok)),
            Err(err) => events.push(Err(err.into_ai_error())),
        }
        events
    }
}

fn parse_sse_events(body: &str) -> Result<Vec<AiStreamEvent>, ProviderError> {
    let mut state = ParseState::default();
    let mut saw_done = false;
    for payload in sse_payloads(body) {
        if payload == "[DONE]" {
            saw_done = true;
            break;
        }
        let value = serde_json::from_str::<Value>(&payload)
            .map_err(|err| ProviderError::Stream(format!("invalid SSE JSON: {err}")))?;
        state.ingest(&value);
    }
    if !saw_done && !state.saw_finish_reason() {
        return Err(ProviderError::Stream("missing SSE done marker".to_owned()));
    }
    state.finish_events()
}

fn sse_payloads(body: &str) -> impl Iterator<Item = String> + '_ {
    body.split("\n\n").filter_map(|chunk| {
        let data = chunk
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("\n");
        (!data.is_empty()).then_some(data)
    })
}

struct ParseState {
    events: Vec<AiStreamEvent>,
    started: bool,
    reasoning_started: bool,
    tool_calls: StreamingToolCallAssembler,
    last_stop_reason: StopReason,
    usage: Option<TokenUsage>,
    saw_finish_reason: bool,
    provider_reported_tool_calls: bool,
    completed_tool_calls: usize,
    finished: bool,
}

impl Default for ParseState {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            started: false,
            reasoning_started: false,
            tool_calls: StreamingToolCallAssembler::new(),
            last_stop_reason: StopReason::EndTurn,
            usage: None,
            saw_finish_reason: false,
            provider_reported_tool_calls: false,
            completed_tool_calls: 0,
            finished: false,
        }
    }
}

impl ParseState {
    fn ingest(&mut self, value: &Value) {
        self.ensure_started(value);
        if let Some(usage) = value.get("usage") {
            self.usage = token_usage_from(usage, "prompt_tokens", "completion_tokens");
        }

        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return;
        };

        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.apply_finish_reason(reason);
            self.saw_finish_reason = true;
        }
        if let Some(delta) = choice.get("delta") {
            self.ingest_delta(delta);
        }
    }

    fn apply_finish_reason(&mut self, reason: &str) {
        match reason {
            "tool_calls" | "function_call" => {
                self.provider_reported_tool_calls = true;
            }
            "length" => self.last_stop_reason = StopReason::MaxTokens,
            "content_filter" => self.last_stop_reason = StopReason::Error,
            _ => self.last_stop_reason = StopReason::EndTurn,
        }
    }

    fn drain_events(&mut self) -> Vec<AiStreamEvent> {
        std::mem::take(&mut self.events)
    }

    const fn is_finished(&self) -> bool {
        self.finished
    }

    const fn saw_finish_reason(&self) -> bool {
        self.saw_finish_reason
    }

    fn ensure_started(&mut self, value: &Value) {
        if self.started {
            return;
        }
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("message")
            .to_owned();
        self.events.push(AiStreamEvent::MessageStart { id });
        self.started = true;
    }

    fn ingest_delta(&mut self, delta: &Value) {
        if let Some(reasoning) = reasoning_delta(delta)
            && !reasoning.is_empty()
        {
            if !self.reasoning_started {
                self.events.push(AiStreamEvent::ThinkingStart {
                    id: "reasoning".to_owned(),
                });
                self.reasoning_started = true;
            }
            self.events.push(AiStreamEvent::ThinkingDelta {
                text: reasoning.to_owned(),
            });
        }

        if let Some(text) = delta.get("content").and_then(Value::as_str)
            && !text.is_empty()
        {
            self.events.push(AiStreamEvent::TextDelta {
                text: text.to_owned(),
            });
        }

        let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) else {
            return;
        };

        for tool_call in tool_calls {
            self.ingest_tool_call(tool_call);
        }
    }

    fn ingest_tool_call(&mut self, tool_call: &Value) {
        let function = tool_call.get("function").unwrap_or(&Value::Null);
        let chunk = ToolCallChunk {
            index: tool_call.get("index").and_then(Value::as_u64),
            id: tool_call
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            name: function
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned),
            arguments_fragment: function
                .get("arguments")
                .and_then(Value::as_str)
                .map(str::to_owned),
        };
        match self.tool_calls.ingest(chunk) {
            Ok(events) => self.push_tool_events(events),
            Err(err) => {
                self.last_stop_reason = StopReason::Error;
                self.saw_finish_reason = true;
                self.events.push(AiStreamEvent::Error {
                    message: err.to_string(),
                });
            }
        }
    }

    fn push_tool_events(&mut self, events: Vec<ToolCallAssemblyEvent>) {
        self.events
            .extend(events.into_iter().map(|event| match event {
                ToolCallAssemblyEvent::Start { id, name } => {
                    AiStreamEvent::ToolCallStart { id, name }
                }
                ToolCallAssemblyEvent::ArgsDelta { id, json_fragment } => {
                    AiStreamEvent::ToolCallArgsDelta { id, json_fragment }
                }
                ToolCallAssemblyEvent::End { id, raw_arguments } => {
                    self.completed_tool_calls += 1;
                    AiStreamEvent::ToolCallEnd { id, raw_arguments }
                }
            }));
    }

    fn finish_events(&mut self) -> Result<Vec<AiStreamEvent>, ProviderError> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;

        let tool_events = self
            .tool_calls
            .finish_all()
            .map_err(|err| ProviderError::Stream(err.to_string()))?;
        self.push_tool_events(tool_events);

        if self.provider_reported_tool_calls {
            if self.completed_tool_calls == 0 {
                self.last_stop_reason = StopReason::Error;
                self.events.push(AiStreamEvent::Error {
                    message: EMPTY_STRUCTURED_TOOL_CALLS_MESSAGE.to_owned(),
                });
            } else {
                self.last_stop_reason = StopReason::ToolUse;
            }
        }
        if self.reasoning_started {
            self.events.push(AiStreamEvent::ThinkingEnd {
                signature: None,
                redacted: false,
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
}

fn reasoning_delta(delta: &Value) -> Option<&str> {
    KNOWN_REASONING_KEYS
        .iter()
        .find_map(|key| delta.get(*key).and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolCall;

    #[test]
    fn message_body_serializes_assistant_tool_calls() {
        let message = ChatMessage::Assistant {
            content: vec![ContentPart::Text {
                text: "calling tool".to_owned(),
            }],
            tool_calls: vec![ToolCall {
                id: "call_1".to_owned(),
                name: "lookup".to_owned(),
                raw_arguments: r#"{"query":"neo"}"#.to_owned(),
            }],
        };

        let body = message_body(&message).expect("assistant message body");

        assert_eq!(body["role"], "assistant");
        assert_eq!(body["content"], "calling tool");
        assert_eq!(body["tool_calls"][0]["id"], "call_1");
        assert_eq!(body["tool_calls"][0]["function"]["name"], "lookup");
        assert_eq!(
            body["tool_calls"][0]["function"]["arguments"],
            "{\"query\":\"neo\"}"
        );
    }

    #[test]
    fn normalize_openai_chat_sse_accepts_finish_reason_without_done_marker() {
        let events = normalize_openai_chat_sse(
            "data: {\"id\":\"chatcmpl_1\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
             data: {\"id\":\"chatcmpl_1\",\"choices\":[{\"finish_reason\":\"stop\",\"delta\":{}}]}\n\n",
        )
        .expect("normalize SSE");

        assert!(matches!(
            events.first(),
            Some(AiStreamEvent::MessageStart { .. })
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            AiStreamEvent::TextDelta { text } if text == "hi"
        )));
        assert!(matches!(
            events.last(),
            Some(AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                ..
            })
        ));
    }
}

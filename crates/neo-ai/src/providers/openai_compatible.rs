use std::collections::BTreeMap;

use futures::{StreamExt, future, stream};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};

use crate::{
    AiError, AiStreamEvent, CacheRetention, ChatMessage, ChatRequest, ContentPart, ImageData,
    ModelClient, StopReason, TokenUsage, ToolSpec,
};

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
        let url = format!("{}/chat/completions", self.base_url);
        let body = request_body(request)?;
        let mut builder = self
            .client
            .post(url)
            .headers(headers(
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
            return Err(ProviderError::HttpStatus(status.as_u16()));
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

#[derive(Debug)]
enum ProviderError {
    Header(String),
    HttpStatus(u16),
    Transport(reqwest::Error),
    Stream(String),
}

impl ProviderError {
    const fn is_retryable(&self) -> bool {
        match self {
            Self::HttpStatus(status) => *status == 429 || *status >= 500,
            Self::Transport(_) => true,
            Self::Header(_) | Self::Stream(_) => false,
        }
    }

    fn into_ai_error(self) -> AiError {
        match self {
            Self::Header(message) | Self::Stream(message) => AiError::Stream(message),
            Self::HttpStatus(status) => AiError::Stream(format!("http status {status}")),
            Self::Transport(err) => AiError::Stream(format!("transport error: {err}")),
        }
    }
}

fn headers(
    api_key: &str,
    extra_headers: &BTreeMap<String, String>,
    session_id: Option<&str>,
) -> Result<HeaderMap, ProviderError> {
    let mut headers = HeaderMap::new();
    let authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
        .map_err(|err| ProviderError::Header(format!("invalid authorization header: {err}")))?;
    headers.insert(AUTHORIZATION, authorization);

    for (name, value) in extra_headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|err| ProviderError::Header(format!("invalid header name {name}: {err}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|err| ProviderError::Header(format!("invalid header value {name}: {err}")))?;
        headers.insert(name, value);
    }
    if let Some(session_id) = session_id {
        let value = HeaderValue::from_str(session_id).map_err(|err| {
            ProviderError::Header(format!("invalid x-client-request-id header: {err}"))
        })?;
        headers.insert(HeaderName::from_static("x-client-request-id"), value);
    }

    Ok(headers)
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
        body["reasoning_effort"] = json!(reasoning_effort.as_str());
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
    let content = content_text(content, "assistant")?;
    Ok(json!({
        "role": "assistant",
        "content": content,
        "tool_calls": tool_calls.iter().map(tool_call_body).collect::<Vec<_>>(),
    }))
}

fn tool_call_body(tool_call: &crate::ToolCall) -> Value {
    json!({
        "id": tool_call.id,
        "type": "function",
        "function": {
            "name": tool_call.name,
            "arguments": tool_call.arguments.to_string(),
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
    reject_images(content, role)?;
    Ok(text_content(content))
}

fn text_content(content: &[ContentPart]) -> String {
    super::collect_text_content(content, true)
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
                "url": image_url(mime_type, data),
            },
        }),
    }
}

fn image_url(mime_type: &str, data: &ImageData) -> String {
    match data {
        ImageData::Base64(data) => format!("data:{mime_type};base64,{data}"),
        ImageData::Url(url) => url.clone(),
    }
}

fn reject_images(content: &[ContentPart], role: &str) -> Result<(), ProviderError> {
    if content
        .iter()
        .any(|part| matches!(part, ContentPart::Image { .. }))
    {
        return Err(ProviderError::Stream(format!(
            "OpenAI-compatible image content is only supported in user messages, not {role} messages"
        )));
    }
    Ok(())
}

fn tool_body(tool: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        },
    })
}

pub fn normalize_openai_chat_sse(body: &str) -> Result<Vec<AiStreamEvent>, AiError> {
    parse_sse_events(body).map_err(ProviderError::into_ai_error)
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

        self.done = true;
        if !self.saw_done && !self.parser.saw_finish_reason() {
            return vec![Err(AiError::Stream("missing SSE done marker".to_owned()))];
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
    tool_args: BTreeMap<String, String>,
    tool_index_ids: BTreeMap<u64, String>,
    last_stop_reason: StopReason,
    usage: Option<TokenUsage>,
    saw_finish_reason: bool,
    finished: bool,
}

impl Default for ParseState {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            started: false,
            tool_args: BTreeMap::new(),
            tool_index_ids: BTreeMap::new(),
            last_stop_reason: StopReason::EndTurn,
            usage: None,
            saw_finish_reason: false,
            finished: false,
        }
    }
}

impl ParseState {
    fn ingest(&mut self, value: &Value) {
        self.ensure_started(value);
        if let Some(usage) = value.get("usage") {
            self.usage = token_usage(usage);
        }

        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return;
        };

        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.last_stop_reason = stop_reason(reason);
            self.saw_finish_reason = true;
        }
        if let Some(delta) = choice.get("delta") {
            self.ingest_delta(delta);
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
        let index = tool_call
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or(self.tool_index_ids.len() as u64);
        let existing_id = self.tool_index_ids.get(&index).cloned();
        let id = tool_call
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or(existing_id)
            .unwrap_or_else(|| format!("tool-{index}"));
        self.tool_index_ids.insert(index, id.clone());

        let function = tool_call.get("function").unwrap_or(&Value::Null);
        if let Some(name) = function.get("name").and_then(Value::as_str) {
            self.events.push(AiStreamEvent::ToolCallStart {
                id: id.clone(),
                name: name.to_owned(),
            });
        }
        if let Some(fragment) = function.get("arguments").and_then(Value::as_str) {
            let arguments = self.tool_args.entry(id.clone()).or_default();
            if let Some(delta) = merge_tool_argument_fragment(arguments, fragment) {
                self.events.push(AiStreamEvent::ToolCallArgsDelta {
                    id,
                    json_fragment: delta,
                });
            }
        }
    }

    fn finish_events(&mut self) -> Result<Vec<AiStreamEvent>, ProviderError> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;

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
}

fn merge_tool_argument_fragment(arguments: &mut String, fragment: &str) -> Option<String> {
    if fragment.is_empty() {
        return None;
    }
    if arguments.is_empty() {
        arguments.push_str(fragment);
        return Some(fragment.to_owned());
    }
    if fragment.starts_with(arguments.as_str()) {
        let delta = fragment[arguments.len()..].to_owned();
        arguments.clear();
        arguments.push_str(fragment);
        return (!delta.is_empty()).then_some(delta);
    }
    if arguments.starts_with(fragment) {
        return None;
    }
    arguments.push_str(fragment);
    Some(fragment.to_owned())
}

fn token_usage(value: &Value) -> Option<TokenUsage> {
    Some(TokenUsage {
        input_tokens: u32::try_from(value.get("prompt_tokens")?.as_u64()?).ok()?,
        output_tokens: u32::try_from(value.get("completion_tokens")?.as_u64()?).ok()?,
    })
}

fn rounded_f64(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn stop_reason(reason: &str) -> StopReason {
    match reason {
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "length" => StopReason::MaxTokens,
        "content_filter" => StopReason::Error,
        _ => StopReason::EndTurn,
    }
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
                arguments: json!({"query": "neo"}),
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

    #[test]
    fn normalize_openai_chat_sse_rejects_unfinished_stream_without_done_or_finish_reason() {
        let error = normalize_openai_chat_sse(
            "data: {\"id\":\"chatcmpl_1\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
        )
        .expect_err("unfinished stream should fail");

        assert!(error.to_string().contains("missing SSE done marker"));
    }
}

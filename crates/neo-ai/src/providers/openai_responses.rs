use std::collections::{BTreeMap, VecDeque};

use futures::{StreamExt, future, stream};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};

use super::common::error::ProviderError;
use super::common::helpers::{reject_images, rounded_f64, token_usage_from};
use super::common::sse::{StreamChunk, find_frame_end, parse_sse_frame};

use crate::{
    AiError, AiStreamEvent, CacheRetention, ChatMessage, ChatRequest, ContentPart, ImageData,
    ModelClient, StopReason, TokenUsage, ToolSpec,
};

#[derive(Clone)]
pub struct OpenAiResponsesClient {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiResponsesClient {
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            client: reqwest::Client::new(),
        }
    }

    async fn open_response(&self, request: ChatRequest) -> Result<reqwest::Response, AiError> {
        super::common::http::open_response(&request, |req| Box::pin(self.open_response_once(req))).await
    }

    async fn open_response_once(
        &self,
        request: &ChatRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let url = format!("{}/responses", self.base_url);
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
            return Err(ProviderError::HttpStatus {
                status: status.as_u16(),
                body: None,
            });
        }

        Ok(response)
    }
}

impl ModelClient for OpenAiResponsesClient {
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
    session_id: Option<&str>,
) -> Result<HeaderMap, ProviderError> {
    let mut headers = HeaderMap::new();
    let authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
        .map_err(|err| ProviderError::Header(format!("invalid authorization header: {err}")))?;
    headers.insert(AUTHORIZATION, authorization);

    super::common::http::inject_extra_headers(&mut headers, extra_headers)?;
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
        "input": request_input(&request.messages, request.options.replay_reasoning)?,
    });

    if !request.tools.is_empty() {
        body["tools"] = Value::Array(request.tools.iter().map(tool_body).collect());
    }
    if let Some(temperature) = request.options.temperature {
        body["temperature"] = json!(rounded_f64(temperature));
    }
    if let Some(max_tokens) = request.options.max_tokens {
        body["max_output_tokens"] = json!(max_tokens);
    }
    if let Some(reasoning_effort) = request.options.reasoning_effort {
        body["reasoning"] = json!({
            "effort": reasoning_effort.as_str(),
            "summary": "auto",
        });
        body["include"] = json!(["reasoning.encrypted_content"]);
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

fn request_input(
    messages: &[ChatMessage],
    replay_reasoning: bool,
) -> Result<Vec<Value>, ProviderError> {
    let mut input = Vec::new();
    for message in messages {
        input.extend(message_body(message, replay_reasoning)?);
    }
    Ok(input)
}

fn message_body(
    message: &ChatMessage,
    replay_reasoning: bool,
) -> Result<Vec<Value>, ProviderError> {
    match message {
        ChatMessage::System { content } => {
            let content = content_text(content, "system")?;
            Ok(vec![json!({
                "role": "system",
                "content": content,
            })])
        }
        ChatMessage::User { content } => Ok(vec![json!({
                "role": "user",
                "content": user_content(content),
        })]),
        ChatMessage::Assistant {
            content,
            tool_calls,
        } => {
            let mut output = Vec::new();
            if replay_reasoning {
                output.extend(reasoning_items(content));
            }
            let text = content_text_with_reasoning_replay(content, "assistant", replay_reasoning)?;
            if !text.is_empty() {
                output.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": text, "annotations": [] }],
                    "status": "completed",
                }));
            }
            output.extend(tool_calls.iter().map(|tool_call| {
                json!({
                    "type": "function_call",
                    "call_id": tool_call.id,
                    "name": tool_call.name,
                    "arguments": tool_call.arguments,
                })
            }));
            Ok(output)
        }
        ChatMessage::ToolResult {
            tool_call_id,
            content,
            is_error: _,
        } => {
            let output = content_text(content, "tool result")?;
            Ok(vec![json!({
                "type": "function_call_output",
                "call_id": tool_call_id,
                "output": output,
            })])
        }
    }
}

fn reasoning_items(content: &[ContentPart]) -> Vec<Value> {
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Thinking { signature, .. } => {
                signature.as_deref().and_then(openai_reasoning_signature)
            }
            ContentPart::Text { .. } | ContentPart::Image { .. } => None,
        })
        .collect()
}

fn openai_reasoning_signature(signature: &str) -> Option<Value> {
    let item = serde_json::from_str::<Value>(signature).ok()?;
    (item.get("type").and_then(Value::as_str) == Some("reasoning")).then_some(item)
}

fn content_part_body(part: &ContentPart) -> Value {
    match part {
        ContentPart::Text { text } => json!({
            "type": "input_text",
            "text": text,
        }),
        ContentPart::Thinking { .. } => json!({
            "type": "input_text",
            "text": "",
        }),
        ContentPart::Image { mime_type, data } => {
            let image_url = image_url(mime_type, data);
            json!({
                "type": "input_image",
                "image_url": image_url,
            })
        }
    }
}

fn image_url(mime_type: &str, data: &ImageData) -> String {
    match data {
        ImageData::Base64(data) => format!("data:{mime_type};base64,{data}"),
        ImageData::Url(url) => url.clone(),
    }
}

fn content_text(content: &[ContentPart], role: &str) -> Result<String, ProviderError> {
    content_text_with_reasoning_replay(content, role, true)
}

fn content_text_with_reasoning_replay(
    content: &[ContentPart],
    role: &str,
    replay_reasoning: bool,
) -> Result<String, ProviderError> {
    reject_images(content, "OpenAI Responses", role)?;
    Ok(text_content_with_reasoning_replay(
        content,
        replay_reasoning,
    ))
}

// NOTE: Uses provider-specific reasoning-replay validation; cannot use the shared
// collect_text_content helper.
fn text_content(content: &[ContentPart]) -> String {
    text_content_with_reasoning_replay(content, true)
}

fn text_content_with_reasoning_replay(content: &[ContentPart], replay_reasoning: bool) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            ContentPart::Thinking {
                text,
                signature,
                redacted: false,
            } if replay_reasoning
                && signature
                    .as_deref()
                    .and_then(openai_reasoning_signature)
                    .is_none() =>
            {
                Some(text.as_str())
            }
            ContentPart::Thinking { .. } | ContentPart::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
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

fn tool_body(tool: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
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


struct ParseState {
    events: Vec<AiStreamEvent>,
    started: bool,
    tool_args: BTreeMap<String, String>,
    item_call_ids: BTreeMap<String, String>,
    thinking_parts: BTreeMap<String, ThinkingPart>,
    thinking_order: VecDeque<String>,
    active_thinking_id: Option<String>,
    last_stop_reason: StopReason,
    usage: Option<TokenUsage>,
    terminal: bool,
    finished: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ThinkingPart {
    text: String,
    // Byte offset into `text`; it is only assigned from `String::len()` after
    // whole-string appends, so slicing at this boundary is valid UTF-8.
    emitted_len: usize,
    done: bool,
    signature: Option<String>,
}

impl Default for ParseState {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            started: false,
            tool_args: BTreeMap::new(),
            item_call_ids: BTreeMap::new(),
            thinking_parts: BTreeMap::new(),
            thinking_order: VecDeque::new(),
            active_thinking_id: None,
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
            Some("response.created") => {
                let id = value
                    .get("response")
                    .and_then(|response| response.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("response")
                    .to_owned();
                self.ensure_started(id);
            }
            Some("response.output_text.delta") => {
                self.ensure_started("response".to_owned());
                if let Some(text) = value.get("delta").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    self.events.push(AiStreamEvent::TextDelta {
                        text: text.to_owned(),
                    });
                }
            }
            Some("response.reasoning_summary_part.added") => self.ingest_thinking_started(value),
            Some("response.reasoning_summary_text.delta") => self.ingest_thinking_delta(value),
            Some("response.reasoning_summary_text.done") => self.ingest_thinking_text_done(value),
            Some("response.reasoning_summary_part.done") => self.ingest_thinking_done(value),
            Some("response.output_item.done") => self.ingest_output_item_done(value),
            Some("response.output_item.added") => self.ingest_item_added(value),
            Some("response.function_call_arguments.delta") => self.ingest_tool_delta(value),
            Some("response.completed") => {
                self.ingest_completed(value);
                self.terminal = true;
            }
            Some("response.failed" | "response.incomplete") => {
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

    fn ingest_item_added(&mut self, value: &Value) {
        let item = value.get("item").unwrap_or(&Value::Null);
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return;
        }

        self.ensure_started("response".to_owned());
        let item_id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("function-call")
            .to_owned();
        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or(&item_id)
            .to_owned();
        self.item_call_ids.insert(item_id, call_id.clone());
        if let Some(name) = item.get("name").and_then(Value::as_str) {
            self.events.push(AiStreamEvent::ToolCallStart {
                id: call_id,
                name: name.to_owned(),
            });
        }
    }

    fn ingest_thinking_started(&mut self, value: &Value) {
        self.ensure_started("response".to_owned());
        let id = thinking_id(value);
        self.ensure_thinking_part(id);
        self.flush_thinking_ready();
    }

    fn ingest_thinking_delta(&mut self, value: &Value) {
        self.ensure_started("response".to_owned());
        let id = thinking_id(value);
        self.ensure_thinking_part(id.clone());
        if let Some(delta) = value.get("delta").and_then(Value::as_str)
            && !delta.is_empty()
        {
            self.thinking_parts
                .get_mut(&id)
                .expect("thinking part should exist")
                .text
                .push_str(delta);
        }
        self.flush_thinking_ready();
    }

    fn ingest_thinking_text_done(&mut self, value: &Value) {
        self.ensure_started("response".to_owned());
        let id = thinking_id(value);
        self.ensure_thinking_part(id.clone());
        let Some(text) = value.get("text").and_then(Value::as_str) else {
            self.flush_thinking_ready();
            return;
        };
        let part = self
            .thinking_parts
            .get_mut(&id)
            .expect("thinking part should exist");
        merge_final_thinking_text(part, text);
        self.flush_thinking_ready();
    }

    fn ingest_thinking_done(&mut self, value: &Value) {
        let id = thinking_id(value);
        self.ensure_thinking_part(id.clone());
        if let Some(text) = value
            .get("part")
            .and_then(|part| part.get("text"))
            .and_then(Value::as_str)
        {
            let part = self
                .thinking_parts
                .get_mut(&id)
                .expect("thinking part should exist");
            merge_final_thinking_text(part, text);
        }
        self.thinking_parts
            .get_mut(&id)
            .expect("thinking part should exist")
            .done = true;
        if let Some(item) = value.get("item") {
            self.thinking_parts
                .get_mut(&id)
                .expect("thinking part should exist")
                .signature = Some(item.to_string());
        }
        self.flush_thinking_ready();
    }

    fn ingest_output_item_done(&mut self, value: &Value) {
        let item = value.get("item").unwrap_or(&Value::Null);
        if item.get("type").and_then(Value::as_str) != Some("reasoning") {
            return;
        }
        let item_id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("reasoning")
            .to_owned();
        let id = self
            .thinking_order
            .iter()
            .find(|candidate| {
                candidate.as_str() == item_id
                    || candidate.starts_with(&format!("{item_id}:summary:"))
                    || candidate.contains(&format!(":{item_id}:summary:"))
            })
            .cloned()
            .unwrap_or(item_id);
        self.ensure_thinking_part(id.clone());
        if let Some(text) = reasoning_item_text(item) {
            let part = self
                .thinking_parts
                .get_mut(&id)
                .expect("thinking part should exist");
            merge_final_thinking_text(part, &text);
        }
        let part = self
            .thinking_parts
            .get_mut(&id)
            .expect("thinking part should exist");
        part.signature = Some(item.to_string());
        part.done = true;
        self.flush_thinking_ready();
    }

    fn ingest_tool_delta(&mut self, value: &Value) {
        let item_id = value
            .get("item_id")
            .and_then(Value::as_str)
            .unwrap_or("function-call");
        let id = self
            .item_call_ids
            .get(item_id)
            .cloned()
            .unwrap_or_else(|| item_id.to_owned());
        if let Some(fragment) = value.get("delta").and_then(Value::as_str) {
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

    fn ingest_completed(&mut self, value: &Value) {
        let response = value.get("response").unwrap_or(&Value::Null);
        self.usage = response
            .get("usage")
            .and_then(|v| token_usage_from(v, "input_tokens", "output_tokens"));
        self.last_stop_reason = if self.tool_args.is_empty() {
            StopReason::EndTurn
        } else {
            StopReason::ToolUse
        };
    }

    fn finish_events(&mut self) -> Result<Vec<AiStreamEvent>, ProviderError> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;

        for part in self.thinking_parts.values_mut() {
            part.done = true;
        }
        self.flush_thinking_ready();

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

    fn ensure_thinking_part(&mut self, id: String) {
        if self.thinking_parts.contains_key(&id) {
            return;
        }
        self.thinking_order.push_back(id.clone());
        self.thinking_parts.insert(id, ThinkingPart::default());
    }

    fn flush_thinking_ready(&mut self) {
        while let Some(id) = self.thinking_order.front().cloned() {
            if self.active_thinking_id.as_deref() != Some(id.as_str()) {
                if self.active_thinking_id.is_some() {
                    return;
                }
                self.events
                    .push(AiStreamEvent::ThinkingStart { id: id.clone() });
                self.active_thinking_id = Some(id.clone());
            }

            let mut is_done = false;
            if let Some(part) = self.thinking_parts.get_mut(&id) {
                if part.emitted_len < part.text.len() {
                    let delta = part.text[part.emitted_len..].to_owned();
                    part.emitted_len = part.text.len();
                    if !delta.is_empty() {
                        self.events
                            .push(AiStreamEvent::ThinkingDelta { text: delta });
                    }
                }
                is_done = part.done;
            }

            if !is_done {
                return;
            }

            self.events.push(AiStreamEvent::ThinkingEnd {
                signature: self
                    .thinking_parts
                    .get(&id)
                    .and_then(|part| part.signature.clone()),
                redacted: false,
            });
            self.active_thinking_id = None;
            self.thinking_parts.remove(&id);
            self.thinking_order.pop_front();
        }
    }
}

fn thinking_id(value: &Value) -> String {
    let item_id = value
        .get("item_id")
        .and_then(Value::as_str)
        .or_else(|| value.get("id").and_then(Value::as_str))
        .unwrap_or("reasoning-summary");
    let Some(summary_index) = value.get("summary_index").and_then(Value::as_u64) else {
        return item_id.to_owned();
    };
    if let Some(output_index) = value.get("output_index").and_then(Value::as_u64) {
        format!("{item_id}:output:{output_index}:summary:{summary_index}")
    } else {
        format!("{item_id}:summary:{summary_index}")
    }
}

fn merge_final_thinking_text(part: &mut ThinkingPart, text: &str) {
    if let Some(delta) = text.strip_prefix(&part.text) {
        part.text.push_str(delta);
    } else if part.emitted_len == 0 {
        text.clone_into(&mut part.text);
    }
}

fn reasoning_item_text(item: &Value) -> Option<String> {
    let values = item
        .get("summary")
        .and_then(Value::as_array)
        .or_else(|| item.get("content").and_then(Value::as_array))?
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join("\n\n"))
}

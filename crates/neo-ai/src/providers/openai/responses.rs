use std::collections::{BTreeMap, VecDeque};

use futures::{StreamExt, future, stream};
use serde_json::{Value, json};

use crate::providers::common::error::{ProviderError, stream_failure};
use crate::providers::common::helpers::{reject_images, rounded_f64, token_usage_from};
use crate::providers::common::sse::{SseFramer, StreamChunk};
use crate::tool_assembly::{StreamingToolCallAssembler, ToolCallAssemblyEvent, ToolCallChunk};

use crate::{
    AiError, AiStreamEvent, CacheRetention, ChatMessage, ChatRequest, ContentPart, MessagePhase,
    ModelClient, ReasoningEffort, ReasoningSelection, StopReason, TokenUsage, ToolSpec,
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
        self.open_response_once(&request)
            .await
            .map_err(ProviderError::into_ai_error)
    }

    async fn open_response_once(
        &self,
        request: &ChatRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let url = crate::providers::common::http::request_url(&self.base_url, "/responses")?;
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
        if !response.status().is_success() {
            return Err(crate::providers::common::http::http_status_error(response).await);
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
    if let Some(effort) = openai_responses_reasoning(&request.options.reasoning)? {
        body["reasoning"] = json!({
            "effort": effort.as_str(),
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
    if let Some(response_format) = &request.options.response_format {
        body["text"] = json!({
            "format": response_format.to_openai_responses_text_format(),
        });
    }

    Ok(body)
}

fn openai_responses_reasoning(
    selection: &ReasoningSelection,
) -> Result<Option<ReasoningEffort>, ProviderError> {
    match selection {
        ReasoningSelection::Off => Ok(None),
        ReasoningSelection::On => Ok(Some(ReasoningEffort::high())),
        ReasoningSelection::Effort { effort } => Ok(Some(effort.clone())),
        ReasoningSelection::BudgetTokens { .. } => Err(ProviderError::Unsupported(
            "OpenAI Responses provider does not support budget reasoning selections".to_owned(),
        )),
    }
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
                    "arguments": tool_call.raw_arguments,
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
            let image_url = super::image_url(mime_type, data);
            json!({
                "type": "input_image",
                "image_url": image_url,
            })
        }
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
                StreamChunk::Data(Err(_)) | StreamChunk::End if state.stopped => Vec::new(),
                StreamChunk::Data(Err(err)) => {
                    if state.saw_done || state.parser.saw_terminal() {
                        state.finish()
                    } else {
                        state.stopped = true;
                        vec![Err(AiError::Transport {
                            message: err.to_string(),
                        })]
                    }
                }
                StreamChunk::End => state.finish(),
            }))
        })
        .flat_map(stream::iter)
        .boxed()
}

#[derive(Default)]
struct IncrementalSse {
    framer: SseFramer,
    parser: ParseState,
    saw_done: bool,
    stopped: bool,
}

impl IncrementalSse {
    fn push_chunk(&mut self, bytes: &[u8]) -> Vec<Result<AiStreamEvent, AiError>> {
        if self.stopped {
            return Vec::new();
        }

        let mut out = Vec::new();
        match self.framer.push(bytes) {
            Ok(frames) => {
                for frame in frames {
                    match frame.parse() {
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
            }
            Err(err) => {
                self.stopped = true;
                out.push(Err(err));
            }
        }

        out
    }

    fn ingest_payload(
        &mut self,
        payload: &str,
        out: &mut Vec<Result<AiStreamEvent, AiError>>,
    ) -> Result<(), AiError> {
        let value = serde_json::from_str::<Value>(payload).map_err(|err| AiError::Protocol {
            message: format!("invalid SSE JSON: {err}"),
        })?;
        self.parser
            .ingest(&value)
            .map_err(ProviderError::into_ai_error)?;
        out.extend(self.parser.drain_events().into_iter().map(Ok));
        Ok(())
    }

    fn finish(&mut self) -> Vec<Result<AiStreamEvent, AiError>> {
        if self.parser.is_finished() {
            return Vec::new();
        }

        self.stopped = true;
        if !self.saw_done && !self.parser.saw_terminal() {
            return vec![Err(AiError::Transport {
                message: "missing SSE done marker".to_owned(),
            })];
        }

        self.parser.finish_events().map_or_else(
            |err| vec![Err(err.into_ai_error())],
            |events| events.into_iter().map(Ok).collect(),
        )
    }
}

#[allow(clippy::struct_excessive_bools)]
struct ParseState {
    events: Vec<AiStreamEvent>,
    pending_events: Vec<AiStreamEvent>,
    started: bool,
    message_id: Option<String>,
    message_phase: MessagePhase,
    tool_calls: StreamingToolCallAssembler,
    item_call_ids: BTreeMap<String, String>,
    item_names: BTreeMap<String, String>,
    item_indexes: BTreeMap<String, u64>,
    next_tool_index: u64,
    thinking_parts: BTreeMap<String, ThinkingPart>,
    thinking_order: VecDeque<String>,
    active_thinking_id: Option<String>,
    last_stop_reason: StopReason,
    usage: Option<TokenUsage>,
    saw_tool_call: bool,
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
            pending_events: Vec::new(),
            started: false,
            message_id: None,
            message_phase: MessagePhase::Unknown,
            tool_calls: StreamingToolCallAssembler::new(),
            item_call_ids: BTreeMap::new(),
            item_names: BTreeMap::new(),
            item_indexes: BTreeMap::new(),
            next_tool_index: 0,
            thinking_parts: BTreeMap::new(),
            thinking_order: VecDeque::new(),
            active_thinking_id: None,
            last_stop_reason: StopReason::EndTurn,
            usage: None,
            saw_tool_call: false,
            terminal: false,
            finished: false,
        }
    }
}

impl ParseState {
    fn ingest(&mut self, value: &Value) -> Result<(), ProviderError> {
        match value.get("type").and_then(Value::as_str) {
            Some("response.created") => {
                let id = value
                    .get("response")
                    .and_then(|response| response.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("response")
                    .to_owned();
                self.message_id = Some(id);
            }
            Some("response.output_text.delta") => {
                self.defer_start("response".to_owned());
                if let Some(text) = value.get("delta").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    self.queue_event(AiStreamEvent::TextDelta {
                        text: text.to_owned(),
                    });
                }
            }
            Some("response.reasoning_summary_part.added") => self.ingest_thinking_started(value),
            Some("response.reasoning_summary_text.delta") => self.ingest_thinking_delta(value),
            Some("response.reasoning_summary_text.done") => self.ingest_thinking_text_done(value),
            Some("response.reasoning_summary_part.done") => self.ingest_thinking_done(value),
            Some("response.output_item.done") => self.ingest_output_item_done(value)?,
            Some("response.output_item.added") => self.ingest_item_added(value)?,
            Some("response.function_call_arguments.delta") => self.ingest_tool_delta(value)?,
            Some("response.completed") => {
                self.ingest_completed(value);
                self.terminal = true;
            }
            Some("error") => return Err(Self::ingest_error_event(value)),
            Some("response.failed" | "response.incomplete") => {
                return Err(Self::ingest_response_failed(value));
            }
            _ => {}
        }
        Ok(())
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

    fn defer_start(&mut self, id: String) {
        if self.message_id.is_none() {
            self.message_id = Some(id);
        }
    }

    fn ensure_started(&mut self, id: String) {
        self.ensure_started_with_phase(id, MessagePhase::Unknown);
    }

    fn ensure_started_with_phase(&mut self, id: String, phase: MessagePhase) {
        self.record_phase(phase);
        if self.started {
            return;
        }
        self.defer_start(id);
        let id = self
            .message_id
            .clone()
            .expect("message id should be recorded before start");
        self.events.push(AiStreamEvent::MessageStart {
            id,
            phase: self.message_phase,
        });
        self.started = true;
        self.events.append(&mut self.pending_events);
    }

    fn queue_event(&mut self, event: AiStreamEvent) {
        if self.started {
            self.events.push(event);
        } else {
            self.pending_events.push(event);
        }
    }

    fn record_phase(&mut self, phase: MessagePhase) {
        if phase != MessagePhase::Unknown {
            self.message_phase = phase;
        }
    }

    fn ingest_message_item(&mut self, item: &Value) {
        if let Some(id) = item.get("id").and_then(Value::as_str)
            && self.message_id.is_none()
        {
            self.message_id = Some(id.to_owned());
        }
        let phase = message_phase_from_item(item);
        if phase == MessagePhase::Unknown {
            self.defer_start("response".to_owned());
        } else {
            self.ensure_started_with_phase("response".to_owned(), phase);
        }
    }

    fn tool_index_for_item(&mut self, item_id: &str) -> u64 {
        if let Some(index) = self.item_indexes.get(item_id) {
            return *index;
        }
        let index = self.next_tool_index;
        self.next_tool_index += 1;
        self.item_indexes.insert(item_id.to_owned(), index);
        index
    }

    fn push_tool_events(&mut self, events: Vec<ToolCallAssemblyEvent>) {
        for event in events {
            let event = match event {
                ToolCallAssemblyEvent::Start { id, name } => {
                    AiStreamEvent::ToolCallStart { id, name }
                }
                ToolCallAssemblyEvent::ArgsDelta { id, json_fragment } => {
                    AiStreamEvent::ToolCallArgsDelta { id, json_fragment }
                }
                ToolCallAssemblyEvent::End { id, raw_arguments } => {
                    self.saw_tool_call = true;
                    AiStreamEvent::ToolCallEnd { id, raw_arguments }
                }
            };
            self.queue_event(event);
        }
    }

    fn ingest_item_added(&mut self, value: &Value) -> Result<(), ProviderError> {
        let item = value.get("item").unwrap_or(&Value::Null);
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                self.ingest_message_item(item);
                return Ok(());
            }
            Some("function_call") => {}
            _ => return Ok(()),
        }

        self.defer_start("response".to_owned());
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
        self.item_call_ids.insert(item_id.clone(), call_id.clone());
        let index = self.tool_index_for_item(&item_id);
        if let Some(name) = item.get("name").and_then(Value::as_str) {
            self.item_names.insert(item_id, name.to_owned());
            let events = self.tool_calls.ingest(ToolCallChunk {
                index: Some(index),
                id: Some(call_id),
                name: Some(name.to_owned()),
                arguments_delta: None,
            });
            match events {
                Ok(events) => self.push_tool_events(events),
                Err(err) => return Err(ProviderError::Protocol(err.to_string())),
            }
        }
        Ok(())
    }

    fn ingest_thinking_started(&mut self, value: &Value) {
        self.defer_start("response".to_owned());
        let id = thinking_id(value);
        self.ensure_thinking_part(id);
        self.flush_thinking_ready();
    }

    fn ingest_thinking_delta(&mut self, value: &Value) {
        self.defer_start("response".to_owned());
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
        self.defer_start("response".to_owned());
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

    fn ingest_output_item_done(&mut self, value: &Value) -> Result<(), ProviderError> {
        let item = value.get("item").unwrap_or(&Value::Null);

        if item.get("type").and_then(Value::as_str) == Some("message") {
            self.ingest_message_item(item);
            return Ok(());
        }

        // Handle function_call items as authoritative final tool-call data.
        if item.get("type").and_then(Value::as_str) == Some("function_call") {
            self.defer_start("response".to_owned());
            let item_id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("function-call");
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .or_else(|| self.item_call_ids.get(item_id).map(String::as_str))
                .unwrap_or(item_id)
                .to_owned();
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| self.item_names.get(item_id).map(String::as_str))
                .unwrap_or("function_call")
                .to_owned();
            let index = self.tool_index_for_item(item_id);
            let raw_arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let events = self.tool_calls.finish_with_final_arguments(
                Some(index),
                call_id,
                name,
                raw_arguments,
            );
            match events {
                Ok(events) => self.push_tool_events(events),
                Err(err) => return Err(ProviderError::Protocol(err.to_string())),
            }
            return Ok(());
        }

        if item.get("type").and_then(Value::as_str) != Some("reasoning") {
            return Ok(());
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
        Ok(())
    }

    fn ingest_tool_delta(&mut self, value: &Value) -> Result<(), ProviderError> {
        self.defer_start("response".to_owned());
        let item_id = value
            .get("item_id")
            .and_then(Value::as_str)
            .unwrap_or("function-call");
        let id = self
            .item_call_ids
            .get(item_id)
            .cloned()
            .unwrap_or_else(|| item_id.to_owned());
        let index = self.tool_index_for_item(item_id);
        let fragment = value
            .get("delta")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let events = self.tool_calls.ingest(ToolCallChunk {
            index: Some(index),
            id: Some(id),
            name: self.item_names.get(item_id).cloned(),
            arguments_delta: fragment,
        });
        match events {
            Ok(events) => self.push_tool_events(events),
            Err(err) => return Err(ProviderError::Protocol(err.to_string())),
        }
        Ok(())
    }

    fn ingest_completed(&mut self, value: &Value) {
        self.ensure_started("response".to_owned());
        let response = value.get("response").unwrap_or(&Value::Null);
        self.usage = response
            .get("usage")
            .and_then(|v| token_usage_from(v, "input_tokens", "output_tokens"));
        self.last_stop_reason = if self.item_call_ids.is_empty() {
            StopReason::EndTurn
        } else {
            StopReason::ToolUse
        };
    }

    /// Extract error information from a top-level `"error"` event.
    fn ingest_error_event(value: &Value) -> ProviderError {
        let nested = value.get("error");
        let numeric_code = value
            .get("code")
            .and_then(Value::as_u64)
            .or_else(|| value.get("status").and_then(Value::as_u64))
            .or_else(|| {
                nested
                    .and_then(|error| error.get("code"))
                    .and_then(Value::as_u64)
            })
            .or_else(|| {
                nested
                    .and_then(|error| error.get("status"))
                    .and_then(Value::as_u64)
            })
            .map(|code| code.to_string());
        let code = value
            .get("code")
            .and_then(Value::as_str)
            .or_else(|| {
                nested
                    .and_then(|error| error.get("code"))
                    .and_then(Value::as_str)
            })
            .or_else(|| value.get("status").and_then(Value::as_str))
            .or_else(|| {
                nested
                    .and_then(|error| error.get("status"))
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                nested
                    .and_then(|error| error.get("type"))
                    .and_then(Value::as_str)
            })
            .or(numeric_code.as_deref());
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| {
                nested
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("provider returned an error")
            .to_owned();
        stream_failure(code, message)
    }

    /// Extract and classify error information from a
    /// `"response.failed"` or `"response.incomplete"` event.
    fn ingest_response_failed(value: &Value) -> ProviderError {
        let response = value.get("response").unwrap_or(&Value::Null);
        let status = response
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("failed");
        let error = response.get("error");
        let numeric_code = error
            .and_then(|error| error.get("code"))
            .and_then(Value::as_u64)
            .or_else(|| {
                error
                    .and_then(|error| error.get("status"))
                    .and_then(Value::as_u64)
            })
            .map(|code| code.to_string());
        let code = error
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str);
        let code = code
            .or_else(|| {
                error
                    .and_then(|error| error.get("type"))
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                error
                    .and_then(|error| error.get("status"))
                    .and_then(Value::as_str)
            })
            .or(numeric_code.as_deref());
        let message = error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .map_or_else(
                || format!("provider response ended with status {status}"),
                str::to_owned,
            );
        stream_failure(code, message)
    }

    fn finish_events(&mut self) -> Result<Vec<AiStreamEvent>, ProviderError> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;
        self.ensure_started("response".to_owned());

        for part in self.thinking_parts.values_mut() {
            part.done = true;
        }
        self.flush_thinking_ready();

        let outcome = self.tool_calls.finish_all();
        self.push_tool_events(outcome.events);
        if let Some(err) = outcome.error {
            return Err(ProviderError::Protocol(err.to_string()));
        }

        if self.saw_tool_call {
            self.last_stop_reason = StopReason::ToolUse;
        }

        if self.started {
            self.events.push(AiStreamEvent::MessageEnd {
                stop_reason: self.last_stop_reason.clone(),
                usage: self.usage.clone(),
                phase: self.message_phase,
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
                self.queue_event(AiStreamEvent::ThinkingStart {
                    id: id.clone(),
                    kind: crate::ThinkingKind::Summary,
                });
                self.active_thinking_id = Some(id.clone());
            }

            let (delta, is_done) = if let Some(part) = self.thinking_parts.get_mut(&id) {
                let delta = if part.emitted_len < part.text.len() {
                    let delta = part.text[part.emitted_len..].to_owned();
                    part.emitted_len = part.text.len();
                    Some(delta)
                } else {
                    None
                };
                (delta, part.done)
            } else {
                (None, false)
            };
            if let Some(delta) = delta
                && !delta.is_empty()
            {
                self.queue_event(AiStreamEvent::ThinkingDelta { text: delta });
            }

            if !is_done {
                return;
            }

            self.queue_event(AiStreamEvent::ThinkingEnd {
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

fn message_phase_from_item(item: &Value) -> MessagePhase {
    match item.get("phase").and_then(Value::as_str) {
        Some("commentary") => MessagePhase::Commentary,
        Some("final_answer") => MessagePhase::FinalAnswer,
        _ => MessagePhase::Unknown,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_sse_frame_is_rejected() {
        let mut parser = IncrementalSse::default();
        let data = "x".repeat(crate::providers::common::sse::SseFramer::MAX_FRAME_BYTES + 1);
        let body = format!("data: {data}\n\n");
        let error = parser
            .push_chunk(body.as_bytes())
            .into_iter()
            .find_map(Result::err)
            .expect("should emit an error");
        assert!(matches!(error, AiError::Protocol { .. }));
        assert!(!error.is_retryable());
    }

    #[test]
    fn response_format_maps_json_schema_for_responses_api() {
        use crate::{ApiKind, ModelCapabilities, ModelSpec, ProviderId, ResponseFormat};

        let format = ResponseFormat {
            name: "result".to_owned(),
            schema: json!({"type": "object", "properties": {"n": {"type": "integer"}}, "required": ["n"], "additionalProperties": false}),
            strict: true,
        };
        let request = ChatRequest {
            model: ModelSpec {
                provider: ProviderId("openai".to_owned()),
                model: "gpt-test".to_owned(),
                api: ApiKind::OpenAiResponse,
                capabilities: ModelCapabilities::chat(),
            },
            messages: vec![ChatMessage::User {
                content: vec![ContentPart::Text {
                    text: "hi".to_owned(),
                }],
            }],
            tools: vec![],
            options: crate::RequestOptions {
                response_format: Some(format.clone()),
                ..crate::RequestOptions::default()
            },
        };
        let body = request_body(&request).expect("body");
        assert_eq!(
            body["text"]["format"],
            format.to_openai_responses_text_format()
        );
    }

    fn message_item(phase: Option<&str>) -> Value {
        let mut item = json!({"id": "message-1", "type": "message"});
        if let Some(phase) = phase {
            item["phase"] = Value::String(phase.to_owned());
        }
        item
    }

    fn parse_message_item_events(
        added_phase: Option<&str>,
        done_phase: Option<&str>,
    ) -> Vec<AiStreamEvent> {
        let mut parser = ParseState::default();
        parser
            .ingest(&json!({
                "type": "response.created",
                "response": {"id": "response-1"}
            }))
            .expect("response.created should parse");
        parser
            .ingest(&json!({
                "type": "response.output_item.added",
                "item": message_item(added_phase)
            }))
            .expect("output item added should parse");
        parser
            .ingest(&json!({
                "type": "response.output_text.delta",
                "delta": "text"
            }))
            .expect("output text delta should parse");
        parser
            .ingest(&json!({
                "type": "response.output_item.done",
                "item": message_item(done_phase)
            }))
            .expect("output item done should parse");
        parser
            .ingest(&json!({
                "type": "response.completed",
                "response": {"status": "completed"}
            }))
            .expect("response.completed should parse");
        parser.finish_events().expect("stream should finish")
    }

    #[test]
    fn response_message_item_phase_maps_explicit_values_without_duplicate_start() {
        for (wire_phase, phase) in [
            ("commentary", MessagePhase::Commentary),
            ("final_answer", MessagePhase::FinalAnswer),
        ] {
            let events = parse_message_item_events(Some(wire_phase), Some(wire_phase));
            assert_eq!(
                events,
                vec![
                    AiStreamEvent::MessageStart {
                        id: "response-1".to_owned(),
                        phase,
                    },
                    AiStreamEvent::TextDelta {
                        text: "text".to_owned(),
                    },
                    AiStreamEvent::MessageEnd {
                        stop_reason: StopReason::EndTurn,
                        usage: None,
                        phase,
                    },
                ]
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, AiStreamEvent::MessageStart { .. }))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn response_message_item_done_phase_promotes_buffered_text_start() {
        let events = parse_message_item_events(None, Some("final_answer"));
        assert_eq!(
            events,
            vec![
                AiStreamEvent::MessageStart {
                    id: "response-1".to_owned(),
                    phase: MessagePhase::FinalAnswer,
                },
                AiStreamEvent::TextDelta {
                    text: "text".to_owned(),
                },
                AiStreamEvent::MessageEnd {
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                    phase: MessagePhase::FinalAnswer,
                },
            ]
        );
    }

    fn ingest_buffered_reasoning_tool_text(parser: &mut ParseState) {
        for value in [
            json!({
                "type": "response.created",
                "response": {"id": "response-1"}
            }),
            json!({
                "type": "response.reasoning_summary_part.added",
                "item_id": "reasoning-1",
                "summary_index": 0
            }),
            json!({
                "type": "response.reasoning_summary_text.delta",
                "item_id": "reasoning-1",
                "summary_index": 0,
                "delta": "think"
            }),
            json!({
                "type": "response.reasoning_summary_part.done",
                "item_id": "reasoning-1",
                "summary_index": 0,
                "part": {"text": "think"},
                "item": {"id": "reasoning-1", "type": "reasoning"}
            }),
            json!({
                "type": "response.output_item.added",
                "item": {
                    "id": "function-1",
                    "type": "function_call",
                    "call_id": "call-1",
                    "name": "lookup"
                }
            }),
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": "function-1",
                "delta": "{\"query\":\"neo\"}"
            }),
            json!({
                "type": "response.output_item.done",
                "item": {
                    "id": "function-1",
                    "type": "function_call",
                    "call_id": "call-1",
                    "name": "lookup",
                    "arguments": "{\"query\":\"neo\"}"
                }
            }),
            json!({
                "type": "response.output_item.added",
                "item": message_item(None)
            }),
            json!({
                "type": "response.output_text.delta",
                "delta": "answer"
            }),
        ] {
            parser
                .ingest(&value)
                .expect("buffered response event should parse");
        }
    }

    fn event_kinds(events: &[AiStreamEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(|event| match event {
                AiStreamEvent::MessageStart { .. } => "message_start",
                AiStreamEvent::ThinkingStart { .. } => "thinking_start",
                AiStreamEvent::ThinkingDelta { .. } => "thinking_delta",
                AiStreamEvent::ThinkingEnd { .. } => "thinking_end",
                AiStreamEvent::TextDelta { .. } => "text_delta",
                AiStreamEvent::ToolCallStart { .. } => "tool_call_start",
                AiStreamEvent::ToolCallArgsDelta { .. } => "tool_call_args_delta",
                AiStreamEvent::ToolCallEnd { .. } => "tool_call_end",
                AiStreamEvent::MessageEnd { .. } => "message_end",
            })
            .collect()
    }

    #[test]
    fn response_message_item_done_phase_promotes_buffered_reasoning_tool_text_in_order() {
        let mut parser = ParseState::default();
        ingest_buffered_reasoning_tool_text(&mut parser);
        parser
            .ingest(&json!({
                "type": "response.output_item.done",
                "item": message_item(Some("final_answer"))
            }))
            .expect("message item done should parse");
        parser
            .ingest(&json!({
                "type": "response.completed",
                "response": {"status": "completed"}
            }))
            .expect("response.completed should parse");

        let events = parser.finish_events().expect("stream should finish");
        assert_eq!(
            event_kinds(&events),
            vec![
                "message_start",
                "thinking_start",
                "thinking_delta",
                "thinking_end",
                "tool_call_start",
                "tool_call_args_delta",
                "tool_call_end",
                "text_delta",
                "message_end",
            ]
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AiStreamEvent::MessageStart { .. }))
                .count(),
            1
        );
        assert!(matches!(
            events.first(),
            Some(AiStreamEvent::MessageStart {
                phase: MessagePhase::FinalAnswer,
                ..
            })
        ));
        assert!(matches!(
            events.last(),
            Some(AiStreamEvent::MessageEnd {
                phase: MessagePhase::FinalAnswer,
                ..
            })
        ));
    }

    #[test]
    fn finish_events_is_idempotent() {
        let mut parser = ParseState::default();
        parser
            .ingest(&json!({
                "type": "response.output_text.delta",
                "delta": "text"
            }))
            .expect("output text delta should parse");

        let first = parser.finish_events().expect("first finish should succeed");
        let second = parser
            .finish_events()
            .expect("second finish should succeed");

        assert_eq!(
            first
                .iter()
                .filter(|event| matches!(event, AiStreamEvent::MessageEnd { .. }))
                .count(),
            1
        );
        assert!(second.is_empty());
    }

    #[test]
    fn buffered_terminal_events_fall_back_to_unknown_phase() {
        let mut parser = ParseState::default();
        ingest_buffered_reasoning_tool_text(&mut parser);

        let events = parser.finish_events().expect("stream should finish");
        assert_eq!(
            event_kinds(&events),
            vec![
                "message_start",
                "thinking_start",
                "thinking_delta",
                "thinking_end",
                "tool_call_start",
                "tool_call_args_delta",
                "tool_call_end",
                "text_delta",
                "message_end",
            ]
        );
        let starts = events
            .iter()
            .filter_map(|event| match event {
                AiStreamEvent::MessageStart { phase, .. } => Some(*phase),
                _ => None,
            })
            .collect::<Vec<_>>();
        let ends = events
            .iter()
            .filter_map(|event| match event {
                AiStreamEvent::MessageEnd { phase, .. } => Some(*phase),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(starts, vec![MessagePhase::Unknown]);
        assert_eq!(ends, vec![MessagePhase::Unknown]);
    }

    #[test]
    fn response_message_item_missing_or_unknown_phase_stays_unknown() {
        for done_phase in [None, Some("draft")] {
            let events = parse_message_item_events(None, done_phase);
            assert_eq!(
                events,
                vec![
                    AiStreamEvent::MessageStart {
                        id: "response-1".to_owned(),
                        phase: MessagePhase::Unknown,
                    },
                    AiStreamEvent::TextDelta {
                        text: "text".to_owned(),
                    },
                    AiStreamEvent::MessageEnd {
                        stop_reason: StopReason::EndTurn,
                        usage: None,
                        phase: MessagePhase::Unknown,
                    },
                ]
            );
        }

        let event: AiStreamEvent = serde_json::from_value(json!({
            "MessageStart": {"id": "historical"}
        }))
        .expect("missing phase should deserialize");
        assert_eq!(
            event,
            AiStreamEvent::MessageStart {
                id: "historical".to_owned(),
                phase: MessagePhase::Unknown,
            }
        );
    }

    #[test]
    fn historical_message_end_without_phase_defaults_to_unknown() {
        let event: AiStreamEvent = serde_json::from_value(json!({
            "MessageEnd": {
                "stop_reason": "EndTurn",
                "usage": null
            }
        }))
        .expect("missing phase should deserialize");
        assert_eq!(
            event,
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
                phase: MessagePhase::Unknown,
            }
        );
    }
}

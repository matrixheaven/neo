use std::collections::BTreeMap;

use futures::{StreamExt, future, stream};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};

use super::common::error::{ProviderError, stream_failure};
use super::common::helpers::{reject_images, rounded_f64, token_usage_from};
use super::common::sse::{StreamChunk, find_frame_end, parse_sse_frame};

use crate::{
    AiError, AiStreamEvent, ChatMessage, ChatRequest, ContentPart, ImageData, ModelClient,
    ReasoningEffort, ReasoningSelection, StopReason, TokenUsage, ToolSpec,
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
        self.open_response_once(&request)
            .await
            .map_err(ProviderError::into_ai_error)
    }

    async fn open_response_once(
        &self,
        request: &ChatRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let url = request_url(&self.base_url, &request.model.model)?;
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
        if !response.status().is_success() {
            return Err(super::common::http::http_status_error(response).await);
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

fn request_url(base_url: &str, model: &str) -> Result<reqwest::Url, ProviderError> {
    let model = model.strip_prefix("models/").unwrap_or(model);
    let mut url = reqwest::Url::parse(&format!("{base_url}/models/{model}:streamGenerateContent"))
        .map_err(|err| ProviderError::Url(format!("invalid Google Generative AI URL: {err}")))?;
    url.query_pairs_mut().append_pair("alt", "sse");
    Ok(url)
}

fn headers(
    api_key: &str,
    extra_headers: &BTreeMap<String, String>,
) -> Result<HeaderMap, ProviderError> {
    let mut headers = HeaderMap::new();
    super::common::http::inject_extra_headers(&mut headers, extra_headers)?;
    let mut api_key = HeaderValue::from_str(api_key)
        .map_err(|err| ProviderError::Header(format!("invalid Google API key header: {err}")))?;
    api_key.set_sensitive(true);
    headers.insert(HeaderName::from_static("x-goog-api-key"), api_key);
    Ok(headers)
}

fn request_body(request: &ChatRequest) -> Result<Value, ProviderError> {
    let mut body = json!({
        "contents": content_bodies(&request.messages, request.options.replay_reasoning)?,
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
    match &request.options.reasoning {
        ReasoningSelection::Off => {}
        ReasoningSelection::On => {
            generation_config.insert(
                "thinkingConfig".to_owned(),
                json!({
                    "includeThoughts": true,
                    "thinkingBudget": 8192,
                }),
            );
        }
        ReasoningSelection::Effort { effort } => {
            generation_config.insert(
                "thinkingConfig".to_owned(),
                json!({
                    "includeThoughts": true,
                    "thinkingBudget": thinking_budget_tokens(effort)?,
                }),
            );
        }
        ReasoningSelection::BudgetTokens { budget_tokens } => {
            generation_config.insert(
                "thinkingConfig".to_owned(),
                json!({
                    "includeThoughts": true,
                    "thinkingBudget": budget_tokens,
                }),
            );
        }
    }
    if !generation_config.is_empty() {
        body["generationConfig"] = Value::Object(generation_config);
    }

    Ok(body)
}

fn content_bodies(
    messages: &[ChatMessage],
    replay_reasoning: bool,
) -> Result<Vec<Value>, ProviderError> {
    let mut contents = Vec::new();
    let mut pending_tool_calls = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        match &messages[index] {
            ChatMessage::Assistant { tool_calls, .. } => {
                ensure_no_pending_tool_results(&pending_tool_calls)?;
                pending_tool_calls = tool_calls
                    .iter()
                    .map(|tool_call| (tool_call.id.as_str(), tool_call.name.as_str()))
                    .collect();
                if let Some(content) = content_body(&messages[index], replay_reasoning) {
                    contents.push(content?);
                }
                index += 1;
            }
            ChatMessage::ToolResult { .. } => {
                let mut results = BTreeMap::new();
                while let Some(ChatMessage::ToolResult {
                    tool_call_id,
                    content,
                    is_error,
                }) = messages.get(index)
                {
                    if !pending_tool_calls.iter().any(|(id, _)| *id == tool_call_id) {
                        return Err(ProviderError::Protocol(format!(
                            "Google tool result references unknown tool call '{tool_call_id}'"
                        )));
                    }
                    if results
                        .insert(tool_call_id.as_str(), (content.as_slice(), *is_error))
                        .is_some()
                    {
                        return Err(ProviderError::Protocol(format!(
                            "Google tool result is duplicated for tool call '{tool_call_id}'"
                        )));
                    }
                    index += 1;
                }

                let missing = pending_tool_calls
                    .iter()
                    .filter_map(|(id, _)| (!results.contains_key(id)).then_some(*id))
                    .collect::<Vec<_>>();
                if !missing.is_empty() {
                    return Err(missing_tool_results_error(&missing));
                }

                let mut parts = Vec::with_capacity(results.len());
                for (tool_call_id, tool_name) in &pending_tool_calls {
                    if let Some((content, is_error)) = results.remove(tool_call_id) {
                        parts.push(tool_result_part(tool_name, content, is_error)?);
                    }
                }
                contents.push(json!({
                    "role": "user",
                    "parts": parts,
                }));
                pending_tool_calls.clear();
            }
            _ => {
                ensure_no_pending_tool_results(&pending_tool_calls)?;
                if let Some(content) = content_body(&messages[index], replay_reasoning) {
                    contents.push(content?);
                }
                index += 1;
            }
        }
    }
    ensure_no_pending_tool_results(&pending_tool_calls)?;
    Ok(contents)
}

fn ensure_no_pending_tool_results(
    pending_tool_calls: &[(&str, &str)],
) -> Result<(), ProviderError> {
    if pending_tool_calls.is_empty() {
        return Ok(());
    }
    let missing = pending_tool_calls
        .iter()
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    Err(missing_tool_results_error(&missing))
}

fn missing_tool_results_error(missing: &[&str]) -> ProviderError {
    ProviderError::Protocol(format!(
        "Google tool results are missing for tool calls: {}",
        missing.join(", ")
    ))
}

fn thinking_budget_tokens(effort: &ReasoningEffort) -> Result<i32, ProviderError> {
    match effort.as_str() {
        ReasoningEffort::MINIMAL | ReasoningEffort::LOW => Ok(1_024),
        ReasoningEffort::MEDIUM => Ok(2_048),
        ReasoningEffort::HIGH => Ok(8_192),
        ReasoningEffort::XHIGH => Ok(16_384),
        ReasoningEffort::MAX => Ok(32_768),
        custom => Err(ProviderError::Unsupported(format!(
            "Google provider does not support custom reasoning effort '{custom}'"
        ))),
    }
}

fn content_body(
    message: &ChatMessage,
    replay_reasoning: bool,
) -> Option<Result<Value, ProviderError>> {
    match message {
        ChatMessage::System { .. } | ChatMessage::ToolResult { .. } => None,
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
                            ProviderError::Protocol(format!(
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
    }
}

fn tool_result_part(
    tool_name: &str,
    content: &[ContentPart],
    is_error: bool,
) -> Result<Value, ProviderError> {
    reject_images(content, "Google Generative AI", "tool result")?;
    Ok(json!({
        "functionResponse": {
            "name": tool_name,
            "response": {
                "result": content_text(content),
                "is_error": is_error,
            },
        },
    }))
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
                StreamChunk::Data(Err(_)) | StreamChunk::End if state.stopped => Vec::new(),
                StreamChunk::Data(Err(err)) => {
                    if state.parser.saw_terminal() {
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
        let value = serde_json::from_str::<Value>(payload).map_err(|err| AiError::Protocol {
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
        if !self.parser.saw_terminal() {
            return vec![Err(AiError::Transport {
                message: "missing terminal marker".to_owned(),
            })];
        }
        self.parser.finish_events().into_iter().map(Ok).collect()
    }
}

struct ParseState {
    events: Vec<AiStreamEvent>,
    started: bool,
    tool_args: Vec<(String, Value)>,
    tool_call_nonce: u128,
    next_thought_index: u64,
    last_stop_reason: StopReason,
    usage: Option<TokenUsage>,
    terminal: bool,
    finished: bool,
}

impl Default for ParseState {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            started: false,
            tool_args: Vec::new(),
            tool_call_nonce: rand::random(),
            next_thought_index: 0,
            last_stop_reason: StopReason::EndTurn,
            usage: None,
            terminal: false,
            finished: false,
        }
    }
}

impl ParseState {
    fn ingest(&mut self, value: &Value) -> Result<(), AiError> {
        if let Some(error) = value.get("error").and_then(Value::as_object) {
            let numeric_code = error
                .get("code")
                .and_then(Value::as_u64)
                .map(|code| code.to_string());
            let code = error
                .get("code")
                .and_then(Value::as_str)
                .or_else(|| error.get("status").and_then(Value::as_str))
                .or_else(|| error.get("type").and_then(Value::as_str))
                .or(numeric_code.as_deref());
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("provider returned an error")
                .to_owned();
            return Err(stream_failure(code, message).into_ai_error());
        }

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

    const fn saw_terminal(&self) -> bool {
        self.terminal
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
            self.terminal = true;
            let stop_reason = if self.tool_args.is_empty() {
                stop_reason(reason)
            } else {
                StopReason::ToolUse
            };
            if !self.started && matches!(stop_reason, StopReason::Error) {
                return Err(AiError::Protocol {
                    message: format!("google response finished without content: {reason}"),
                });
            }
            self.last_stop_reason = stop_reason;
            self.ensure_started();
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
        let fragment = serde_json::to_string(&args).map_err(|err| AiError::Protocol {
            message: format!("invalid tool arguments: {err}"),
        })?;

        self.ensure_started();
        let id = format!(
            "google-tool:{:032x}:{}",
            self.tool_call_nonce,
            self.tool_args.len()
        );
        self.events.push(AiStreamEvent::ToolCallStart {
            id: id.clone(),
            name,
        });
        self.tool_args.push((id.clone(), args));
        self.events.push(AiStreamEvent::ToolCallArgsDelta {
            id,
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
    fn api_key_header_is_sensitive() {
        let headers = headers("secret-key", &BTreeMap::new()).unwrap();

        assert!(
            headers
                .get("x-goog-api-key")
                .expect("API key header should be present")
                .is_sensitive()
        );
    }

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
            matches!(err, ProviderError::Protocol(message) if message.contains("invalid raw tool arguments"))
        );
    }

    #[test]
    fn duplicate_function_calls_keep_unique_ids_and_replay_names() {
        let mut parser = ParseState {
            tool_call_nonce: 1,
            ..ParseState::default()
        };
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [
                        { "functionCall": { "name": "read", "args": { "path": "a" } } },
                        { "functionCall": { "name": "read", "args": { "path": "b" } } }
                    ]
                },
                "finishReason": "STOP"
            }]
        });
        parser.ingest(&response).unwrap();
        let mut events = parser.drain_events();
        events.extend(parser.finish_events());

        let ids = events
            .iter()
            .filter_map(|event| match event {
                AiStreamEvent::ToolCallStart { id, name } if name == "read" => Some(id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);

        for (id, arguments) in ids.iter().zip([r#"{"path":"a"}"#, r#"{"path":"b"}"#]) {
            assert!(events.contains(&AiStreamEvent::ToolCallArgsDelta {
                id: id.clone(),
                json_fragment: arguments.to_owned(),
            }));
            assert!(events.contains(&AiStreamEvent::ToolCallEnd {
                id: id.clone(),
                raw_arguments: arguments.to_owned(),
            }));
        }

        let mut second_parser = ParseState {
            tool_call_nonce: 2,
            ..ParseState::default()
        };
        second_parser
            .ingest(&json!({
                "candidates": [{
                    "content": {
                        "parts": [
                            { "functionCall": { "name": "write", "args": { "path": "c" } } }
                        ]
                    },
                    "finishReason": "STOP"
                }]
            }))
            .unwrap();
        let second_id = second_parser
            .drain_events()
            .into_iter()
            .find_map(|event| match event {
                AiStreamEvent::ToolCallStart { id, name } if name == "write" => Some(id),
                _ => None,
            })
            .unwrap();
        assert!(!ids.contains(&second_id));

        assert_content_bodies_replay(&ids, second_id);
    }

    fn assert_content_bodies_replay(ids: &[String], second_id: String) {
        let messages = vec![
            ChatMessage::Assistant {
                content: Vec::new(),
                tool_calls: ids
                    .iter()
                    .zip([r#"{"path":"a"}"#, r#"{"path":"b"}"#])
                    .map(|(id, raw_arguments)| ToolCall {
                        id: id.clone(),
                        name: "read".to_owned(),
                        raw_arguments: raw_arguments.to_owned(),
                    })
                    .collect(),
            },
            ChatMessage::ToolResult {
                tool_call_id: ids[1].clone(),
                content: vec![ContentPart::Text {
                    text: "second".to_owned(),
                }],
                is_error: false,
            },
            ChatMessage::ToolResult {
                tool_call_id: ids[0].clone(),
                content: vec![ContentPart::Text {
                    text: "first".to_owned(),
                }],
                is_error: false,
            },
            ChatMessage::Assistant {
                content: Vec::new(),
                tool_calls: vec![ToolCall {
                    id: second_id.clone(),
                    name: "write".to_owned(),
                    raw_arguments: r#"{"path":"c"}"#.to_owned(),
                }],
            },
            ChatMessage::ToolResult {
                tool_call_id: second_id,
                content: vec![ContentPart::Text {
                    text: "written".to_owned(),
                }],
                is_error: false,
            },
        ];
        let contents = content_bodies(&messages, false).unwrap();
        assert_eq!(contents.len(), 4);
        assert_eq!(contents[1]["role"], "user");
        assert_eq!(contents[1]["parts"][0]["functionResponse"]["name"], "read");
        assert_eq!(
            contents[1]["parts"][0]["functionResponse"]["response"]["result"],
            "first"
        );
        assert_eq!(contents[1]["parts"][1]["functionResponse"]["name"], "read");
        assert_eq!(
            contents[1]["parts"][1]["functionResponse"]["response"]["result"],
            "second"
        );
        assert_eq!(contents[3]["role"], "user");
        assert_eq!(contents[3]["parts"][0]["functionResponse"]["name"], "write");

        let unknown = vec![ChatMessage::ToolResult {
            tool_call_id: "unknown".to_owned(),
            content: vec![ContentPart::Text {
                text: "result".to_owned(),
            }],
            is_error: false,
        }];
        let err = content_bodies(&unknown, false).unwrap_err();
        assert!(matches!(
            err,
            ProviderError::Protocol(message) if message.contains("unknown tool call 'unknown'")
        ));
    }

    #[test]
    fn tool_results_reject_incomplete_parallel_batch() {
        let messages = vec![
            ChatMessage::Assistant {
                content: Vec::new(),
                tool_calls: vec![
                    ToolCall {
                        id: "call-1".to_owned(),
                        name: "read".to_owned(),
                        raw_arguments: "{}".to_owned(),
                    },
                    ToolCall {
                        id: "call-2".to_owned(),
                        name: "write".to_owned(),
                        raw_arguments: "{}".to_owned(),
                    },
                ],
            },
            ChatMessage::ToolResult {
                tool_call_id: "call-1".to_owned(),
                content: vec![ContentPart::Text {
                    text: "done".to_owned(),
                }],
                is_error: false,
            },
        ];

        let err = content_bodies(&messages, false).unwrap_err();
        assert!(matches!(
            err,
            ProviderError::Protocol(message)
                if message == "Google tool results are missing for tool calls: call-2"
        ));
    }

    #[test]
    fn tool_results_reject_zero_result_batch_at_end_of_history() {
        let messages = vec![ChatMessage::Assistant {
            content: Vec::new(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_owned(),
                name: "read".to_owned(),
                raw_arguments: "{}".to_owned(),
            }],
        }];

        let err = content_bodies(&messages, false).unwrap_err();
        assert!(matches!(
            err,
            ProviderError::Protocol(message)
                if message == "Google tool results are missing for tool calls: call-1"
        ));
    }

    #[test]
    fn content_free_stop_emits_balanced_message() {
        let mut parser = IncrementalSse::default();
        let body = format!(
            "data: {}\n\n",
            serde_json::json!({ "candidates": [{ "finishReason": "STOP" }] })
        );
        let mut events = parser.push_chunk(body.as_bytes());
        events.extend(parser.finish());
        let events = events.into_iter().collect::<Result<Vec<_>, _>>().unwrap();

        assert_eq!(
            events,
            vec![
                AiStreamEvent::MessageStart {
                    id: "google-generative-ai".to_owned(),
                },
                AiStreamEvent::MessageEnd {
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                },
            ]
        );
    }

    #[test]
    fn content_free_error_finish_is_protocol() {
        let mut parser = IncrementalSse::default();
        let body = format!(
            "data: {}\n\n",
            serde_json::json!({ "candidates": [{ "finishReason": "SAFETY" }] })
        );
        let error = parser
            .push_chunk(body.as_bytes())
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap_err();

        assert!(matches!(error, AiError::Protocol { .. }));
    }
}

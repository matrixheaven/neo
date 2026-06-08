use std::collections::{BTreeMap, BTreeSet};

use futures::{StreamExt, future, stream};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};

use crate::{
    AiError, AiStreamEvent, ChatMessage, ChatRequest, ContentPart, ImageData, ModelClient,
    StopReason, TokenUsage, ToolSpec,
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
            return Err(ProviderError::HttpStatus(status.as_u16()));
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

#[derive(Debug)]
enum ProviderError {
    Header(String),
    HttpStatus(u16),
    Transport(reqwest::Error),
    Url(String),
    Unsupported(String),
}

impl ProviderError {
    const fn is_retryable(&self) -> bool {
        match self {
            Self::HttpStatus(status) => *status == 429 || *status >= 500,
            Self::Transport(_) => true,
            Self::Header(_) | Self::Url(_) | Self::Unsupported(_) => false,
        }
    }

    fn into_ai_error(self) -> AiError {
        match self {
            Self::Header(message) | Self::Url(message) | Self::Unsupported(message) => {
                AiError::Stream(message)
            }
            Self::HttpStatus(status) => AiError::Stream(format!("http status {status}")),
            Self::Transport(err) => AiError::Stream(format!("transport error: {err}")),
        }
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
        "contents": request
            .messages
            .iter()
            .filter_map(content_body)
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
    if !generation_config.is_empty() {
        body["generationConfig"] = Value::Object(generation_config);
    }

    Ok(body)
}

fn content_body(message: &ChatMessage) -> Option<Result<Value, ProviderError>> {
    match message {
        ChatMessage::System { .. } => None,
        ChatMessage::User { content } => Some(content_parts(content).map(|parts| {
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
            Some(content_parts(content).map(move |mut parts| {
                for tool_call in &tool_calls {
                    parts.push(json!({
                        "functionCall": {
                            "name": tool_call.name,
                            "args": tool_call.arguments,
                        },
                    }));
                }
                json!({
                    "role": "model",
                    "parts": parts,
                })
            }))
        }
        ChatMessage::ToolResult {
            tool_call_id,
            content,
            is_error,
        } => Some(reject_images(content, "tool result").map(|()| {
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
        })),
    }
}

fn content_parts(content: &[ContentPart]) -> Result<Vec<Value>, ProviderError> {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => Ok(json!({ "text": text })),
            ContentPart::Image { mime_type, data } => match data {
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
            },
        })
        .collect()
}

fn text_parts(content: &[ContentPart]) -> Result<Vec<Value>, ProviderError> {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => Ok(json!({ "text": text })),
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
            ContentPart::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn reject_images(content: &[ContentPart], role: &str) -> Result<(), ProviderError> {
    if content
        .iter()
        .any(|part| matches!(part, ContentPart::Image { .. }))
    {
        return Err(ProviderError::Unsupported(format!(
            "Google Generative AI image content is only supported in user/model messages, not {role} messages"
        )));
    }
    Ok(())
}

fn tool_body(tool: &ToolSpec) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
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
        let value = serde_json::from_str::<Value>(payload)
            .map_err(|err| AiError::Stream(format!("invalid SSE JSON: {err}")))?;
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
    tool_args: BTreeMap<String, Value>,
    open_tool_ids: BTreeSet<String>,
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
            .and_then(token_usage)
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
        let fragment = serde_json::to_string(&args)
            .map_err(|err| AiError::Stream(format!("invalid tool arguments: {err}")))?;

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
                arguments: arguments.clone(),
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

fn token_usage(value: &Value) -> Option<TokenUsage> {
    Some(TokenUsage {
        input_tokens: u32::try_from(value.get("promptTokenCount")?.as_u64()?).ok()?,
        output_tokens: u32::try_from(value.get("candidatesTokenCount")?.as_u64()?).ok()?,
    })
}

fn stop_reason(reason: &str) -> StopReason {
    match reason {
        "MAX_TOKENS" => StopReason::MaxTokens,
        "SAFETY" | "RECITATION" | "SPII" | "MALFORMED_FUNCTION_CALL" => StopReason::Error,
        _ => StopReason::EndTurn,
    }
}

fn rounded_f64(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

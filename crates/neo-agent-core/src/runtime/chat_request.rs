use neo_ai::{
    AiError, ApiKind, ChatMessage, ChatRequest, EffectiveMediaCapability, MediaKind, MediaPosition,
    MediaTransportCapabilities, MediaTransportMode, ModelCapabilities, RequestOptions,
    effective_media_capability,
};

use super::config::AgentConfig;
use super::context::AgentContext;
use super::image_blobs::resolve_media_blobs;
use crate::{AgentMessage, Content, MediaRef, sanitize_tool_exchange_messages};

/// How one (media kind × message position) cell is projected into the request
/// copy. Every unsendable decision is explicit — never a silent drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaProjectionMode {
    /// Media stays in place and is sent with the cell's transport mode.
    SendInPlace,
    /// Tool-result media moves to a user message appended after the whole
    /// tool exchange; it is never inserted between call and results.
    AttachAfterExchange,
    /// Media is replaced by a fixed, digest-anchored description. The blob is
    /// never read or encoded, so the text is identical for every request in
    /// the same lane.
    FixedDescription,
}

/// Projection decision for one (media kind × message position) cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MediaProjectionCell {
    mode: MediaProjectionMode,
    /// Fixed reason template (with a `{kind}` placeholder) for
    /// `FixedDescription` cells; `None` for other modes.
    description_reason: Option<&'static str>,
}

const UNSUPPORTED_POSITION_CELL: MediaProjectionCell = MediaProjectionCell {
    mode: MediaProjectionMode::FixedDescription,
    description_reason: Some("media is not supported in this message position"),
};

impl MediaProjectionCell {
    const fn send_in_place() -> Self {
        Self {
            mode: MediaProjectionMode::SendInPlace,
            description_reason: None,
        }
    }

    const fn attach_after_exchange() -> Self {
        Self {
            mode: MediaProjectionMode::AttachAfterExchange,
            description_reason: None,
        }
    }

    const fn description(reason: &'static str) -> Self {
        Self {
            mode: MediaProjectionMode::FixedDescription,
            description_reason: Some(reason),
        }
    }

    fn user_cell(
        kind: MediaKind,
        model: &ModelCapabilities,
        transport: MediaTransportCapabilities,
    ) -> Self {
        match effective_media_capability(kind, MediaPosition::UserMessage, model, transport) {
            EffectiveMediaCapability::Sendable(_) => Self::send_in_place(),
            EffectiveMediaCapability::ModelRejectsMediaKind => {
                Self::description("current model does not support {kind} input")
            }
            EffectiveMediaCapability::TransportUnsupported => {
                Self::description("provider cannot transport {kind} in user messages")
            }
        }
    }

    fn tool_cell(
        kind: MediaKind,
        model: &ModelCapabilities,
        transport: MediaTransportCapabilities,
    ) -> Self {
        let tool = effective_media_capability(kind, MediaPosition::ToolResult, model, transport);
        let user = effective_media_capability(kind, MediaPosition::UserMessage, model, transport);
        match (tool, user) {
            (EffectiveMediaCapability::Sendable(MediaTransportMode::AttachAfterResult), _) => {
                Self::attach_after_exchange()
            }
            (EffectiveMediaCapability::Sendable(_), _) => Self::send_in_place(),
            // The model rejects the kind at the tool position; the user cell
            // then rejects it too, because capability is model-first.
            (EffectiveMediaCapability::ModelRejectsMediaKind, _) => {
                Self::description("current model does not support {kind} input")
            }
            // The model accepts the kind and the user cell can carry it: the
            // media is appended as a user message after the full exchange.
            (
                EffectiveMediaCapability::TransportUnsupported,
                EffectiveMediaCapability::Sendable(_),
            ) => Self::attach_after_exchange(),
            (EffectiveMediaCapability::TransportUnsupported, _) => {
                Self::description("provider cannot transport {kind} in tool results")
            }
        }
    }
}

/// How historical media tool exchanges are projected into the request copy.
///
/// Historical `ReadMediaFile` calls and their results are facts of the
/// session and are never deleted; this decision only controls the shape of
/// the replayed exchange. The lane-key shape derives from this decision so
/// two different exchange projections never share a cache lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExchangeProjection {
    /// The full exchange (assistant call + all matching results) replays
    /// verbatim in the provider wire format. Every current adapter takes this
    /// path; the per-provider request-body fixed-point tests prove the
    /// serialized shape.
    Preserve,
    /// The whole exchange is converted to stable text when a provider cannot
    /// legally replay calls for tools absent from the current tool table.
    /// Not activated by any current adapter; the decision type exists so the
    /// lane shape can distinguish it once an adapter needs it.
    #[allow(dead_code)] // reserved decision path; see the variant doc comment
    ConvertToText,
}

impl ExchangeProjection {
    /// Stable lane-shape label; part of the cache lane key format.
    fn lane_label(self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::ConvertToText => "convert",
        }
    }
}

/// Static projection plan for every (kind, position) cell plus the historical
/// tool-exchange decision. The plan is a pure function of model semantic
/// capabilities, provider transport capabilities and the exchange projection
/// decision, so it is identical for every request in one lane and never
/// changes as history grows.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MediaProjectionPlan {
    user_image: MediaProjectionCell,
    user_video: MediaProjectionCell,
    tool_image: MediaProjectionCell,
    tool_video: MediaProjectionCell,
    exchange: ExchangeProjection,
}

impl MediaProjectionPlan {
    fn compute(
        model: &ModelCapabilities,
        transport: MediaTransportCapabilities,
        exchange: ExchangeProjection,
    ) -> MediaProjectionPlan {
        MediaProjectionPlan {
            user_image: MediaProjectionCell::user_cell(MediaKind::Image, model, transport),
            user_video: MediaProjectionCell::user_cell(MediaKind::Video, model, transport),
            tool_image: MediaProjectionCell::tool_cell(MediaKind::Image, model, transport),
            tool_video: MediaProjectionCell::tool_cell(MediaKind::Video, model, transport),
            exchange,
        }
    }

    fn cell(&self, kind: MediaKind, position: MediaPosition) -> &MediaProjectionCell {
        match (kind, position) {
            (MediaKind::Image, MediaPosition::UserMessage) => &self.user_image,
            (MediaKind::Video, MediaPosition::UserMessage) => &self.user_video,
            (MediaKind::Image, MediaPosition::ToolResult) => &self.tool_image,
            (MediaKind::Video, MediaPosition::ToolResult) => &self.tool_video,
        }
    }
}

/// Where a media part sits inside one message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectionPosition {
    UserMessage,
    ToolResult,
    /// System and assistant messages have no declared transport cell; media
    /// there is never sendable.
    UnsupportedMessage,
}

/// Projected outcome for one media part.
enum ProjectedMedia {
    /// Part stays in the message untouched (sendable; blob resolution happens
    /// later and only for parts that survive projection).
    Keep(Content),
    /// Part moves to the user message appended after the current exchange.
    MoveToAttach(Content),
    /// Part is replaced by a fixed description.
    Description(Content),
}

pub(super) async fn chat_request(
    config: &AgentConfig,
    context: &AgentContext,
    transport: MediaTransportCapabilities,
) -> Result<ChatRequest, AiError> {
    let mut messages = Vec::new();
    if let Some(system_prompt) = &config.system_prompt {
        messages.push(AgentMessage::system_text(system_prompt.as_str()).to_chat_message());
    }
    if let Some(workspace_context) = workspace_context_message(config) {
        messages.push(workspace_context.to_chat_message());
    }
    let mut context_messages = context.messages.clone();
    if let Some(transform) = &config.context_append_transform {
        context_messages.extend(transform(context.messages()));
    }
    // Repair tool exchanges first so the media projection only ever sees
    // complete exchanges and can append after them.
    let context_messages = sanitize_tool_exchange_messages(&context_messages).into_owned();
    // Decide media projection before touching the blob store: unsendable media
    // becomes a fixed description and its blob is never read.
    let (context_messages, plan) = project_request_media(
        context_messages,
        &config.model.capabilities,
        transport,
        // Every current adapter replays historical media tool exchanges
        // verbatim (proved by the per-provider request-body fixed-point
        // tests); the convert-to-text decision is reserved for adapters that
        // cannot legally replay calls for tools absent from the tool table.
        ExchangeProjection::Preserve,
    );
    // Resolve blob references to inline base64 only for media that survived
    // the projection (i.e. sendable media).
    let context_messages =
        resolve_media_blobs(context_messages, config.session_directory.as_deref()).await?;
    for message in &context_messages {
        messages.push(if config.replay_reasoning {
            message.to_chat_message()
        } else {
            without_reasoning_content(message.to_chat_message())
        });
    }
    let session_id = session_directory_name(config);
    Ok(ChatRequest {
        model: config.model.clone(),
        messages,
        tools: config.tools.clone(),
        options: RequestOptions {
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            reasoning: config.reasoning.clone(),
            replay_reasoning: config.replay_reasoning,
            session_id: session_id.clone(),
            prompt_cache_key: lane_cache_key(config, &plan, transport, session_id.as_deref()),
            response_format: config.response_format.clone(),
            ..RequestOptions::default()
        },
    })
}

/// Project media on a request copy. Only media parts are touched: text,
/// thinking, tool calls, tool-result text, call ids and the N follow-up turns
/// keep their original order and content.
fn project_request_media(
    messages: Vec<AgentMessage>,
    model: &ModelCapabilities,
    transport: MediaTransportCapabilities,
    exchange: ExchangeProjection,
) -> (Vec<AgentMessage>, MediaProjectionPlan) {
    let plan = MediaProjectionPlan::compute(model, transport, exchange);
    let mut out = Vec::with_capacity(messages.len());
    let mut attached = Vec::new();
    for message in messages {
        // Tool results of one exchange are consecutive (sanitized above), so
        // flushing on the first non-tool-result message appends exactly after
        // the complete exchange — never between call and results.
        if !matches!(message, AgentMessage::ToolResult { .. }) && !attached.is_empty() {
            out.push(AgentMessage::user_content(std::mem::take(&mut attached)));
        }
        out.push(project_message(message, &plan, &mut attached));
    }
    if !attached.is_empty() {
        out.push(AgentMessage::user_content(attached));
    }
    (out, plan)
}

fn project_message(
    message: AgentMessage,
    plan: &MediaProjectionPlan,
    attached: &mut Vec<Content>,
) -> AgentMessage {
    match message {
        AgentMessage::System { content } => AgentMessage::System {
            content: project_content(content, ProjectionPosition::UnsupportedMessage, plan),
        },
        AgentMessage::User {
            content,
            display_text,
            origin,
        } => AgentMessage::User {
            content: project_content(content, ProjectionPosition::UserMessage, plan),
            display_text,
            origin,
        },
        AgentMessage::Assistant {
            content,
            tool_calls,
            stop_reason,
        } => AgentMessage::Assistant {
            content: project_content(content, ProjectionPosition::UnsupportedMessage, plan),
            tool_calls,
            stop_reason,
        },
        AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            is_error,
        } => {
            let mut projected = Vec::with_capacity(content.len());
            let mut moved = Vec::new();
            for part in content {
                match project_media_part(part, ProjectionPosition::ToolResult, plan) {
                    ProjectedMedia::Keep(part) | ProjectedMedia::Description(part) => {
                        projected.push(part);
                    }
                    ProjectedMedia::MoveToAttach(part) => moved.push(part),
                }
            }
            // Degenerate boundary: a result whose parts were all media and all
            // attached would otherwise serialize as empty content, which some
            // protocols reject. Keep a stable, digest-anchored marker so the
            // media is never silently dropped and the result stays honest.
            if projected.is_empty() && !moved.is_empty() {
                projected.push(attached_media_marker(&moved));
            }
            attached.extend(moved);
            AgentMessage::ToolResult {
                tool_call_id,
                tool_name,
                content: projected,
                is_error,
            }
        }
        AgentMessage::ShellCommand {
            command,
            stdout,
            stderr,
            exit_code,
            outcome,
            truncated,
        } => AgentMessage::ShellCommand {
            command,
            stdout,
            stderr,
            exit_code,
            outcome,
            truncated,
        },
    }
}

fn project_content(
    content: Vec<Content>,
    position: ProjectionPosition,
    plan: &MediaProjectionPlan,
) -> Vec<Content> {
    let mut projected = Vec::with_capacity(content.len());
    for part in content {
        match project_media_part(part, position, plan) {
            ProjectedMedia::Keep(part) | ProjectedMedia::Description(part) => projected.push(part),
            ProjectedMedia::MoveToAttach(_) => unreachable!("attach only applies to tool results"),
        }
    }
    projected
}

fn project_media_part(
    part: Content,
    position: ProjectionPosition,
    plan: &MediaProjectionPlan,
) -> ProjectedMedia {
    let kind = match &part {
        Content::Image { .. } => MediaKind::Image,
        Content::Video { .. } => MediaKind::Video,
        _ => return ProjectedMedia::Keep(part),
    };
    let cell = match position {
        ProjectionPosition::UserMessage => plan.cell(kind, MediaPosition::UserMessage),
        ProjectionPosition::ToolResult => plan.cell(kind, MediaPosition::ToolResult),
        ProjectionPosition::UnsupportedMessage => &UNSUPPORTED_POSITION_CELL,
    };
    match cell.mode {
        MediaProjectionMode::SendInPlace => ProjectedMedia::Keep(part),
        MediaProjectionMode::AttachAfterExchange => ProjectedMedia::MoveToAttach(part),
        MediaProjectionMode::FixedDescription => {
            let (Content::Image { data, .. } | Content::Video { data, .. }) = &part else {
                unreachable!("media kind matched above");
            };
            ProjectedMedia::Description(media_description(kind, data, cell))
        }
    }
}

/// Fixed, path-independent, lane-stable "media not sent" description. The
/// digest anchors the description to the media without reading its bytes.
fn media_description(kind: MediaKind, data: &MediaRef, cell: &MediaProjectionCell) -> Content {
    let kind_label = kind_label(kind);
    let reason = cell
        .description_reason
        .unwrap_or("media was not sent")
        .replace("{kind}", kind_label);
    Content::text(format!(
        "[media not sent: {kind_label} {}; {reason}]",
        media_digest(data)
    ))
}

/// Stable marker for a tool result whose media parts all moved to the user
/// message attached after the exchange. The marker is deterministic per lane
/// (digest-anchored, path-independent) and keeps the result non-empty without
/// claiming the media was read here.
fn attached_media_marker(moved: &[Content]) -> Content {
    let parts = moved
        .iter()
        .filter_map(|part| match part {
            Content::Image { data, .. } => Some(format!("image {}", media_digest(data))),
            Content::Video { data, .. } => Some(format!("video {}", media_digest(data))),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(", ");
    Content::text(format!(
        "[media attached after this tool exchange: {parts}; sent as a user message]"
    ))
}

fn kind_label(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "image",
        MediaKind::Video => "video",
    }
}

/// Stable digest for one media reference: the blob SHA-256 for blob refs, the
/// SHA-256 of the encoded value for base64/URL refs. Deterministic per lane.
fn media_digest(data: &MediaRef) -> String {
    match data {
        MediaRef::Blob(sha) => sha.to_string(),
        MediaRef::Base64(value) | MediaRef::Url(value) => {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(value.as_bytes());
            format!("{:x}", hasher.finalize())
        }
    }
}

pub(super) fn workspace_context_message(config: &AgentConfig) -> Option<AgentMessage> {
    let workspace_root = config.workspace_root.as_ref()?;
    Some(AgentMessage::system_text(format!(
        "Runtime Context\n\
         - cwd: {}\n\
         - Read may accept absolute paths when the user asks for them or the task requires them.\n\
         - Write, Edit, Bash, and Terminal are governed by Neo's permission layer; write and shell tools are constrained by workspace permissions.\n\
         - Shell tools already run in this workspace. Do not prefix shell commands with `cd <cwd> &&`; use the bash `cwd` field for a workspace subdirectory.\n\
         - Commands that work inside a nested project subtree must set the tool's typed `cwd` field (Bash, Terminal start) to that subtree. Command text is never inspected for paths, so nested AGENTS.md instructions load only from typed `cwd`/path arguments.\n\
         - Network access is not a separate Neo prompt guarantee; it depends on the available tools, host environment, and permission decisions.\n\
         - If an approval is denied, treat it as the user's decision and choose a different safe path instead of retrying the same request.",
        workspace_root.display()
    )))
}

fn session_directory_name(config: &AgentConfig) -> Option<String> {
    config
        .session_directory
        .as_ref()?
        .file_name()?
        .to_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Cache lane key: session directory identity + provider instance identity +
/// model identity + static projection shape. It never contains history
/// content, media bytes, the current turn input, or any per-turn field, so
/// one lane keeps one key as history grows.
///
/// Every component is length-prefixed (`{len}:{value}`), which makes the key
/// injective over its component tuple: two different (session, provider,
/// model, shape) tuples always encode to different strings, so user-controlled
/// provider/model ids can never alias two lanes.
fn lane_cache_key(
    config: &AgentConfig,
    plan: &MediaProjectionPlan,
    transport: MediaTransportCapabilities,
    session_id: Option<&str>,
) -> Option<String> {
    let session = session_id?;
    let shape = projection_shape(plan, transport, config.model.api);
    Some(format!(
        "{}{}{}{}",
        length_prefixed(session),
        length_prefixed(&config.model.provider.0),
        length_prefixed(&config.model.model),
        length_prefixed(&shape),
    ))
}

/// Self-delimiting lane-key segment: decimal byte length, a colon, then the
/// value. Concatenated segments are uniquely decodable, so the lane key is
/// collision-free over its component tuple regardless of component content.
fn length_prefixed(value: &str) -> String {
    format!("{}:{value}", value.len())
}

/// Static projection shape: wire protocol, per-cell projection decision
/// (including the transport mode of sendable cells), and the historical
/// tool-exchange projection mode. Independent of message content.
fn projection_shape(
    plan: &MediaProjectionPlan,
    transport: MediaTransportCapabilities,
    protocol: ApiKind,
) -> String {
    let cell =
        |kind: MediaKind, position: MediaPosition, cell: &MediaProjectionCell| match cell.mode {
            MediaProjectionMode::SendInPlace => {
                format!(
                    "send:{}",
                    transport_lane_label(transport.mode(kind, position))
                )
            }
            MediaProjectionMode::AttachAfterExchange => "attach:user-message".to_owned(),
            MediaProjectionMode::FixedDescription => "description".to_owned(),
        };
    format!(
        "proto={protocol:?};user_image={};user_video={};tool_image={};tool_video={};exchange={}",
        cell(
            MediaKind::Image,
            MediaPosition::UserMessage,
            &plan.user_image
        ),
        cell(
            MediaKind::Video,
            MediaPosition::UserMessage,
            &plan.user_video
        ),
        cell(
            MediaKind::Image,
            MediaPosition::ToolResult,
            &plan.tool_image
        ),
        cell(
            MediaKind::Video,
            MediaPosition::ToolResult,
            &plan.tool_video
        ),
        plan.exchange.lane_label(),
    )
}

fn transport_lane_label(mode: MediaTransportMode) -> &'static str {
    match mode {
        MediaTransportMode::Inline => "inline",
        MediaTransportMode::Url => "url",
        MediaTransportMode::FileRef => "file-ref",
        MediaTransportMode::InPlace => "in-place",
        MediaTransportMode::AttachAfterResult => "attach-after-result",
        MediaTransportMode::Unsupported => "unsupported",
    }
}

fn without_reasoning_content(message: ChatMessage) -> ChatMessage {
    match message {
        ChatMessage::System { content } => ChatMessage::System {
            content: filter_reasoning(content),
        },
        ChatMessage::User { content } => ChatMessage::User {
            content: filter_reasoning(content),
        },
        ChatMessage::Assistant {
            content,
            tool_calls,
        } => ChatMessage::Assistant {
            content: filter_reasoning(content),
            tool_calls,
        },
        ChatMessage::ToolResult {
            tool_call_id,
            content,
            is_error,
        } => ChatMessage::ToolResult {
            tool_call_id,
            content: filter_reasoning(content),
            is_error,
        },
    }
}

fn filter_reasoning(content: Vec<neo_ai::ContentPart>) -> Vec<neo_ai::ContentPart> {
    content
        .into_iter()
        .filter(|part| !matches!(part, neo_ai::ContentPart::Thinking { .. }))
        .collect()
}

pub(super) fn validate_model_capabilities(request: &ChatRequest) -> Result<(), AiError> {
    let capabilities = &request.model.capabilities;
    if !request.tools.is_empty() && !capabilities.tools {
        return Err(AiError::Configuration {
            message: format!(
                "model {}/{} does not support tools",
                request.model.provider.0, request.model.model
            ),
        });
    }
    if !capabilities.reasoning.supports(&request.options.reasoning) {
        return Err(AiError::Configuration {
            message: format!(
                "model {}/{} does not support reasoning selection {:?}; capability is {:?}",
                request.model.provider.0,
                request.model.model,
                request.options.reasoning,
                capabilities.reasoning
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "test_cases/chat_request.rs"]
mod tests;

#[cfg(test)]
#[path = "test_cases/media_projection.rs"]
mod media_projection_tests;

//! Cross-module model-switch behavior for media sessions (design §7, §9.2,
//! §10): A (video-capable) → B (no video) → A over a live `AgentRuntime`,
//! cache lane identity, compaction boundary, in-flight switch isolation, and
//! delegate child inheritance of the media tool.
//!
//! The per-cell projection parameter matrix and blob-boundary errors are
//! covered by `runtime::test_cases::media_projection` and the `ReadMediaFile`
//! tool tests; this target covers only the runtime-level risks: model
//! switching, lane stability, compaction, and in-flight routing.
#![allow(clippy::similar_names)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, CompactionSettings, Content,
    MediaRef, ToolRegistry, harness::FakeHarness, multi_agent::AgentRole, tools::ReadMediaFileTool,
};
use neo_ai::{
    AiError, AiStreamEvent, ApiKind, ChatMessage, ChatRequest, ContentPart, ImageData,
    MediaTransportCapabilities, MediaTransportMode, MessagePhase, ModelCapabilities, ModelClient,
    ModelSpec, ProviderId, StopReason,
};
use serde_json::json;
use tokio::time::{sleep, timeout};

/// Minimal MP4 signatures (ftyp + known major brand) accepted by the media
/// sniffer; one distinct file per exchange so new reads produce new shas.
const MP4_BYTES: &[u8] = b"\x00\x00\x00\x18ftypmp42filler-bytes-for-mp4";
const SECOND_MP4_BYTES: &[u8] = b"\x00\x00\x00\x18ftypisomfiller-bytes-for-second";

/// Transport that carries every (kind, position) cell inline, the same shape
/// `FakeModelClient` declares for deterministic projection tests.
fn all_inline_transport() -> MediaTransportCapabilities {
    MediaTransportCapabilities {
        user_image: MediaTransportMode::Inline,
        user_video: MediaTransportMode::Inline,
        tool_image: MediaTransportMode::InPlace,
        tool_video: MediaTransportMode::InPlace,
    }
}

fn media_model(name: &str, images: bool, videos: bool) -> ModelSpec {
    ModelSpec {
        provider: ProviderId("switch-test".to_owned()),
        model: name.to_owned(),
        api: ApiKind::Local,
        capabilities: ModelCapabilities {
            images,
            videos,
            ..ModelCapabilities::tool_chat()
        },
    }
}

/// One scripted step of a fake turn.
#[derive(Clone)]
enum ScriptStep {
    Event(AiStreamEvent),
}

/// Recording fake client: pops one scripted turn per request, records every
/// request, and declares the all-inline media transport.
struct SwitchClient {
    scripts: Mutex<VecDeque<Vec<ScriptStep>>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl ModelClient for SwitchClient {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request);
        let steps = self
            .scripts
            .lock()
            .expect("scripts lock poisoned")
            .pop_front()
            .unwrap_or_default();
        futures::stream::unfold(steps.into_iter(), |mut steps| async move {
            match steps.next()? {
                ScriptStep::Event(event) => Some((Ok(event), steps)),
            }
        })
        .boxed()
    }

    fn media_transport(&self) -> MediaTransportCapabilities {
        all_inline_transport()
    }
}

/// Model spec + its own recording client; one harness per model lane.
struct SwitchHarness {
    model: ModelSpec,
    client: Arc<SwitchClient>,
}

impl SwitchHarness {
    fn new(model: ModelSpec, scripts: Vec<Vec<ScriptStep>>) -> Self {
        Self {
            model,
            client: Arc::new(SwitchClient {
                scripts: Mutex::new(scripts.into()),
                requests: Mutex::default(),
            }),
        }
    }

    fn model(&self) -> ModelSpec {
        self.model.clone()
    }

    fn client(&self) -> Arc<dyn ModelClient> {
        self.client.clone()
    }

    fn requests(&self) -> Vec<ChatRequest> {
        self.client
            .requests
            .lock()
            .expect("request lock poisoned")
            .clone()
    }
}

fn text_script(text: &str) -> Vec<ScriptStep> {
    vec![
        ScriptStep::Event(AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg".to_owned(),
        }),
        ScriptStep::Event(AiStreamEvent::TextDelta {
            text: text.to_owned(),
        }),
        ScriptStep::Event(AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        }),
    ]
}

fn tool_call_script(id: &str, name: &str, arguments: &serde_json::Value) -> Vec<ScriptStep> {
    vec![
        ScriptStep::Event(AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_tool".to_owned(),
        }),
        ScriptStep::Event(AiStreamEvent::ToolCallStart {
            id: id.to_owned(),
            name: name.to_owned(),
        }),
        ScriptStep::Event(AiStreamEvent::ToolCallEnd {
            id: id.to_owned(),
            raw_arguments: arguments.to_string(),
        }),
        ScriptStep::Event(AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: StopReason::ToolUse,
            usage: None,
        }),
    ]
}

struct SessionFixture {
    _tmp: tempfile::TempDir,
    workspace: PathBuf,
    session_dir: PathBuf,
}

fn session_with_media_files() -> SessionFixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("clip.mp4"), MP4_BYTES).expect("write clip");
    std::fs::write(workspace.join("second.mp4"), SECOND_MP4_BYTES).expect("write second clip");
    let session_dir = tmp.path().join("session_lane_switch");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    SessionFixture {
        _tmp: tmp,
        workspace,
        session_dir,
    }
}

/// Runtime whose tool table is assembled exactly like production: the
/// capability-aware `ReadMediaFile` tool for the lane's model.
fn media_runtime(harness: &SwitchHarness, fixture: &SessionFixture) -> AgentRuntime {
    let mut registry = ToolRegistry::new();
    registry.register(
        ReadMediaFileTool::from_model(&harness.model.capabilities, all_inline_transport())
            .expect("media-capable lane registers ReadMediaFile"),
    );
    AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_session_directory(&fixture.session_dir)
            .with_workspace_root(&fixture.workspace)
            .expect("canonical workspace"),
        harness.client(),
        registry,
    )
}

async fn run_turn_ok(runtime: &AgentRuntime, context: &mut AgentContext, input: &str) {
    runtime
        .run_turn(context, AgentMessage::user_text(input))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");
}

fn text_parts(request: &ChatRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .flat_map(|message| match message {
            ChatMessage::System { content }
            | ChatMessage::User { content }
            | ChatMessage::Assistant { content, .. }
            | ChatMessage::ToolResult { content, .. } => content,
        })
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Base64 payloads of every video part in the request copy, in order.
fn video_base64_parts(request: &ChatRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .flat_map(|message| match message {
            ChatMessage::System { content }
            | ChatMessage::User { content }
            | ChatMessage::Assistant { content, .. }
            | ChatMessage::ToolResult { content, .. } => content,
        })
        .filter_map(|part| match part {
            ContentPart::Video {
                data: ImageData::Base64(value),
                ..
            } => Some(value.clone()),
            _ => None,
        })
        .collect()
}

fn base64_encode(bytes: &[u8]) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
}

/// The single `<sha>.bin` blob in the session's blob store; panics when the
/// store does not contain exactly one blob.
fn sole_blob_sha(session_dir: &Path) -> String {
    let blob_dir = session_dir.join("blobs");
    let mut blobs = std::fs::read_dir(&blob_dir)
        .expect("blob dir exists after a media read")
        .map(|entry| {
            entry
                .expect("blob entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    blobs.sort();
    assert_eq!(blobs.len(), 1, "exactly one blob expected, got {blobs:?}");
    blobs[0]
        .strip_suffix(".bin")
        .expect("blob suffix")
        .to_owned()
}

/// Model A reads a video and finishes one follow-up turn; then the lane
/// switches to model B (no video) for one turn. Returns the lane state.
struct SwitchedLane {
    fixture: SessionFixture,
    harness_a: SwitchHarness,
    harness_b: SwitchHarness,
    context: AgentContext,
    blob_sha: String,
}

async fn switched_lane_with_read_and_turns() -> SwitchedLane {
    let fixture = session_with_media_files();
    let harness_a = SwitchHarness::new(
        media_model("model-a", true, true),
        vec![
            tool_call_script("call_1", "ReadMediaFile", &json!({"path": "clip.mp4"})),
            text_script("the cat jumped"),
            text_script("seen it"),
        ],
    );
    let harness_b = SwitchHarness::new(
        media_model("model-b", true, false),
        vec![text_script("you're welcome")],
    );
    let runtime_a = media_runtime(&harness_a, &fixture);
    let runtime_b = media_runtime(&harness_b, &fixture);
    let mut context = AgentContext::new();

    run_turn_ok(&runtime_a, &mut context, "watch clip.mp4").await;
    run_turn_ok(&runtime_a, &mut context, "what happened next").await;
    let blob_sha = sole_blob_sha(&fixture.session_dir);
    run_turn_ok(&runtime_b, &mut context, "thanks").await;

    SwitchedLane {
        fixture,
        harness_a,
        harness_b,
        context,
        blob_sha,
    }
}

#[tokio::test]
async fn switch_to_video_incapable_model_projects_fixed_description_with_full_history() {
    let lane = switched_lane_with_read_and_turns().await;
    let SwitchedLane {
        harness_a,
        harness_b,
        context,
        blob_sha,
        ..
    } = lane;

    let requests_a = harness_a.requests();
    let after_read = &requests_a[1];
    let follow_up = &requests_a[2];

    // A's lane key is stable as history grows.
    let key_a = after_read.options.prompt_cache_key.clone();
    assert!(
        key_a.is_some(),
        "A's session-backed lane must carry a cache key"
    );
    assert_eq!(
        follow_up.options.prompt_cache_key, key_a,
        "A's cache lane must not move as the session history grows"
    );

    // A received the original video bytes inline in the tool result.
    assert_eq!(
        video_base64_parts(after_read),
        vec![base64_encode(MP4_BYTES)],
        "the read exchange must deliver the original video bytes to A"
    );
    assert!(
        after_read.messages.iter().any(
            |message| matches!(message, ChatMessage::ToolResult { content, .. }
                if content.iter().any(|part| matches!(part, ContentPart::Video { .. })))
        ),
        "the video must sit in the tool result message, not a user message"
    );

    // Canonical history keeps a blob ref (never base64) and the blob store
    // holds the exact source bytes.
    let canonical_video_ref = context
        .messages()
        .iter()
        .filter_map(|message| match message {
            AgentMessage::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .flat_map(|content| content.iter())
        .find_map(|part| match part {
            Content::Video {
                data: MediaRef::Blob(sha),
                ..
            } => Some(sha.to_string()),
            _ => None,
        })
        .expect("canonical tool result keeps the blob ref");
    assert_eq!(canonical_video_ref, blob_sha);
    let stored = std::fs::read(
        lane.fixture
            .session_dir
            .join("blobs")
            .join(format!("{blob_sha}.bin")),
    )
    .expect("read stored blob");
    assert_eq!(stored, MP4_BYTES, "blob bytes must match the source file");

    // B's request: the video becomes the fixed description, the follow-up
    // turns and the historical exchange replay verbatim.
    let request_b = &harness_b.requests()[0];
    let description =
        format!("[media not sent: video {blob_sha}; current model does not support video input]");
    let texts = text_parts(request_b);
    assert!(
        texts.contains(&description),
        "B must see the fixed video description, got {texts:?}"
    );
    assert!(
        video_base64_parts(request_b).is_empty(),
        "B must never receive the video bytes"
    );
    for expected in [
        "watch clip.mp4",
        "the cat jumped",
        "what happened next",
        "seen it",
        "thanks",
    ] {
        assert!(
            texts.contains(&expected.to_owned()),
            "B must receive the full history including {expected:?}"
        );
    }
    assert!(
        request_b.messages.iter().any(|message| matches!(
            message,
            ChatMessage::Assistant { tool_calls, .. }
                if tool_calls.iter().any(|call| call.id == "call_1" && call.name == "ReadMediaFile")
        )),
        "B must replay the historical ReadMediaFile exchange"
    );

    // B is a fresh lane: different key from A, anchored to B's identity.
    let key_b = request_b.options.prompt_cache_key.clone();
    assert!(key_b.is_some());
    assert_ne!(key_b, key_a, "model lanes must not share a cache key");
    assert!(
        key_b.as_deref().unwrap().contains("model-b"),
        "B's lane key must carry B's model identity"
    );
}

#[tokio::test]
async fn switch_back_to_video_model_recovers_original_video_and_reuses_lane_key() {
    let fixture = session_with_media_files();
    let harness_a = SwitchHarness::new(
        media_model("model-a", true, true),
        vec![
            tool_call_script("call_1", "ReadMediaFile", &json!({"path": "clip.mp4"})),
            text_script("the cat jumped"),
            text_script("seen it"),
            text_script("the cat slept"),
        ],
    );
    let harness_b = SwitchHarness::new(
        media_model("model-b", true, false),
        vec![text_script("you're welcome"), text_script("b again")],
    );
    let runtime_a = media_runtime(&harness_a, &fixture);
    let runtime_b = media_runtime(&harness_b, &fixture);
    let mut context = AgentContext::new();

    // A reads the video and appends a follow-up turn.
    run_turn_ok(&runtime_a, &mut context, "watch clip.mp4").await;
    run_turn_ok(&runtime_a, &mut context, "what happened next").await;
    let requests_a = harness_a.requests();
    let key_a_before = requests_a[1].options.prompt_cache_key.clone();
    let prefix_before = requests_a[1].messages.clone();

    // B (no video) serves one turn while the video is still active context.
    run_turn_ok(&runtime_b, &mut context, "thanks").await;
    let key_b_before = harness_b.requests()[0].options.prompt_cache_key.clone();
    assert!(
        key_b_before.is_some(),
        "B's session-backed lane must carry a cache key"
    );

    // Back to A: the original video is restored and the B-era messages are a
    // new tail on A's unchanged lane.
    run_turn_ok(&runtime_a, &mut context, "anything else?").await;
    let requests_a = harness_a.requests();
    let back_on_a = &requests_a[3];
    assert_eq!(
        video_base64_parts(back_on_a),
        vec![base64_encode(MP4_BYTES)],
        "A must recover the original video while it is still in active context"
    );
    let texts = text_parts(back_on_a);
    for expected in ["thanks", "you're welcome", "anything else?"] {
        assert!(
            texts.contains(&expected.to_owned()),
            "A must receive the tail produced while B was active, including {expected:?}"
        );
    }
    assert_eq!(
        back_on_a.options.prompt_cache_key, key_a_before,
        "A's lane key must be reused after the B interlude"
    );
    assert_eq!(
        &back_on_a.messages[..prefix_before.len()],
        prefix_before.as_slice(),
        "the pre-switch A prefix must replay byte-identically"
    );

    // B again, now with history it has seen before: the B lane key is stable.
    run_turn_ok(&runtime_b, &mut context, "check again").await;
    let key_b_after = harness_b.requests()[1].options.prompt_cache_key.clone();
    assert_eq!(
        key_b_after, key_b_before,
        "B's lane key must be stable once B has served requests before"
    );
}

#[tokio::test]
async fn in_flight_turn_keeps_old_model_until_next_request() {
    let fixture = session_with_media_files();
    let harness_a = super::fake_harness::DelayedHarness::new(vec![
        super::fake_harness::DelayedStep::Event(AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_a".to_owned(),
        }),
        super::fake_harness::DelayedStep::Delay(Duration::from_millis(400)),
        super::fake_harness::DelayedStep::Event(AiStreamEvent::TextDelta {
            text: "old model answer".to_owned(),
        }),
        super::fake_harness::DelayedStep::Event(AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        }),
    ]);
    let harness_b = FakeHarness::from_turns([super::fake_harness::text_turn_events(
        "msg_b",
        "new model answer",
    )]);
    let runtime_a = AgentRuntime::new(
        AgentConfig::for_model(harness_a.model()).with_session_directory(&fixture.session_dir),
        harness_a.client(),
    );
    let runtime_b = AgentRuntime::new(
        AgentConfig::for_model(harness_b.model()).with_session_directory(&fixture.session_dir),
        harness_b.client(),
    );
    let context = Arc::new(tokio::sync::Mutex::new(AgentContext::new()));

    // The switch: runtime B exists while A's turn is still streaming.
    let turn = {
        let context = Arc::clone(&context);
        tokio::spawn(async move {
            let mut ctx = context.lock().await;
            runtime_a
                .run_turn(
                    &mut ctx,
                    AgentMessage::user_text("question for the old model"),
                )
                .collect::<Vec<_>>()
                .await
        })
    };
    timeout(Duration::from_secs(2), async {
        while harness_a.requests().is_empty() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("A's in-flight request must be captured while the turn is still running");
    assert!(
        harness_b.requests().is_empty(),
        "the model switch must not redirect the in-flight request to B"
    );

    // The in-flight turn completes with the old model's response.
    let events = timeout(Duration::from_secs(2), turn)
        .await
        .expect("in-flight turn completes")
        .expect("turn task joins");
    events
        .into_iter()
        .collect::<Result<Vec<AgentEvent>, _>>()
        .expect("in-flight turn succeeds");
    let mut context_guard = context.lock().await;
    assert!(
        context_guard.messages().iter().any(
            |message| matches!(message, AgentMessage::Assistant { content, .. }
                if content.iter().any(|part| matches!(part, Content::Text { text }
                    if text.as_ref() == "old model answer")))
        ),
        "the in-flight turn must finish with the old model's answer"
    );

    // The next request goes to the new model's client on its own lane.
    run_turn_ok(&runtime_b, &mut context_guard, "next request").await;
    drop(context_guard);
    assert_eq!(
        harness_a.requests().len(),
        1,
        "A's request must never be re-sent or redirected"
    );
    assert_eq!(harness_b.requests().len(), 1);
    assert_ne!(
        harness_a.requests()[0].options.prompt_cache_key,
        harness_b.requests()[0].options.prompt_cache_key,
        "old and new lanes must differ after the switch"
    );
}

/// Lane state after A read a video (plus one follow-up turn), B served one
/// turn, and the next turn crossed the compaction boundary while B was active.
/// `post_compaction_scripts` are the A-lane turns the caller will drive after
/// the boundary.
struct CompactedLane {
    fixture: SessionFixture,
    harness_a: SwitchHarness,
    harness_b_compact: SwitchHarness,
    context: AgentContext,
    key_a_before: String,
    key_b_before: String,
}

async fn compacted_lane(post_compaction_scripts: Vec<Vec<ScriptStep>>) -> CompactedLane {
    let fixture = session_with_media_files();
    let mut scripts_a = vec![
        tool_call_script("call_1", "ReadMediaFile", &json!({"path": "clip.mp4"})),
        text_script("the cat jumped"),
        text_script("seen it"),
    ];
    scripts_a.extend(post_compaction_scripts);
    let harness_a = SwitchHarness::new(media_model("model-a", true, true), scripts_a);
    let harness_b = SwitchHarness::new(
        media_model("model-b", true, false),
        vec![text_script("you're welcome")],
    );
    // The compaction run under B: one summarizer call, then the retried
    // model call on the compacted context.
    let harness_b_compact = SwitchHarness::new(
        media_model("model-b", true, false),
        vec![
            text_script(
                "the user watched a video (clip.mp4) via ReadMediaFile; its bytes are not re-sent",
            ),
            text_script("post-compaction answer"),
        ],
    );
    let runtime_a = media_runtime(&harness_a, &fixture);
    let runtime_b = media_runtime(&harness_b, &fixture);
    let runtime_b_compact = AgentRuntime::new(
        AgentConfig::for_model(harness_b_compact.model())
            .with_session_directory(&fixture.session_dir)
            .with_workspace_root(&fixture.workspace)
            .expect("canonical workspace")
            .with_compaction(CompactionSettings::new(4, 1)),
        harness_b_compact.client(),
    );
    let mut context = AgentContext::new();

    run_turn_ok(&runtime_a, &mut context, "watch clip.mp4").await;
    run_turn_ok(&runtime_a, &mut context, "what happened next").await;
    let key_a_before = harness_a.requests()[1]
        .options
        .prompt_cache_key
        .clone()
        .expect("A lane key");
    run_turn_ok(&runtime_b, &mut context, "thanks").await;
    let key_b_before = harness_b.requests()[0]
        .options
        .prompt_cache_key
        .clone()
        .expect("B lane key");
    run_turn_ok(&runtime_b_compact, &mut context, "please continue").await;

    CompactedLane {
        fixture,
        harness_a,
        harness_b_compact,
        context,
        key_a_before,
        key_b_before,
    }
}

#[tokio::test]
async fn compacted_context_keeps_source_description_without_reinjecting_video() {
    let lane = compacted_lane(vec![text_script("still there")]).await;
    let CompactedLane {
        harness_a,
        harness_b_compact,
        mut context,
        key_a_before,
        key_b_before,
        ..
    } = lane;

    // The summariser request is fed by the real render path
    // (`render_messages_to_text`): the compacted video must appear as the
    // exact `[video: mime_type]` marker, never as raw bytes.
    let summary_request = &harness_b_compact.requests()[0];
    assert!(
        text_parts(summary_request)
            .iter()
            .any(|text| text.contains("  [video: video/mp4]")),
        "the compaction render must mark the video as `[video: mime_type]` text"
    );
    assert!(
        video_base64_parts(summary_request).is_empty(),
        "the compaction render must never send media bytes to the summariser"
    );

    // The retried request on the compacted context keeps the summary
    // description, sends no video, and stays on B's lane.
    let compacted_request_b = &harness_b_compact.requests()[1];
    assert!(
        video_base64_parts(compacted_request_b).is_empty(),
        "the compacted request must not re-inject the original video bytes"
    );
    assert!(
        text_parts(compacted_request_b)
            .iter()
            .any(|text| text.contains("its bytes are not re-sent")),
        "the compaction summary must keep the stable media source description"
    );
    assert_eq!(
        compacted_request_b.options.prompt_cache_key,
        Some(key_b_before),
        "compaction must not move B's cache lane"
    );

    // Switch back to A: the old video is not re-injected; the stable source
    // description stays; A's lane key is unchanged.
    run_turn_ok(
        &media_runtime(&harness_a, &lane.fixture),
        &mut context,
        "still there?",
    )
    .await;
    let back_on_a = &harness_a.requests()[3];
    assert!(
        video_base64_parts(back_on_a).is_empty(),
        "media past the compaction boundary must never be re-injected"
    );
    assert!(
        text_parts(back_on_a)
            .iter()
            .any(|text| text.contains("its bytes are not re-sent")),
        "A must keep the stable media source description after compaction"
    );
    assert_eq!(
        back_on_a.options.prompt_cache_key,
        Some(key_a_before),
        "A's cache lane must survive compaction"
    );
}

#[tokio::test]
async fn fresh_read_after_compaction_appends_new_exchange() {
    let lane = compacted_lane(vec![
        tool_call_script("call_2", "ReadMediaFile", &json!({"path": "second.mp4"})),
        text_script("second clip noted"),
    ])
    .await;
    let CompactedLane {
        fixture,
        harness_a,
        mut context,
        ..
    } = lane;

    // A fresh ReadMediaFile call appends a new exchange whose media is
    // delivered, alongside the retained compaction description.
    run_turn_ok(
        &media_runtime(&harness_a, &fixture),
        &mut context,
        "read the second clip",
    )
    .await;
    let requests_a = harness_a.requests();
    let after_fresh_read = &requests_a[4];
    assert_eq!(
        video_base64_parts(after_fresh_read),
        vec![base64_encode(SECOND_MP4_BYTES)],
        "a fresh read must append the new exchange with the new video bytes"
    );
    assert!(
        text_parts(after_fresh_read)
            .iter()
            .any(|text| text.contains("its bytes are not re-sent")),
        "the retained compaction description must still be present"
    );
    assert!(
        after_fresh_read.messages.iter().any(|message| matches!(
            message,
            ChatMessage::Assistant { tool_calls, .. }
                if tool_calls.iter().any(|call| call.id == "call_2" && call.name == "ReadMediaFile")
        )),
        "the fresh exchange must replay its own tool call"
    );
    let blob_count = std::fs::read_dir(fixture.session_dir.join("blobs"))
        .expect("blob dir exists")
        .count();
    assert_eq!(blob_count, 2, "old and new media blobs both persist");
}

#[test]
fn delegate_child_registry_inherits_read_media_file_from_parent() {
    let mut parent = ToolRegistry::with_builtin_tools_and_todos(Arc::new(Mutex::new(Vec::new())));
    parent.register(
        ReadMediaFileTool::from_model(
            &media_model("parent", true, true).capabilities,
            all_inline_transport(),
        )
        .expect("media-capable parent registers ReadMediaFile"),
    );

    for role in [
        AgentRole::Coder,
        AgentRole::Explorer,
        AgentRole::Reviewer,
        AgentRole::Planner,
    ] {
        let child = parent.filtered_for_agent_role(role);
        assert!(
            child.contains("ReadMediaFile"),
            "{role:?} children share the parent model semantics and must inherit ReadMediaFile"
        );
    }
    assert!(
        parent.for_workflow_child(None).contains("ReadMediaFile"),
        "workflow children must inherit ReadMediaFile too"
    );

    // Capability-aware inheritance: a parent lane without media capability
    // never seeds a media tool into its children.
    let plain = ToolRegistry::with_builtin_tools_and_todos(Arc::new(Mutex::new(Vec::new())));
    assert!(
        !plain
            .filtered_for_agent_role(AgentRole::Coder)
            .contains("ReadMediaFile"),
        "no media capability means no inherited media tool"
    );
}

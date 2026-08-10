//! Request media projection behavior: sendable vs unsendable media, tool
//! exchange attachment, lane keys, and canonical-history invariants.

use neo_ai::{
    ApiKind, ChatMessage, ContentPart, ImageData, MediaTransportCapabilities, MediaTransportMode,
    ModelCapabilities, ModelSpec, ProviderId,
};

use super::*;
use crate::Content;
use crate::MediaRef;

fn dual_media_model(name: &str) -> ModelSpec {
    ModelSpec {
        provider: ProviderId("test".to_owned()),
        model: name.to_owned(),
        api: ApiKind::Local,
        capabilities: ModelCapabilities {
            images: true,
            videos: true,
            ..ModelCapabilities::chat()
        },
    }
}

fn image_only_model(name: &str) -> ModelSpec {
    ModelSpec {
        provider: ProviderId("test".to_owned()),
        model: name.to_owned(),
        api: ApiKind::Local,
        capabilities: ModelCapabilities {
            images: true,
            videos: false,
            ..ModelCapabilities::chat()
        },
    }
}

/// Transport that carries every (kind, position) cell (fake-model shape):
/// tool-result media stays in place.
fn all_inline_transport() -> MediaTransportCapabilities {
    MediaTransportCapabilities {
        user_image: MediaTransportMode::Inline,
        user_video: MediaTransportMode::Inline,
        tool_image: MediaTransportMode::InPlace,
        tool_video: MediaTransportMode::InPlace,
    }
}

/// Transport that carries images only in user messages (OpenAI shape):
/// tool-result media can only be attached after the exchange.
fn user_image_url_transport() -> MediaTransportCapabilities {
    MediaTransportCapabilities {
        user_image: MediaTransportMode::Url,
        ..MediaTransportCapabilities::default()
    }
}

fn image_part(sha: &str) -> Content {
    Content::Image {
        mime_type: "image/png".into(),
        data: MediaRef::Blob(sha.into()),
    }
}

fn video_part(sha: &str) -> Content {
    Content::Video {
        mime_type: "video/mp4".into(),
        data: MediaRef::Blob(sha.into()),
    }
}

fn read_media_file_exchange() -> Vec<AgentMessage> {
    vec![
        AgentMessage::assistant(
            vec![Content::text("reading the file")],
            vec![crate::AgentToolCall {
                id: "call_1".into(),
                name: "ReadMediaFile".into(),
                raw_arguments: r#"{"path":"clip.mp4"}"#.into(),
            }],
            crate::StopReason::ToolUse,
        ),
        AgentMessage::tool_result(
            "call_1",
            "ReadMediaFile",
            vec![
                Content::text("captured the clip"),
                image_part("img-sha"),
                video_part("vid-sha"),
            ],
            false,
        ),
    ]
}

fn follow_up_turns() -> Vec<AgentMessage> {
    vec![
        AgentMessage::user_text("what happened next"),
        AgentMessage::assistant(
            vec![Content::text("the video shows a cat")],
            Vec::new(),
            crate::StopReason::EndTurn,
        ),
        AgentMessage::user_text("thanks"),
    ]
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

fn media_parts(request: &ChatRequest) -> Vec<&ContentPart> {
    request
        .messages
        .iter()
        .flat_map(|message| match message {
            ChatMessage::System { content }
            | ChatMessage::User { content }
            | ChatMessage::Assistant { content, .. }
            | ChatMessage::ToolResult { content, .. } => content,
        })
        .filter(|part| matches!(part, ContentPart::Image { .. } | ContentPart::Video { .. }))
        .collect()
}

/// Create a session directory whose `blobs/` subdirectory contains one image
/// and one video blob; returns the session directory path.
async fn blob_session_with(
    image_sha: &str,
    video_sha: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let session_dir = dir.path().join("session_lane_abc");
    let blob_dir = session_dir.join("blobs");
    tokio::fs::create_dir_all(&blob_dir)
        .await
        .expect("create blob dir");
    tokio::fs::write(blob_dir.join(format!("{image_sha}.bin")), b"image-bytes")
        .await
        .expect("write image blob");
    tokio::fs::write(blob_dir.join(format!("{video_sha}.bin")), b"video-bytes")
        .await
        .expect("write video blob");
    (dir, session_dir)
}

#[tokio::test]
async fn projection_dual_to_no_video_to_dual_keeps_history_and_lane_keys() {
    let (_blob_dir, session_dir) = blob_session_with("img-sha", "vid-sha").await;
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_content(vec![
        Content::text("watch this"),
        video_part("vid-sha"),
    ]));
    for message in read_media_file_exchange() {
        context.append_message(message);
    }
    for message in follow_up_turns() {
        context.append_message(message);
    }

    let canonical_before = context.messages().to_vec();
    let video_bytes_before = tokio::fs::read(session_dir.join("blobs/vid-sha.bin"))
        .await
        .expect("read video blob");

    let config_a =
        AgentConfig::for_model(dual_media_model("model-a")).with_session_directory(&session_dir);
    let config_b =
        AgentConfig::for_model(image_only_model("model-b")).with_session_directory(&session_dir);

    let request_a = chat_request(&config_a, &context, all_inline_transport())
        .await
        .expect("dual request");
    let request_b = chat_request(&config_b, &context, all_inline_transport())
        .await
        .expect("image-only request");

    // A (dual capability): user video and tool-result media are all sendable
    // and resolve to inline base64; nothing is replaced.
    let a_media = media_parts(&request_a);
    assert!(
        matches!(
            a_media.as_slice(),
            [
                ContentPart::Video {
                    data: ImageData::Base64(video_base64),
                    ..
                },
                ContentPart::Image {
                    data: ImageData::Base64(_),
                    ..
                },
                ContentPart::Video {
                    data: ImageData::Base64(_),
                    ..
                }
            ] if video_base64
                == &base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    b"video-bytes"
                )
        ),
        "model A must send the user video and both tool-result media parts"
    );

    // B (no video): the user video and the tool-result video become the same
    // fixed description; the tool-result image stays sendable. Follow-up
    // turns and tool-call ids are preserved verbatim.
    let b_media = media_parts(&request_b);
    assert_eq!(
        b_media.len(),
        1,
        "only the tool-result image stays sendable"
    );
    let video_description =
        "[media not sent: video vid-sha; current model does not support video input]";
    assert_eq!(
        text_parts(&request_b)
            .iter()
            .filter(|text| text.starts_with("[media not sent: video"))
            .collect::<Vec<_>>(),
        vec![video_description, video_description],
        "the same video must produce the identical fixed description in both positions"
    );
    assert_eq!(
        text_parts(&request_b)
            .iter()
            .filter(|text| text.starts_with("[media not sent"))
            .count(),
        2,
        "user video and tool-result video are both described, nothing else"
    );

    // Follow-up turns and the exchange survive both projections unchanged.
    for (name, request) in [("A", &request_a), ("B", &request_b)] {
        let texts = text_parts(request);
        assert!(
            texts.contains(&"what happened next".to_owned()),
            "{name}: follow-up turn must be preserved"
        );
        assert!(
            texts.contains(&"the video shows a cat".to_owned()),
            "{name}: follow-up turn must be preserved"
        );
        assert!(
            texts.contains(&"thanks".to_owned()),
            "{name}: follow-up turn must be preserved"
        );
        assert!(
            texts.contains(&"captured the clip".to_owned()),
            "{name}: tool-result text must be preserved"
        );
        assert!(
            request.messages.iter().any(|message| matches!(
                message,
                ChatMessage::Assistant { tool_calls, .. }
                    if tool_calls.iter().any(|call| call.id == "call_1" && call.name == "ReadMediaFile")
            )),
            "{name}: historical ReadMediaFile call must replay"
        );
    }

    // Lane keys: A and B differ; each lane key is stable as history grows.
    let key_a = request_a.options.prompt_cache_key.clone();
    let key_b = request_b.options.prompt_cache_key.clone();
    assert_ne!(key_a, key_b, "model lanes must not share a cache key");
    assert!(key_a.as_deref().unwrap().contains("model-a"));
    assert!(key_b.as_deref().unwrap().contains("model-b"));

    context.append_message(AgentMessage::user_text("newest request"));
    let request_a2 = chat_request(&config_a, &context, all_inline_transport())
        .await
        .expect("dual request after history append");
    let request_b2 = chat_request(&config_b, &context, all_inline_transport())
        .await
        .expect("image-only request after history append");

    assert_eq!(
        request_a2.options.prompt_cache_key, key_a,
        "lane key must not change when history grows"
    );
    assert_eq!(
        request_b2.options.prompt_cache_key, key_b,
        "lane key must not change when history grows"
    );
    assert_eq!(
        &request_a2.messages[..request_a.messages.len()],
        request_a.messages.as_slice(),
        "model A prefix must be byte-identical after history appends"
    );

    // Canonical messages are append-only: the original prefix is byte-identical
    // after history appends, and the blob bytes are untouched by any build.
    assert_eq!(
        &context.messages()[..canonical_before.len()],
        canonical_before.as_slice()
    );
    assert_eq!(context.messages().len(), canonical_before.len() + 1);
    let video_bytes_after = tokio::fs::read(session_dir.join("blobs/vid-sha.bin"))
        .await
        .expect("read video blob");
    assert_eq!(video_bytes_after, video_bytes_before);
}

#[tokio::test]
async fn projection_attaches_tool_result_media_after_exchange_not_inside() {
    let (_blob_dir, session_dir) = blob_session_with("img-sha", "vid-sha").await;
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("read the image"));
    context.append_message(AgentMessage::assistant(
        vec![Content::text("calling read")],
        vec![crate::AgentToolCall {
            id: "call_1".into(),
            name: "ReadMediaFile".into(),
            raw_arguments: r#"{"path":"shot.png"}"#.into(),
        }],
        crate::StopReason::ToolUse,
    ));
    context.append_message(AgentMessage::tool_result(
        "call_1",
        "ReadMediaFile",
        vec![
            Content::text("captured"),
            image_part("img-sha"),
            video_part("vid-sha"),
        ],
        false,
    ));
    context.append_message(AgentMessage::user_text("describe it"));

    let config = AgentConfig::for_model(image_only_model("model-image-only"))
        .with_session_directory(&session_dir);
    let request = chat_request(&config, &context, user_image_url_transport())
        .await
        .expect("request");

    // Order: assistant call -> tool result (text only) -> attached user media
    // message -> the follow-up user turn. Nothing is inserted between call
    // and result.
    let positions = request
        .messages
        .iter()
        .map(|message| match message {
            ChatMessage::Assistant { tool_calls, .. } if !tool_calls.is_empty() => "assistant-call",
            ChatMessage::ToolResult { .. } => "tool-result",
            ChatMessage::User { content }
                if content
                    .iter()
                    .any(|part| matches!(part, ContentPart::Image { .. })) =>
            {
                "user-media"
            }
            ChatMessage::User { .. } => "user",
            _ => "other",
        })
        .collect::<Vec<_>>();
    assert_eq!(
        positions,
        [
            "user",
            "assistant-call",
            "tool-result",
            "user-media",
            "user"
        ],
        "tool-result media must attach after the complete exchange, never inside it"
    );

    let messages = request.messages.clone();
    let ChatMessage::ToolResult { content, .. } = &messages[2] else {
        panic!("expected tool result");
    };
    assert!(
        content
            .iter()
            .all(|part| !matches!(part, ContentPart::Image { .. } | ContentPart::Video { .. })),
        "tool result must keep only text; media moves to the attached user message"
    );
    assert!(
        content
            .iter()
            .any(|part| matches!(part, ContentPart::Text { text } if text == "captured")),
        "tool-result text is preserved"
    );

    let ChatMessage::User { content } = &messages[3] else {
        panic!("expected attached user media message");
    };
    assert_eq!(
        content
            .iter()
            .filter(|part| matches!(part, ContentPart::Image { .. }))
            .count(),
        1,
        "the attached user message carries the tool-result image"
    );

    // The video was rejected by the model and stays inside the result as a
    // fixed description — never silently dropped, never attached.
    let ChatMessage::ToolResult { content, .. } = &messages[2] else {
        unreachable!();
    };
    assert!(
        content.iter().any(|part| matches!(
            part,
            ContentPart::Text { text } if text == "[media not sent: video vid-sha; current model does not support video input]"
        ))
    );
}

#[tokio::test]
async fn projection_description_is_digest_anchored_and_identical_across_requests() {
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_content(vec![
        video_part("vid-sha"),
        Content::Image {
            mime_type: "image/png".into(),
            data: MediaRef::Base64("aGVsbG8=".into()),
        },
        Content::Image {
            mime_type: "image/png".into(),
            data: MediaRef::Url("https://example.test/cat.png".into()),
        },
    ]));

    let config = AgentConfig::for_model(dual_media_model("model-description"))
        .with_session_directory("/tmp/neo/session_lane_desc");
    // All-unsupported transport: every user-cell media kind is described.
    let transport = MediaTransportCapabilities::default();

    let first = chat_request(&config, &context, transport)
        .await
        .expect("first request");
    context.append_message(AgentMessage::user_text("extra history"));
    let second = chat_request(&config, &context, transport)
        .await
        .expect("second request");

    let described = |request: &ChatRequest| {
        text_parts(request)
            .into_iter()
            .filter(|text| text.starts_with("[media not sent"))
            .collect::<Vec<_>>()
    };
    let first_descriptions = described(&first);
    let second_descriptions = described(&second);
    assert_eq!(first_descriptions, second_descriptions);

    // The digest is the blob SHA-256 for blob refs and the SHA-256 of the
    // encoded value for base64/URL refs, so descriptions are path-independent,
    // deterministic per lane, and never require reading a blob.
    let digest_of = |value: &[u8]| {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(value);
        format!("{:x}", hasher.finalize())
    };
    assert_eq!(
        first_descriptions,
        vec![
            format!(
                "[media not sent: video vid-sha; provider cannot transport video in user messages]"
            ),
            format!(
                "[media not sent: image {}; provider cannot transport image in user messages]",
                digest_of(b"aGVsbG8=")
            ),
            format!(
                "[media not sent: image {}; provider cannot transport image in user messages]",
                digest_of(b"https://example.test/cat.png")
            ),
        ]
    );
}

#[tokio::test]
async fn chat_request_missing_sendable_blob_becomes_unavailable_text() {
    let config = AgentConfig::for_model(dual_media_model("model-missing-blob"))
        .with_session_directory("/tmp/neo/session_lane_missing");
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_content(vec![video_part("deadbeef")]));

    let request = chat_request(&config, &context, all_inline_transport())
        .await
        .expect("request");

    assert!(
        text_parts(&request)
            .iter()
            .any(|text| text == "[unavailable video: blob deadbeef]"),
        "a sendable media part with a missing blob must become deterministic unavailable text, never an empty encoding"
    );
}

#[tokio::test]
async fn chat_request_oversized_video_blob_fails_before_provider_request() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session_dir = dir.path().join("session_lane_big");
    let blob_dir = session_dir.join("blobs");
    tokio::fs::create_dir_all(&blob_dir)
        .await
        .expect("create blob dir");
    let oversized = vec![0u8; crate::runtime::image_blobs::MAX_INLINE_VIDEO_BYTES + 1];
    tokio::fs::write(blob_dir.join("big.bin"), &oversized)
        .await
        .expect("write oversized video blob");

    let config = AgentConfig::for_model(dual_media_model("model-big-video"))
        .with_session_directory(&session_dir);
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_content(vec![video_part("big")]));

    let error = chat_request(&config, &context, all_inline_transport())
        .await
        .expect_err("over-limit video must fail before any provider request");
    let neo_ai::AiError::Configuration { message } = error else {
        panic!("expected configuration error, got {error:?}");
    };
    assert!(message.contains("exceeds the inline size limit"));
    assert!(message.contains("big"));
    assert_eq!(
        context.messages().len(),
        1,
        "canonical context is untouched by the failed request"
    );
}

#[test]
fn projection_plan_honors_declared_attach_after_result_transport() {
    let transport = MediaTransportCapabilities {
        user_image: MediaTransportMode::Url,
        tool_image: MediaTransportMode::AttachAfterResult,
        ..MediaTransportCapabilities::default()
    };
    let model = image_only_model("model-declared-attach").capabilities;
    let plan = MediaProjectionPlan::compute(&model, transport, ExchangeProjection::Preserve);
    assert_eq!(
        plan.tool_image.mode,
        MediaProjectionMode::AttachAfterExchange,
        "a declared attach-after-result cell must project tool media to the appended user message"
    );
}

#[tokio::test]
async fn projection_all_media_tool_result_keeps_stable_marker_when_attached() {
    let (_blob_dir, session_dir) = blob_session_with("img-sha", "vid-sha").await;
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::assistant(
        vec![Content::text("calling read")],
        vec![crate::AgentToolCall {
            id: "call_1".into(),
            name: "ReadMediaFile".into(),
            raw_arguments: r#"{"path":"shot.png"}"#.into(),
        }],
        crate::StopReason::ToolUse,
    ));
    // The result has no text at all: every part is media.
    context.append_message(AgentMessage::tool_result(
        "call_1",
        "ReadMediaFile",
        vec![image_part("img-sha")],
        false,
    ));
    let config = AgentConfig::for_model(image_only_model("model-empty-result"))
        .with_session_directory(&session_dir);

    let request = chat_request(&config, &context, user_image_url_transport())
        .await
        .expect("request");

    // The tool result keeps a stable, digest-anchored marker instead of
    // empty content; the media itself is attached right after the exchange.
    let messages = request.messages.clone();
    let ChatMessage::ToolResult { content, .. } = &messages[1] else {
        panic!("expected tool result");
    };
    assert_eq!(
        content,
        &vec![ContentPart::Text {
            text:
                "[media attached after this tool exchange: image img-sha; sent as a user message]"
                    .to_owned()
        }],
        "an all-media result must keep a marker, never empty content"
    );
    let ChatMessage::User { content } = &messages[2] else {
        panic!("expected attached user media message");
    };
    assert!(
        content
            .iter()
            .any(|part| matches!(part, ContentPart::Image { .. })),
        "the attached user message carries the media"
    );
}

#[test]
fn lane_cache_key_never_aliases_colliding_provider_and_model_ids() {
    let transport = MediaTransportCapabilities::default();
    let capabilities = image_only_model("model-collision").capabilities;
    let session = "/tmp/neo/session_lane_collision";

    let left = AgentConfig::for_model(ModelSpec {
        provider: ProviderId("a|model=x".to_owned()),
        model: "y".to_owned(),
        api: ApiKind::Local,
        capabilities: capabilities.clone(),
    })
    .with_session_directory(session);
    let right = AgentConfig::for_model(ModelSpec {
        provider: ProviderId("a".to_owned()),
        model: "x|y".to_owned(),
        api: ApiKind::Local,
        capabilities: capabilities.clone(),
    })
    .with_session_directory(session);

    let plan = MediaProjectionPlan::compute(&capabilities, transport, ExchangeProjection::Preserve);
    let left_key = lane_cache_key(&left, &plan, transport, Some("session_lane_collision"));
    let right_key = lane_cache_key(&right, &plan, transport, Some("session_lane_collision"));

    assert!(left_key.is_some());
    assert_ne!(
        left_key, right_key,
        "provider/model ids that collide under plain `|` encoding must never alias two lanes"
    );
}

#[test]
fn projection_shape_derives_exchange_decision() {
    let model = image_only_model("model-exchange").capabilities;
    let transport = user_image_url_transport();
    let preserved = projection_shape(
        &MediaProjectionPlan::compute(&model, transport, ExchangeProjection::Preserve),
        transport,
        ApiKind::OpenAi,
    );
    let converted = projection_shape(
        &MediaProjectionPlan::compute(&model, transport, ExchangeProjection::ConvertToText),
        transport,
        ApiKind::OpenAi,
    );

    assert!(
        preserved.ends_with("exchange=preserve"),
        "preserve decision must derive its own lane-shape label: {preserved}"
    );
    assert!(
        converted.ends_with("exchange=convert"),
        "convert decision must derive its own lane-shape label: {converted}"
    );
    assert_ne!(
        preserved, converted,
        "different exchange projections must never share a cache lane shape"
    );
}

/// `tool_result_kind_deliverable` must never drift from the projection's
/// `tool_cell` decision: the tool table is only exposed for kinds the
/// projection would actually deliver (in place or attached after the
/// exchange). Exhaustive over every capability cell — kind × model
/// acceptance × tool-result transport mode × user-message transport mode.
#[test]
fn tool_result_deliverable_matches_tool_cell_for_all_capability_cells() {
    use neo_ai::MediaKind;

    let modes = [
        MediaTransportMode::Inline,
        MediaTransportMode::Url,
        MediaTransportMode::FileRef,
        MediaTransportMode::InPlace,
        MediaTransportMode::AttachAfterResult,
        MediaTransportMode::Unsupported,
    ];
    for kind in [MediaKind::Image, MediaKind::Video] {
        for model_accepts in [false, true] {
            let model = ModelCapabilities {
                images: kind == MediaKind::Image && model_accepts,
                videos: kind == MediaKind::Video && model_accepts,
                ..ModelCapabilities::chat()
            };
            for &tool_mode in &modes {
                for &user_mode in &modes {
                    let transport = MediaTransportCapabilities {
                        user_image: user_mode,
                        user_video: user_mode,
                        tool_image: tool_mode,
                        tool_video: tool_mode,
                    };
                    let cell = MediaProjectionCell::tool_cell(kind, &model, transport);
                    let deliverable = tool_result_kind_deliverable(kind, &model, transport);
                    assert_eq!(
                        cell.mode == MediaProjectionMode::FixedDescription,
                        !deliverable,
                        "deliverable must equal non-fixed-description for \
                         kind={kind:?}, model_accepts={model_accepts}, \
                         tool_mode={tool_mode:?}, user_mode={user_mode:?}"
                    );
                }
            }
        }
    }
}

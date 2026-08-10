use super::*;
use crate::ToolAccess;
use crate::ToolContext;
use crate::runtime::image_blobs::MAX_INLINE_VIDEO_BYTES;
use neo_ai::{MediaTransportCapabilities, MediaTransportMode, ModelCapabilities};
use serde_json::json;
use sha2::Sha256;

const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDRfiller-bytes";
const MP4_BYTES: &[u8] = b"\x00\x00\x00\x18ftypmp42filler-bytes-for-mp4";

fn model_with(images: bool, videos: bool) -> ModelCapabilities {
    ModelCapabilities {
        images,
        videos,
        ..ModelCapabilities::chat()
    }
}

fn all_sendable_transport() -> MediaTransportCapabilities {
    MediaTransportCapabilities {
        user_image: MediaTransportMode::Inline,
        user_video: MediaTransportMode::Inline,
        tool_image: MediaTransportMode::InPlace,
        tool_video: MediaTransportMode::InPlace,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn blob_path(session_dir: &std::path::Path, sha: &str) -> std::path::PathBuf {
    session_dir.join("blobs").join(format!("{sha}.bin"))
}

fn assert_no_blob_records(session_dir: &std::path::Path) {
    let blob_dir = session_dir.join("blobs");
    if !blob_dir.exists() {
        return;
    }
    let entries: Vec<_> = std::fs::read_dir(&blob_dir)
        .expect("read blob dir")
        .collect();
    assert!(
        entries.is_empty(),
        "no media record may be written on a failed read: {:?}",
        entries
    );
}

fn media_ctx(workspace: &std::path::Path, session_dir: Option<&std::path::Path>) -> ToolContext {
    let mut ctx = ToolContext::new(workspace).expect("tool context");
    ctx.session_directory = session_dir.map(|path| path.to_path_buf());
    ctx.access = ToolAccess {
        file_read: true,
        ..ToolAccess::none()
    };
    ctx
}

#[test]
fn four_capability_combinations_shape_tool_exposure() {
    let transport = all_sendable_transport();
    let dual = ReadMediaFileTool::from_model(&model_with(true, true), transport)
        .expect("dual media must expose the tool");
    assert!(dual.images && dual.videos);

    let images_only =
        ReadMediaFileTool::from_model(&model_with(true, false), transport).expect("images tool");
    assert!(images_only.images && !images_only.videos);

    let videos_only =
        ReadMediaFileTool::from_model(&model_with(false, true), transport).expect("videos tool");
    assert!(!videos_only.images && videos_only.videos);

    assert!(
        ReadMediaFileTool::from_model(&model_with(false, false), transport).is_none(),
        "no media capability must not expose the tool"
    );
}

#[test]
fn tool_exposure_omits_kind_without_any_transport_path() {
    // Anthropic-like transport: images inline in user messages only, no
    // video anywhere. Tool-result images are still deliverable (attached
    // after the exchange), videos are not.
    let transport = MediaTransportCapabilities {
        user_image: MediaTransportMode::Inline,
        ..MediaTransportCapabilities::default()
    };
    let tool = ReadMediaFileTool::from_model(&model_with(true, true), transport)
        .expect("images deliverable via user message");
    assert!(
        tool.images && !tool.videos,
        "a kind with no transport path at any position must not be exposed"
    );

    // Fully unsupported transport: nothing deliverable, no tool.
    assert!(
        ReadMediaFileTool::from_model(
            &model_with(true, true),
            MediaTransportCapabilities::default()
        )
        .is_none(),
        "model acceptance alone is never a transport guarantee"
    );
}

#[test]
fn dual_media_tool_table_lists_image_and_video_with_image_params() {
    let tool = ReadMediaFileTool::new(true, true);
    let mut registry = crate::ToolRegistry::new();
    registry.register(tool);
    let specs = registry.specs();
    assert_eq!(specs.len(), 1);
    let spec = &specs[0];
    assert_eq!(spec.name, "ReadMediaFile");
    assert!(
        spec.description.contains("image") && spec.description.contains("video"),
        "dual-media description must state both kinds"
    );
    let properties = spec.input_schema["properties"]
        .as_object()
        .expect("properties object");
    assert!(
        properties.contains_key("path")
            && properties.contains_key("region")
            && properties.contains_key("resolution"),
        "dual-media schema must expose path and image parameters: {properties:?}"
    );
}

#[test]
fn images_only_tool_table_avoids_video_claims() {
    let tool = ReadMediaFileTool::new(true, false);
    let spec = tool.spec();
    assert_eq!(spec.name, "ReadMediaFile");
    assert!(
        !spec.description.contains("video") && !spec.description.contains("Video"),
        "images-only description must not claim video support: {}",
        spec.description
    );
    let properties = spec.input_schema["properties"]
        .as_object()
        .expect("properties object");
    assert!(
        properties.contains_key("region") && properties.contains_key("resolution"),
        "images-only schema must keep image parameters: {properties:?}"
    );
}

#[test]
fn videos_only_tool_table_exposes_path_only() {
    let tool = ReadMediaFileTool::new(false, true);
    let spec = tool.spec();
    assert_eq!(spec.name, "ReadMediaFile");
    assert!(spec.description.contains("video"));
    let properties = spec.input_schema["properties"]
        .as_object()
        .expect("properties object");
    assert_eq!(
        properties.keys().collect::<Vec<_>>(),
        vec!["path"],
        "videos-only schema must not expose image-specific parameters"
    );
}

#[tokio::test]
async fn read_media_image_persists_blob_and_returns_structured_media() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("shot.png"), PNG_BYTES).expect("write png");
    let session_dir = temp.path().join("session");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    let ctx = media_ctx(&workspace, Some(&session_dir));

    let result = ReadMediaFileTool::new(true, true)
        .execute(&ctx, json!({"path": "shot.png"}))
        .await
        .expect("execute");

    assert!(!result.is_error);
    assert!(!result.content.is_empty(), "text summary is mandatory");
    let sha = sha256_hex(PNG_BYTES);
    assert_eq!(
        result.media,
        vec![crate::Content::Image {
            mime_type: "image/png".into(),
            data: crate::MediaRef::Blob(sha.clone().into()),
        }]
    );
    let stored = std::fs::read(blob_path(&session_dir, &sha)).expect("blob file");
    assert_eq!(stored, PNG_BYTES, "blob bytes must match the source file");
}

#[tokio::test]
async fn read_media_video_persists_blob_and_returns_structured_media() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("clip.mp4"), MP4_BYTES).expect("write mp4");
    let session_dir = temp.path().join("session");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    let ctx = media_ctx(&workspace, Some(&session_dir));

    // Videos-only capability still reads videos.
    let result = ReadMediaFileTool::new(false, true)
        .execute(&ctx, json!({"path": "clip.mp4"}))
        .await
        .expect("execute");

    assert!(!result.is_error);
    let sha = sha256_hex(MP4_BYTES);
    assert_eq!(
        result.media,
        vec![crate::Content::Video {
            mime_type: "video/mp4".into(),
            data: crate::MediaRef::Blob(sha.clone().into()),
        }]
    );
    let stored = std::fs::read(blob_path(&session_dir, &sha)).expect("blob file");
    assert_eq!(stored, MP4_BYTES);
}

#[tokio::test]
async fn read_media_image_accepts_region_and_resolution_hints() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("shot.png"), PNG_BYTES).expect("write png");
    let session_dir = temp.path().join("session");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    let ctx = media_ctx(&workspace, Some(&session_dir));

    let result = ReadMediaFileTool::new(true, true)
        .execute(
            &ctx,
            json!({
                "path": "shot.png",
                "region": {"x": 0, "y": 0, "width": 10, "height": 10},
                "resolution": "high"
            }),
        )
        .await
        .expect("execute");

    assert!(!result.is_error);
    assert_eq!(result.media.len(), 1);
    assert!(result.content.contains("no transformation was performed"));
}

#[tokio::test]
async fn read_media_over_limit_video_rejected_without_blob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    let session_dir = temp.path().join("session");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    let ctx = media_ctx(&workspace, Some(&session_dir));

    // Sparse file: valid MP4 signature, metadata length just over the limit.
    let big = workspace.join("big.mp4");
    let file = std::fs::File::create(&big).expect("create sparse");
    file.set_len(MAX_INLINE_VIDEO_BYTES as u64 + 1)
        .expect("set length");
    use std::io::Write;
    let mut writer = std::fs::OpenOptions::new()
        .write(true)
        .open(&big)
        .expect("reopen");
    writer
        .write_all(b"\x00\x00\x00\x18ftypmp42")
        .expect("header");

    let result = ReadMediaFileTool::new(true, true)
        .execute(&ctx, json!({"path": "big.mp4"}))
        .await
        .expect("execute");

    assert!(result.is_error);
    assert!(result.content.contains("size limit"));
    assert_no_blob_records(&session_dir);
}

#[tokio::test]
async fn read_media_unknown_type_rejected_without_blob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("notes.txt"), b"plain text").expect("write text");
    let session_dir = temp.path().join("session");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    let ctx = media_ctx(&workspace, Some(&session_dir));

    let result = ReadMediaFileTool::new(true, true)
        .execute(&ctx, json!({"path": "notes.txt"}))
        .await
        .expect("execute");

    assert!(result.is_error);
    assert!(result.content.contains("not a supported media file"));
    assert_no_blob_records(&session_dir);
}

#[tokio::test]
async fn read_media_rejects_ftyp_without_known_major_brand() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    // `ftyp` alone is not a media signature: an unknown major brand must be
    // rejected, so arbitrary files cannot masquerade as MP4.
    std::fs::write(
        workspace.join("fake.mp4"),
        b"\x00\x00\x00\x18ftypzzzzfiller-bytes",
    )
    .expect("write fake mp4");
    let session_dir = temp.path().join("session");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    let ctx = media_ctx(&workspace, Some(&session_dir));

    let result = ReadMediaFileTool::new(true, true)
        .execute(&ctx, json!({"path": "fake.mp4"}))
        .await
        .expect("execute");

    assert!(result.is_error);
    assert!(result.content.contains("not a supported media file"));
    assert_no_blob_records(&session_dir);
}

#[tokio::test]
async fn read_media_missing_file_rejected_without_blob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    let session_dir = temp.path().join("session");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    let ctx = media_ctx(&workspace, Some(&session_dir));

    let result = ReadMediaFileTool::new(true, true)
        .execute(&ctx, json!({"path": "missing.png"}))
        .await
        .expect("execute");

    assert!(result.is_error);
    assert!(result.content.contains("does not exist"));
    assert_no_blob_records(&session_dir);
}

#[tokio::test]
async fn read_media_directory_path_rejected_without_blob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    let session_dir = temp.path().join("session");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    let ctx = media_ctx(&workspace, Some(&session_dir));

    // A path that resolves to a directory (`..` escapes the workspace and
    // lands on the temp root) is refused as "not a file" with no blob record.
    let result = ReadMediaFileTool::new(true, true)
        .execute(&ctx, json!({"path": ".."}))
        .await
        .expect("execute");

    assert!(result.is_error);
    assert!(result.content.contains("not a file"));
    assert_no_blob_records(&session_dir);
}

#[tokio::test]
async fn read_media_image_rejected_when_video_only_capability() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("shot.png"), PNG_BYTES).expect("write png");
    let session_dir = temp.path().join("session");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    let ctx = media_ctx(&workspace, Some(&session_dir));

    let result = ReadMediaFileTool::new(false, true)
        .execute(&ctx, json!({"path": "shot.png"}))
        .await
        .expect("execute");

    assert!(result.is_error);
    assert!(result.content.contains("does not support image input"));
    assert_no_blob_records(&session_dir);
}

#[tokio::test]
async fn read_media_video_rejected_when_image_only_capability() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("clip.mp4"), MP4_BYTES).expect("write mp4");
    let session_dir = temp.path().join("session");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    let ctx = media_ctx(&workspace, Some(&session_dir));

    let result = ReadMediaFileTool::new(true, false)
        .execute(&ctx, json!({"path": "clip.mp4"}))
        .await
        .expect("execute");

    assert!(result.is_error);
    assert!(result.content.contains("does not support video input"));
    assert_no_blob_records(&session_dir);
}

#[tokio::test]
async fn read_media_video_rejects_image_specific_parameters() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("clip.mp4"), MP4_BYTES).expect("write mp4");
    let session_dir = temp.path().join("session");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    let ctx = media_ctx(&workspace, Some(&session_dir));

    let result = ReadMediaFileTool::new(true, true)
        .execute(
            &ctx,
            json!({
                "path": "clip.mp4",
                "region": {"x": 0, "y": 0, "width": 10, "height": 10}
            }),
        )
        .await
        .expect("execute");

    assert!(result.is_error);
    assert!(result.content.contains("cannot be applied to a video file"));
    assert_no_blob_records(&session_dir);
}

#[tokio::test]
async fn read_media_without_session_directory_fails_closed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("shot.png"), PNG_BYTES).expect("write png");
    let ctx = media_ctx(&workspace, None);

    let result = ReadMediaFileTool::new(true, true)
        .execute(&ctx, json!({"path": "shot.png"}))
        .await
        .expect("execute");

    assert!(result.is_error);
    assert!(result.content.contains("session directory is unavailable"));
    assert!(result.media.is_empty(), "no media without a persisted blob");
}

#[tokio::test]
async fn read_media_denied_without_file_read_permission() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("shot.png"), PNG_BYTES).expect("write png");
    let mut ctx = media_ctx(&workspace, None);
    ctx.access = ToolAccess::none();

    let error = ReadMediaFileTool::new(true, true)
        .execute(&ctx, json!({"path": "shot.png"}))
        .await
        .expect_err("must fail closed without file_read permission");
    assert!(matches!(error, crate::ToolError::PermissionDenied { .. }));
}

#[tokio::test]
async fn read_media_rejects_unknown_input_fields() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    let ctx = media_ctx(&workspace, None);

    let error = ReadMediaFileTool::new(true, true)
        .execute(&ctx, json!({"path": "shot.png", "bogus": 1}))
        .await
        .expect_err("unknown fields must be rejected at parse time");
    assert!(matches!(error, crate::ToolError::InvalidInput { .. }));
}

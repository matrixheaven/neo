use super::*;
use crate::ToolAccess;
use crate::ToolContext;
use crate::WorkspaceAccessPolicy;
use crate::WorkspaceAccessRoot;
use crate::WorkspaceAccessRootKind;
use serde_json::json;

#[tokio::test]
async fn read_tool_allows_absolute_paths_outside_workspace() {
    use super::{ReadTool, Tool};
    use serde_json::json;

    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    let external_dir = temp.path().join("external");
    std::fs::create_dir_all(&external_dir).expect("external dir");
    let external_file = external_dir.join("note.md");
    std::fs::write(&external_file, "external content\n").expect("write external");

    let ctx = crate::ToolContext::new(&workspace)
        .expect("tool context")
        .with_access(crate::ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: false,
            user_question: false,
        });

    let tool = ReadTool;
    let input = json!({
        "path": external_file.to_str().unwrap(),
    });
    let result = tool.execute(&ctx, input).await.expect("outside allow");
    assert!(!result.is_error);
    assert!(result.content.contains("external content"));
}

#[tokio::test]
async fn read_tool_resolves_relative_paths_against_workspace() {
    use super::{ReadTool, Tool};
    use serde_json::json;

    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("note.md"), "workspace content\n").expect("write note");

    let ctx = crate::ToolContext::new(&workspace)
        .expect("tool context")
        .with_access(crate::ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: false,
            user_question: false,
        });

    let tool = ReadTool;
    let input = json!({"path": "note.md"});
    let result = tool.execute(&ctx, input).await.expect("execute");
    assert!(!result.is_error);
    assert!(result.content.contains("workspace content"));
}

#[tokio::test]
async fn read_tool_rejects_missing_file() {
    use super::{ReadTool, Tool};
    use serde_json::json;

    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");

    let ctx = crate::ToolContext::new(&workspace)
        .expect("tool context")
        .with_access(crate::ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: false,
            user_question: false,
        });

    let tool = ReadTool;
    let input = json!({"path": "missing.txt"});
    let result = tool.execute(&ctx, input).await.expect("execute");
    assert!(result.is_error);
    assert!(result.content.contains("does not exist"));
}

#[tokio::test]
async fn read_tool_rejects_directories() {
    use super::{ReadTool, Tool};
    use serde_json::json;

    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::create_dir_all(workspace.join("src")).expect("src dir");

    let ctx = crate::ToolContext::new(&workspace)
        .expect("tool context")
        .with_access(crate::ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: false,
            user_question: false,
        });

    let tool = ReadTool;
    let input = json!({"path": "src"});
    let result = tool.execute(&ctx, input).await.expect("execute");
    assert!(result.is_error);
    assert!(result.content.contains("is not a file"));
}

#[tokio::test]
async fn read_tool_rejects_sensitive_files() {
    use super::{ReadTool, Tool};
    use serde_json::json;

    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join(".env"), "SECRET=value\n").expect("write env");
    std::fs::write(workspace.join("key.pem"), "secret key\n").expect("write pem");

    let ctx = crate::ToolContext::new(&workspace)
        .expect("tool context")
        .with_access(crate::ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: false,
            user_question: false,
        });

    let tool = ReadTool;

    let dot_env = tool
        .execute(&ctx, json!({"path": ".env"}))
        .await
        .expect("execute");
    assert!(dot_env.is_error);
    assert!(dot_env.content.contains("sensitive-file pattern"));

    let pem = tool
        .execute(&ctx, json!({"path": "key.pem"}))
        .await
        .expect("execute");
    assert!(pem.is_error);
    assert!(pem.content.contains("sensitive-file pattern"));
}

#[tokio::test]
async fn read_tool_transcodes_utf16_text() {
    use super::{ReadTool, Tool};

    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");

    let text = "first line\r\n第二行\r\n";
    let mut utf16_le_bom = vec![0xff, 0xfe];
    let mut utf16_le = Vec::new();
    let mut utf16_be_bom = vec![0xfe, 0xff];
    for unit in text.encode_utf16() {
        utf16_le_bom.extend_from_slice(&unit.to_le_bytes());
        utf16_le.extend_from_slice(&unit.to_le_bytes());
        utf16_be_bom.extend_from_slice(&unit.to_be_bytes());
    }

    let ctx = crate::ToolContext::new(&workspace)
        .expect("tool context")
        .with_access(crate::ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: false,
            user_question: false,
        });
    let tool = ReadTool;

    for (name, bytes) in [
        ("utf16le-bom.txt", utf16_le_bom),
        ("utf16le.txt", utf16_le),
        ("utf16be-bom.txt", utf16_be_bom),
    ] {
        std::fs::write(workspace.join(name), bytes).expect("write UTF-16 text");
        let result = tool
            .execute(&ctx, json!({ "path": name }))
            .await
            .expect("execute");

        assert!(
            !result.is_error,
            "unexpected read error: {}",
            result.content
        );
        assert!(
            result.content.contains("1\tfirst line"),
            "{}",
            result.content
        );
        assert!(result.content.contains("2\t第二行"), "{}", result.content);

        let tail = tool
            .execute(
                &ctx,
                json!({ "path": name, "line_offset": -1, "n_lines": 1 }),
            )
            .await
            .expect("read tail");
        assert!(!tail.is_error, "unexpected tail error: {}", tail.content);
        assert!(tail.content.contains("2\t第二行"), "{}", tail.content);
        assert!(!tail.content.contains("1\tfirst line"), "{}", tail.content);
    }
}

#[tokio::test]
async fn read_tool_rejects_nul_bytes() {
    use super::{ReadTool, Tool};
    use serde_json::json;

    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    let mut bytes = b"text\n".to_vec();
    bytes.push(0);
    bytes.extend_from_slice(b"tail\n");
    std::fs::write(workspace.join("blob.bin"), &bytes).expect("write binary");

    let ctx = crate::ToolContext::new(&workspace)
        .expect("tool context")
        .with_access(crate::ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: false,
            user_question: false,
        });

    let tool = ReadTool;
    let input = json!({"path": "blob.bin"});
    let result = tool.execute(&ctx, input).await.expect("execute");
    assert!(result.is_error);
    assert!(result.content.contains("not readable as text"));
}

#[tokio::test]
async fn read_allows_added_read_root() {
    let primary = tempfile::tempdir().expect("primary");
    let added = tempfile::tempdir().expect("added");
    let file = added.path().join("lib.rs");
    std::fs::write(&file, "pub fn lib() {}\n").expect("write");
    let policy = WorkspaceAccessPolicy::with_roots(
        primary.path(),
        [WorkspaceAccessRoot {
            path: added.path().canonicalize().expect("canonical added"),
            kind: WorkspaceAccessRootKind::Added,
            read: true,
            write: false,
        }],
    )
    .expect("policy");
    let ctx = ToolContext::new(primary.path())
        .expect("context")
        .with_workspace_policy(policy)
        .with_access(ToolAccess::all());

    let result = ReadTool
        .execute(&ctx, json!({ "path": file }))
        .await
        .expect("read");

    assert!(
        !result.is_error,
        "unexpected read error: {}",
        result.content
    );
    assert!(result.content.contains("pub fn lib()"));
}

#[tokio::test]
async fn read_denies_path_outside_all_roots() {
    let primary = tempfile::tempdir().expect("primary");
    let outside = tempfile::tempdir().expect("outside");
    let file = outside.path().join("secret.txt");
    std::fs::write(&file, "secret\n").expect("write");
    let ctx = ToolContext::new(primary.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = ReadTool
        .execute(&ctx, json!({ "path": file }))
        .await
        .expect("outside allow");
    assert!(!result.is_error);
    assert!(result.content.contains("secret"));
}

use neo_sdk::{ExportConversation, ExportMessage, HtmlExportOptions, export_html};
use serde_json::json;

#[test]
fn export_html_escapes_content_and_renders_markdown() {
    let conversation = ExportConversation::new(
        "Unsafe <Session>",
        vec![
            ExportMessage::new("user", "Hello <script>alert(1)</script>"),
            ExportMessage::new("assistant", "Use **bold** and `code`."),
        ],
    );

    let html = export_html(&conversation, &HtmlExportOptions::default()).unwrap();

    assert!(html.contains("<title>Unsafe &lt;Session&gt;</title>"));
    assert!(html.contains("Hello &lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(html.contains("<strong>bold</strong>"));
    assert!(html.contains("<code>code</code>"));
    assert!(!html.contains("<script>alert(1)</script>"));
}

#[test]
fn export_html_rejects_unsafe_role_class_names() {
    let conversation = ExportConversation::new(
        "role",
        vec![ExportMessage::new("assistant onclick=alert(1)", "bad")],
    );

    let err = export_html(&conversation, &HtmlExportOptions::default()).unwrap_err();

    assert!(err.to_string().contains("message role"));
}

#[test]
fn export_html_sanitizes_markdown_link_urls() {
    let conversation = ExportConversation::new(
        "links",
        vec![ExportMessage::new(
            "assistant",
            "[safe](https://example.test/?q=<tag>) [unsafe](java\u{0}script:alert(1)) [breakout](https://example.test/\" onclick=\"alert(1))",
        )],
    );

    let html = export_html(&conversation, &HtmlExportOptions::default()).unwrap();

    assert!(html.contains("<a href=\"https://example.test/"));
    assert!(html.contains(">safe</a>"));
    assert!(html.contains("unsafe"));
    assert!(!html.contains("java"));
    assert!(!html.contains("script:alert"));
    assert!(html.contains("https://example.test/&quot; onclick=&quot;alert(1)"));
    assert!(!html.contains("\" onclick=\"alert(1)"));
}

#[test]
fn export_conversation_serializes_with_stable_shape() {
    let conversation = ExportConversation::new(
        "Stable",
        vec![
            ExportMessage::new("user", "Hello"),
            ExportMessage::new("assistant", "Hi"),
        ],
    );

    let value = serde_json::to_value(&conversation).unwrap();

    assert_eq!(
        value,
        json!({
            "title": "Stable",
            "messages": [
                { "role": "user", "content": "Hello" },
                { "role": "assistant", "content": "Hi" }
            ]
        })
    );
}

#[test]
fn export_html_can_omit_default_css() {
    let conversation = ExportConversation::new("Plain", vec![ExportMessage::new("user", "Hello")]);

    let html = export_html(
        &conversation,
        &HtmlExportOptions {
            include_default_css: false,
        },
    )
    .unwrap();

    assert!(!html.contains("<style>"));
    assert!(!html.contains(":root { color-scheme: light dark;"));
    assert!(html.contains("<main class=\"conversation\">"));
}

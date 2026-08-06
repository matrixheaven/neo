use super::*;

#[test]
fn sanitize_replaces_special_chars() {
    assert_eq!(sanitize_tool_name_segment("a/b"), "a_b");
    assert_eq!(sanitize_tool_name_segment(""), "unnamed");
}

#[test]
fn namespaced_tool_name_format() {
    assert_eq!(
        namespaced_tool_name("filesystem", "read_file"),
        "mcp__filesystem__read_file"
    );
}

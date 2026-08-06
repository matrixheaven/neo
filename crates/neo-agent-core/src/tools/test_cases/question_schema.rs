use super::*;
use tokio::sync::mpsc;

#[test]
fn tool_name_and_description() {
    let (tx, _rx) = mpsc::unbounded_channel::<PendingQuestion>();
    let tool = AskUserTool::new(tx);
    assert_eq!(tool.name(), "AskUserQuestion");
    assert!(!tool.description().is_empty());
}

#[test]
fn schema_has_questions_array() {
    let (tx, _rx) = mpsc::unbounded_channel::<PendingQuestion>();
    let tool = AskUserTool::new(tx);
    let schema = tool.input_schema();
    let props = schema
        .get("properties")
        .expect("properties")
        .as_object()
        .unwrap();
    assert!(props.contains_key("questions"));
}

#[test]
fn schema_has_background_flag() {
    let (tx, _rx) = mpsc::unbounded_channel::<PendingQuestion>();
    let tool = AskUserTool::new(tx);
    let schema = tool.input_schema();
    let props = schema
        .get("properties")
        .expect("properties")
        .as_object()
        .unwrap();
    assert!(props.contains_key("background"));
}

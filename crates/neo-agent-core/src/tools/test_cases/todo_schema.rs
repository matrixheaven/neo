use super::*;
use std::sync::Arc;
use std::sync::Mutex;

#[test]
fn schema_has_optional_todos_array() {
    let tool = TodoTool::new();
    let schema = tool.input_schema();
    let props = schema
        .get("properties")
        .expect("properties")
        .as_object()
        .unwrap();
    assert!(props.contains_key("todos"));
    let required = schema.get("required").and_then(|v| v.as_array());
    assert!(!required.is_some_and(|arr| { arr.iter().any(|v| v.as_str() == Some("todos")) }));
}

#[test]
fn description_contains_usage_guidance() {
    let tool = TodoTool::new();
    let description = tool.description();
    assert!(description.contains("When to use"));
    assert!(description.contains("When NOT to use"));
    assert!(description.contains("How to use"));
    assert!(description.contains("`in_progress`"));
}

#[test]
fn schema_descriptions_are_present() {
    let tool = TodoTool::new();
    let schema = tool.input_schema();
    let props = schema
        .get("properties")
        .expect("properties")
        .as_object()
        .unwrap();
    let todos = props.get("todos").expect("todos schema");
    assert!(
        todos.get("description").is_some(),
        "todos field should have a description"
    );
    // The item schema is either inline or referenced via $ref in schemars.
    let items = todos.get("items").expect("todos items");
    let item_schema = if let Some(reference) = items.get("$ref").and_then(|v| v.as_str()) {
        let definitions = schema
            .get("$defs")
            .or_else(|| schema.get("definitions"))
            .expect("schema definitions");
        definitions
            .get(reference.split('/').next_back().expect("ref name"))
            .expect("resolved item schema")
    } else {
        items
    };
    assert!(
        item_schema.get("properties").is_some(),
        "item schema should expose properties"
    );
}

#[test]
fn current_todos_reflects_state() {
    let shared: Arc<Mutex<Vec<TodoEventData>>> = Arc::new(Mutex::new(vec![TodoEventData {
        title: "X".into(),
        status: "done".into(),
    }]));
    let tool = TodoTool::with_state(shared);
    let todos = tool.current_todos();
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0].title, "X");
}

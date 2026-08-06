use super::*;

#[test]
fn tool_descriptions_are_non_empty() {
    assert!(!TaskListTool.description().is_empty());
    assert!(!TaskOutputTool.description().is_empty());
    assert!(!TaskPauseTool.description().is_empty());
    assert!(!TaskResumeTool.description().is_empty());
    assert!(!TaskStopTool.description().is_empty());
}

use super::*;

#[test]
fn prevalidate_accepts_valid_input() {
    let input = json!({
        "plan_summary": "Add feature",
        "options": [
            {"label": "Approach A", "description": "Simple"},
            {"label": "Approach B", "description": "Fast"}
        ]
    });
    assert!(prevalidate_exit_plan_mode(&input).is_ok());
}

#[test]
fn prevalidate_rejects_reserved_label() {
    let input = json!({
        "options": [{"label": "Approve"}]
    });
    assert!(prevalidate_exit_plan_mode(&input).is_err());
}

#[test]
fn prevalidate_rejects_duplicate_label() {
    let input = json!({
        "options": [
            {"label": "Same"},
            {"label": "same"}
        ]
    });
    assert!(prevalidate_exit_plan_mode(&input).is_err());
}

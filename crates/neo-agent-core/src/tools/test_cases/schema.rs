use super::*;

#[test]
fn enter_plan_mode_schema_is_valid() {
    let schema = EnterPlanModeTool.input_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["additionalProperties"], false);
}

#[test]
fn exit_plan_mode_schema_does_not_require_summary() {
    let schema = ExitPlanModeTool.input_schema();
    assert_eq!(schema["type"], "object");
    let plan_summary = resolve_schema_ref(&schema, &schema["properties"]["plan_summary"]);
    assert!(
        plan_summary["type"].is_string()
            || plan_summary["type"].as_array().is_some_and(|types| {
                types.iter().any(|t| t == "string") && types.iter().any(|t| t == "null")
            })
            || plan_summary.get("anyOf").is_some()
            || plan_summary.get("oneOf").is_some(),
        "plan_summary schema should be a string or optional string, got: {plan_summary}"
    );
    let required = schema["required"].as_array();
    assert!(!required.is_some_and(|arr| { arr.iter().any(|v| v == "plan_summary") }));
}

#[test]
fn exit_plan_mode_schema_has_options() {
    let schema = ExitPlanModeTool.input_schema();
    let options = resolve_schema_ref(&schema, &schema["properties"]["options"]);
    assert!(
        options["type"] == "array"
            || options["type"].as_array().is_some_and(|types| {
                types.iter().any(|t| t == "array") && types.iter().any(|t| t == "null")
            })
            || options.get("anyOf").is_some()
            || options.get("oneOf").is_some(),
        "options schema should be an array or optional array, got: {options}"
    );
}

fn resolve_schema_ref<'schema>(root: &'schema Value, node: &'schema Value) -> &'schema Value {
    if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
        let defs = root
            .get("$defs")
            .or_else(|| root.get("definitions"))
            .expect("schema defs");
        let name = reference.split('/').next_back().expect("ref name");
        return &defs[name];
    }
    node
}

#[test]
fn tool_names() {
    assert_eq!(EnterPlanModeTool.name(), "EnterPlanMode");
    assert_eq!(ExitPlanModeTool.name(), "ExitPlanMode");
}

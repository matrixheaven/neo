use neo_agent_core::ToolRegistry;
use serde_json::Value;

/// Verifies that every built-in tool's input schema has a non-empty description
/// on every object property (recursing into `$defs`/`definitions`).
#[test]
fn builtin_tool_schema_fields_have_descriptions() {
    let registry = ToolRegistry::with_builtin_tools();
    let mut failures = Vec::new();
    for tool in registry.specs() {
        let schema = tool.input_schema;
        if schema.get("properties").is_some() {
            check_schema(&tool.name, &schema, &schema, &mut failures);
        }
    }
    assert!(
        failures.is_empty(),
        "tool schema fields missing descriptions:\n{}",
        failures.join("\n")
    );
}

fn check_schema(tool_name: &str, root: &Value, node: &Value, failures: &mut Vec<String>) {
    let Some(obj) = node.as_object() else {
        return;
    };

    if let Some(props) = obj.get("properties").and_then(Value::as_object) {
        for (key, prop) in props {
            // The description lives on the property itself, not on a $ref target.
            if prop
                .get("description")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                failures.push(format!("{tool_name}: property `{key}` missing description"));
            }
            // Recurse into the referenced/combined schema for nested properties.
            check_schema(tool_name, root, resolve_ref(root, prop), failures);
        }
    }

    // Follow $defs / definitions for referenced types.
    for defs_key in ["$defs", "definitions"] {
        if let Some(defs) = obj.get(defs_key).and_then(Value::as_object) {
            for def in defs.values() {
                check_schema(tool_name, root, def, failures);
            }
        }
    }
}

fn resolve_ref<'a>(root: &'a Value, node: &'a Value) -> &'a Value {
    if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
        let name = reference.split('/').next_back().expect("ref name");
        let defs = root.get("$defs").or_else(|| root.get("definitions"));
        if let Some(def) = defs.and_then(|d| d.get(name)) {
            return def;
        }
    }
    node
}

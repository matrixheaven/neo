use neo_agent_core::ToolRegistry;
use serde_json::Value;

/// Terminal(start) exposes a typed `cwd` and both shell tools require it for
/// nested scopes, because command strings are never parsed for paths.
#[test]
fn terminal_start_exposes_cwd_and_requires_it_for_nested_scope() {
    let registry = ToolRegistry::with_builtin_tools();
    let specs = registry.specs();
    let terminal = specs
        .iter()
        .find(|spec| spec.name == "Terminal")
        .expect("Terminal spec");
    let bash = specs
        .iter()
        .find(|spec| spec.name == "Bash")
        .expect("Bash spec");

    // Terminal's schema exposes `cwd` as a start-only working directory.
    let cwd_schema = terminal
        .input_schema
        .get("properties")
        .and_then(|properties| properties.get("cwd"))
        .expect("Terminal must expose a `cwd` property");
    let cwd_description = cwd_schema
        .get("description")
        .and_then(Value::as_str)
        .expect("`cwd` description");
    assert!(
        cwd_description.contains("start"),
        "`cwd` must be documented as start-only: {cwd_description}"
    );
    assert!(
        cwd_description.contains("AGENTS.md"),
        "`cwd` must name the nested AGENTS.md scope requirement: {cwd_description}"
    );

    // Both shell tools' guidance requires the typed `cwd` for nested scopes.
    for (name, description) in [
        ("Terminal", &terminal.description),
        ("Bash", &bash.description),
    ] {
        assert!(
            description.contains("cwd"),
            "{name} guidance must mention `cwd`"
        );
        assert!(
            description.contains("never parsed") || description.contains("never inspected"),
            "{name} guidance must state command text is not parsed for paths: {description}"
        );
        assert!(
            description.contains("AGENTS.md"),
            "{name} guidance must name the nested AGENTS.md requirement: {description}"
        );
    }

    // Bash's `cwd` field carries the same nested-scope requirement.
    let bash_cwd = bash
        .input_schema
        .get("properties")
        .and_then(|properties| properties.get("cwd"))
        .and_then(|cwd| cwd.get("description"))
        .and_then(Value::as_str)
        .expect("Bash `cwd` description");
    assert!(
        bash_cwd.contains("AGENTS.md"),
        "Bash `cwd` must name the nested AGENTS.md scope requirement: {bash_cwd}"
    );
}

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

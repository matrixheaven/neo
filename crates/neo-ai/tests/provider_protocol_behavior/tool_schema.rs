use neo_ai::ToolSpec;
use serde_json::json;

#[test]
fn tool_spec_helpers_build_single_string_schema() {
    let tool = ToolSpec::string_arg("read_file", "Read a file", "path", "Path to read");

    assert_eq!(tool.input_schema["type"], "object");
    assert_eq!(tool.input_schema["properties"]["path"]["type"], "string");
    assert_eq!(
        tool.input_schema["properties"]["path"]["description"],
        "Path to read"
    );
    assert_eq!(tool.input_schema["required"], json!(["path"]));
}

#[test]
fn normalize_tool_schema_removes_provider_hostile_metadata() {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$defs": {
            "TerminalMode": {
                "oneOf": [
                    { "const": "start", "description": "Launch a new PTY session.", "type": "string" },
                    { "const": "read", "description": "Read buffered output.", "type": "string" }
                ]
            }
        },
        "title": "TerminalInput",
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "mode": {
                "$ref": "#/$defs/TerminalMode",
                "description": "The operation to perform."
            },
            "command": {
                "description": "The shell command to launch.",
                "type": ["string", "null"],
                "default": null
            },
            "timeout": {
                "description": "Optional timeout in seconds.",
                "format": "uint64",
                "minimum": 0,
                "type": ["integer", "null"]
            }
        },
        "required": ["mode"]
    });

    let normalized = neo_ai::tool_schema::normalize_tool_schema(&schema);

    assert_eq!(
        normalized,
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "mode": {
                    "description": "The operation to perform.",
                    "type": "string",
                    "enum": ["start", "read"]
                },
                "command": {
                    "description": "The shell command to launch.",
                    "type": "string"
                },
                "timeout": {
                    "description": "Optional timeout in seconds.",
                    "minimum": 0,
                    "type": "integer"
                }
            },
            "required": ["mode"]
        })
    );
}

#[test]
fn normalize_tool_schema_requires_an_object_root() {
    let missing_type = json!({
        "oneOf": [
            { "type": "object", "properties": { "action": { "const": "preview" } } },
            { "type": "object", "properties": { "action": { "const": "save" } } }
        ]
    });
    let null_type = json!({ "type": null, "properties": {} });

    assert_eq!(
        neo_ai::tool_schema::normalize_tool_schema(&missing_type)["type"],
        "object"
    );
    assert_eq!(
        neo_ai::tool_schema::normalize_tool_schema(&null_type)["type"],
        "object"
    );
}

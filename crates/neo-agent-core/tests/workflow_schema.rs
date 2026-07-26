//! Host schema validation tests for workflow structured outputs.

use neo_agent_core::workflow::{
    CompiledSchema, SchemaErrorCode, StructuredOutputSource, accept_structured_output,
    attach_response_format_hint, parse_strict_json_value,
};
use neo_ai::RequestOptions;
use serde_json::json;

#[test]
fn provider_native_and_text_fallback_share_host_validation() {
    let schema_doc = json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" }
        },
        "required": ["ok"],
        "additionalProperties": false
    });
    let schema = CompiledSchema::compile(&schema_doc).expect("compile schema");

    let native = accept_structured_output(
        &schema,
        StructuredOutputSource::ProviderNative(json!({ "ok": true })),
    )
    .expect("provider-native valid value");
    let text = accept_structured_output(
        &schema,
        StructuredOutputSource::AssistantText(r#"{"ok":true}"#.to_owned()),
    )
    .expect("text fallback valid value");
    assert_eq!(native, text);
    assert_eq!(native, json!({ "ok": true }));

    // Both invalid paths fail with the same host error code.
    let bad_native = accept_structured_output(
        &schema,
        StructuredOutputSource::ProviderNative(json!({ "ok": "nope" })),
    )
    .expect_err("type mismatch must fail host validation");
    let bad_text = accept_structured_output(
        &schema,
        StructuredOutputSource::AssistantText(r#"{"ok":"nope"}"#.to_owned()),
    )
    .expect_err("type mismatch must fail host validation");
    assert_eq!(bad_native.code, SchemaErrorCode::SchemaInvalid);
    assert_eq!(bad_text.code, SchemaErrorCode::SchemaInvalid);

    // Text fallback is strict: fences / prose never become accepted JSON.
    let fenced = accept_structured_output(
        &schema,
        StructuredOutputSource::AssistantText("```json\n{\"ok\":true}\n```".to_owned()),
    )
    .expect_err("markdown fences must fail");
    assert_eq!(fenced.code, SchemaErrorCode::StrictJsonFailed);

    let prose = accept_structured_output(
        &schema,
        StructuredOutputSource::AssistantText("sure: {\"ok\":true}".to_owned()),
    )
    .expect_err("prose must fail");
    assert_eq!(prose.code, SchemaErrorCode::StrictJsonFailed);

    // Composition seam attaches a provider-neutral hint without bypassing host
    // validation: wire acceptance is not simulated; host still rejects bad values.
    let mut options = RequestOptions::default();
    attach_response_format_hint(&mut options, "child_output", schema_doc.clone(), true);
    let hint = options
        .response_format
        .expect("response format hint attached");
    assert_eq!(hint.name, "child_output");
    assert!(hint.strict);
    assert_eq!(hint.schema, schema_doc);

    // Even with a hint present, invalid text still fails the same host path.
    let still_invalid = accept_structured_output(
        &schema,
        StructuredOutputSource::AssistantText("not json".to_owned()),
    )
    .expect_err("hint does not skip host validation");
    assert_eq!(still_invalid.code, SchemaErrorCode::StrictJsonFailed);

    // Strict parse helper rejects multiple top-level values.
    let multi = parse_strict_json_value("{\"ok\":true}{\"ok\":false}")
        .expect_err("multiple values must fail");
    assert_eq!(multi.code, SchemaErrorCode::StrictJsonFailed);
}

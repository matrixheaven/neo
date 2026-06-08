use std::collections::BTreeMap;
use std::time::Duration;

use neo_ai::{
    CacheRetention, ReasoningEffort, RequestMetadata, RequestOptions, env_api_key_from,
    find_env_keys_from,
};

#[test]
fn env_api_key_resolves_provider_specific_variables_in_priority_order() {
    let env = BTreeMap::from([
        ("ANTHROPIC_API_KEY".to_owned(), "anthropic-key".to_owned()),
        (
            "ANTHROPIC_OAUTH_TOKEN".to_owned(),
            "anthropic-oauth".to_owned(),
        ),
        ("OPENAI_API_KEY".to_owned(), "openai-key".to_owned()),
    ]);

    assert_eq!(
        env_api_key_from("openai", &env),
        Some("openai-key".to_owned())
    );
    assert_eq!(
        env_api_key_from("anthropic", &env),
        Some("anthropic-oauth".to_owned())
    );
    assert_eq!(
        find_env_keys_from("anthropic", &env),
        vec![
            "ANTHROPIC_OAUTH_TOKEN".to_owned(),
            "ANTHROPIC_API_KEY".to_owned()
        ]
    );
    assert_eq!(env_api_key_from("unknown", &env), None);
}

#[test]
fn request_options_have_typed_defaults_and_preserve_metadata() {
    let options = RequestOptions {
        temperature: Some(0.2),
        max_tokens: Some(4096),
        headers: BTreeMap::from([("x-neo-trace".to_owned(), "trace-1".to_owned())]),
        timeout: Some(Duration::from_secs(15)),
        reasoning_effort: Some(ReasoningEffort::High),
        retries: Some(2),
        cache: CacheRetention::Long,
        session_id: Some("session-123".to_owned()),
        metadata: RequestMetadata::from_pairs([("user_id", "u-1"), ("project_id", "p-1")]),
    };

    assert_eq!(options.temperature, Some(0.2));
    assert_eq!(options.max_tokens, Some(4096));
    assert_eq!(options.timeout, Some(Duration::from_secs(15)));
    assert_eq!(options.reasoning_effort, Some(ReasoningEffort::High));
    assert_eq!(options.retries, Some(2));
    assert_eq!(options.cache, CacheRetention::Long);
    assert_eq!(options.session_id.as_deref(), Some("session-123"));
    assert_eq!(options.metadata.get("project_id"), Some("p-1"));
}

#[test]
fn reasoning_effort_serializes_as_stable_snake_case_values() {
    assert_eq!(
        serde_json::to_value(ReasoningEffort::Minimal).expect("serialize effort"),
        serde_json::json!("minimal")
    );
    assert_eq!(
        serde_json::from_value::<ReasoningEffort>(serde_json::json!("xhigh"))
            .expect("deserialize effort"),
        ReasoningEffort::XHigh
    );
}

#[test]
fn request_options_default_to_short_cache_without_transport_overrides() {
    let options = RequestOptions::default();

    assert_eq!(options.temperature, None);
    assert_eq!(options.max_tokens, None);
    assert!(options.headers.is_empty());
    assert_eq!(options.timeout, None);
    assert_eq!(options.reasoning_effort, None);
    assert_eq!(options.retries, Some(0));
    assert_eq!(options.cache, CacheRetention::Short);
    assert_eq!(options.session_id, None);
    assert!(options.metadata.is_empty());
}

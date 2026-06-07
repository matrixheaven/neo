use std::{collections::BTreeMap, env};

const PROVIDER_ENV_KEYS: &[(&str, &[&str])] = &[
    ("anthropic", &["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]),
    ("openai", &["OPENAI_API_KEY"]),
    (
        "openai-codex",
        &["OPENAI_CODEX_OAUTH_TOKEN", "OPENAI_API_KEY"],
    ),
    (
        "github-copilot",
        &["GITHUB_COPILOT_OAUTH_TOKEN", "GITHUB_TOKEN"],
    ),
    ("google", &["GOOGLE_API_KEY", "GEMINI_API_KEY"]),
    ("google-vertex", &["GOOGLE_APPLICATION_CREDENTIALS"]),
    ("mistral", &["MISTRAL_API_KEY"]),
    ("openrouter", &["OPENROUTER_API_KEY"]),
    ("bedrock", &["AWS_ACCESS_KEY_ID"]),
    ("amazon-bedrock", &["AWS_ACCESS_KEY_ID"]),
];

#[must_use]
pub fn find_env_keys(provider: &str) -> Vec<String> {
    find_env_keys_from(provider, &env::vars().collect())
}

#[must_use]
pub fn env_api_key(provider: &str) -> Option<String> {
    env_api_key_from(provider, &env::vars().collect())
}

#[must_use]
pub fn find_env_keys_from(provider: &str, env: &BTreeMap<String, String>) -> Vec<String> {
    provider_keys(provider)
        .iter()
        .filter(|key| env.contains_key(**key))
        .map(|key| (*key).to_owned())
        .collect()
}

#[must_use]
pub fn env_api_key_from(provider: &str, env: &BTreeMap<String, String>) -> Option<String> {
    provider_keys(provider)
        .iter()
        .find_map(|key| env.get(*key).filter(|value| !value.is_empty()).cloned())
}

fn provider_keys(provider: &str) -> &'static [&'static str] {
    PROVIDER_ENV_KEYS
        .iter()
        .find_map(|(known, keys)| (*known == provider).then_some(*keys))
        .unwrap_or(&[])
}

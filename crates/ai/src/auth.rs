use std::{collections::BTreeMap, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    Cli,
    Environment,
    AuthFile,
    CloudProfile,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedCredential {
    secret: String,
    source: CredentialSource,
    label: String,
}

impl ResolvedCredential {
    #[must_use]
    pub fn new(
        secret: impl Into<String>,
        source: CredentialSource,
        label: impl Into<String>,
    ) -> Self {
        Self {
            secret: secret.into(),
            source,
            label: label.into(),
        }
    }

    #[must_use]
    pub fn secret(&self) -> &str {
        &self.secret
    }

    #[must_use]
    pub const fn source(&self) -> CredentialSource {
        self.source
    }

    #[must_use]
    pub fn redacted_label(&self) -> &str {
        &self.label
    }
}

impl fmt::Debug for ResolvedCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedCredential")
            .field("source", &self.source)
            .field("label", &self.label)
            .field("secret", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct CredentialResolver {
    provider: String,
    cli_api_key: Option<String>,
    env_vars: Vec<String>,
    env: BTreeMap<String, String>,
    auth_file_credentials: BTreeMap<String, String>,
    cloud_profile_credentials: BTreeMap<String, String>,
}

impl CredentialResolver {
    #[must_use]
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            cli_api_key: None,
            env_vars: Vec::new(),
            env: BTreeMap::new(),
            auth_file_credentials: BTreeMap::new(),
            cloud_profile_credentials: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_cli_api_key(mut self, api_key: Option<String>) -> Self {
        self.cli_api_key = api_key.filter(|value| !value.is_empty());
        self
    }

    #[must_use]
    pub fn with_env<'a>(
        mut self,
        env_vars: impl IntoIterator<Item = &'a str>,
        env: &BTreeMap<String, String>,
    ) -> Self {
        self.env_vars = env_vars.into_iter().map(str::to_owned).collect();
        self.env = env.clone();
        self
    }

    #[must_use]
    pub fn with_auth_file_credentials(mut self, credentials: BTreeMap<String, String>) -> Self {
        self.auth_file_credentials = credentials;
        self
    }

    #[must_use]
    pub fn with_cloud_profile_credentials(mut self, credentials: BTreeMap<String, String>) -> Self {
        self.cloud_profile_credentials = credentials;
        self
    }

    #[must_use]
    pub fn resolve(&self) -> Option<ResolvedCredential> {
        self.cli_api_key
            .as_ref()
            .map(|secret| {
                ResolvedCredential::new(secret.clone(), CredentialSource::Cli, "cli --api-key")
            })
            .or_else(|| self.resolve_env())
            .or_else(|| {
                self.auth_file_credentials
                    .get(&self.provider)
                    .filter(|value| !value.is_empty())
                    .map(|secret| {
                        ResolvedCredential::new(
                            secret.clone(),
                            CredentialSource::AuthFile,
                            "auth file",
                        )
                    })
            })
            .or_else(|| {
                self.cloud_profile_credentials
                    .get(&self.provider)
                    .filter(|value| !value.is_empty())
                    .map(|secret| {
                        ResolvedCredential::new(
                            secret.clone(),
                            CredentialSource::CloudProfile,
                            "cloud profile",
                        )
                    })
            })
    }

    fn resolve_env(&self) -> Option<ResolvedCredential> {
        self.env_vars.iter().find_map(|key| {
            self.env
                .get(key)
                .filter(|value| !value.is_empty())
                .map(|secret| {
                    ResolvedCredential::new(
                        secret.clone(),
                        CredentialSource::Environment,
                        format!("env {key}"),
                    )
                })
        })
    }
}

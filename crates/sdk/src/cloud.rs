pub use neo_cloud_protocol::{
    BootstrapRequest, BootstrapResponse, CloudCommandCatalogResponse, CloudCommandRecord,
    CloudContinueSessionRequest, CloudContinueSessionResponse, CloudCreateShareRequest,
    CloudForkSessionResponse, CloudImportSessionRequest, CloudImportSessionResponse, CloudProfile,
    CloudSessionListResponse, CloudSessionPayload, CloudSessionRecord, CloudSessionTreeRecord,
    CloudSessionTreeResponse, CloudShareListResponse, CloudSharePayload, CloudShareRecord,
    CloudShareRecordResponse, CloudUpdateBranchRequest, CloudUpdateBranchResponse,
    DeviceTokenLoginRequest, HealthResponse, ProfilePullResponse, ProfilePushRequest,
    ProfileStatusResponse, SettingsPullResponse, SettingsPushRequest,
};

#[derive(Debug, Clone)]
pub struct CloudClient {
    base_url: String,
    client: reqwest::Client,
}

impl CloudClient {
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn bootstrap(&self, device_name: &str) -> anyhow::Result<BootstrapResponse> {
        self.client
            .post(format!("{}/v1/auth/bootstrap", self.base_url))
            .json(&BootstrapRequest {
                device_name: device_name.to_owned(),
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn login_with_device_token(
        &self,
        device_id: &str,
        device_token: &str,
    ) -> anyhow::Result<BootstrapResponse> {
        self.client
            .post(format!("{}/v1/auth/device-token", self.base_url))
            .json(&DeviceTokenLoginRequest {
                device_id: device_id.to_owned(),
                device_token: device_token.to_owned(),
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn push_profile(
        &self,
        access_token: &str,
        profile: CloudProfile,
    ) -> anyhow::Result<ProfileStatusResponse> {
        self.client
            .put(format!("{}/v1/profile", self.base_url))
            .bearer_auth(access_token)
            .json(&ProfilePushRequest { profile })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn pull_profile(&self, access_token: &str) -> anyhow::Result<ProfilePullResponse> {
        self.client
            .get(format!("{}/v1/profile", self.base_url))
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn profile_status(
        &self,
        access_token: &str,
    ) -> anyhow::Result<ProfileStatusResponse> {
        self.client
            .get(format!("{}/v1/profile/status", self.base_url))
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn health(&self) -> anyhow::Result<HealthResponse> {
        self.client
            .get(format!("{}/v1/health", self.base_url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn pull_settings(&self, access_token: &str) -> anyhow::Result<SettingsPullResponse> {
        self.client
            .get(format!("{}/v1/settings", self.base_url))
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn push_settings(
        &self,
        access_token: &str,
        settings: CloudProfile,
    ) -> anyhow::Result<ProfileStatusResponse> {
        self.client
            .put(format!("{}/v1/settings", self.base_url))
            .bearer_auth(access_token)
            .json(&SettingsPushRequest { settings })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn command_catalog(
        &self,
        access_token: &str,
    ) -> anyhow::Result<CloudCommandCatalogResponse> {
        self.client
            .get(format!("{}/v1/commands", self.base_url))
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn import_session(
        &self,
        access_token: &str,
        local_session_id: &str,
        jsonl: String,
        name: Option<String>,
        summary: Option<String>,
        remote_parent_id: Option<String>,
    ) -> anyhow::Result<CloudSessionRecord> {
        let response = self
            .client
            .post(format!("{}/v1/sessions/import", self.base_url))
            .bearer_auth(access_token)
            .json(&CloudImportSessionRequest {
                local_session_id: local_session_id.to_owned(),
                jsonl,
                name,
                summary,
                remote_parent_id,
            })
            .send()
            .await?
            .error_for_status()?
            .json::<CloudImportSessionResponse>()
            .await?;
        Ok(response.record)
    }

    pub async fn list_sessions(
        &self,
        access_token: &str,
    ) -> anyhow::Result<Vec<CloudSessionRecord>> {
        let response = self
            .client
            .get(format!("{}/v1/sessions", self.base_url))
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json::<CloudSessionListResponse>()
            .await?;
        Ok(response.sessions)
    }

    pub async fn session_tree(
        &self,
        access_token: &str,
    ) -> anyhow::Result<Vec<CloudSessionTreeRecord>> {
        let response = self
            .client
            .get(format!("{}/v1/sessions/tree", self.base_url))
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json::<CloudSessionTreeResponse>()
            .await?;
        Ok(response.tree)
    }

    pub async fn get_session(
        &self,
        access_token: &str,
        session_id: &str,
    ) -> anyhow::Result<CloudSessionPayload> {
        self.client
            .get(format!("{}/v1/sessions/{session_id}", self.base_url))
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn update_branch(
        &self,
        access_token: &str,
        session_id: &str,
        name: Option<String>,
        summary: Option<String>,
        remote_parent_id: Option<String>,
    ) -> anyhow::Result<CloudSessionRecord> {
        let response = self
            .client
            .put(format!("{}/v1/sessions/{session_id}/branch", self.base_url))
            .bearer_auth(access_token)
            .json(&CloudUpdateBranchRequest {
                name,
                summary,
                remote_parent_id,
            })
            .send()
            .await?
            .error_for_status()?
            .json::<CloudUpdateBranchResponse>()
            .await?;
        Ok(response.record)
    }

    pub async fn continue_session(
        &self,
        access_token: &str,
        session_id: &str,
        local_session_id: Option<String>,
        name: Option<String>,
    ) -> anyhow::Result<CloudContinueSessionResponse> {
        self.client
            .post(format!(
                "{}/v1/sessions/{session_id}/continue",
                self.base_url
            ))
            .bearer_auth(access_token)
            .json(&CloudContinueSessionRequest {
                local_session_id,
                name,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn fork_session(
        &self,
        access_token: &str,
        session_id: &str,
    ) -> anyhow::Result<CloudSessionRecord> {
        let response = self
            .client
            .post(format!("{}/v1/sessions/{session_id}/fork", self.base_url))
            .bearer_auth(access_token)
            .json(&serde_json::json!({}))
            .send()
            .await?
            .error_for_status()?
            .json::<CloudForkSessionResponse>()
            .await?;
        Ok(response.record)
    }

    pub async fn list_share_records(
        &self,
        access_token: &str,
        session_id: &str,
    ) -> anyhow::Result<Vec<CloudShareRecord>> {
        let response = self
            .client
            .get(format!("{}/v1/sessions/{session_id}/shares", self.base_url))
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json::<CloudShareListResponse>()
            .await?;
        Ok(response.shares)
    }

    pub async fn create_share(
        &self,
        access_token: &str,
        session_id: &str,
        public: bool,
    ) -> anyhow::Result<CloudSharePayload> {
        self.client
            .post(format!("{}/v1/sessions/{session_id}/shares", self.base_url))
            .bearer_auth(access_token)
            .json(&CloudCreateShareRequest { public })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn get_share_record(
        &self,
        access_token: &str,
        share_id: &str,
    ) -> anyhow::Result<CloudShareRecord> {
        let response = self
            .client
            .get(format!("{}/v1/share-records/{share_id}", self.base_url))
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json::<CloudShareRecordResponse>()
            .await?;
        Ok(response.record)
    }

    pub async fn get_share(&self, share_id: &str) -> anyhow::Result<CloudSharePayload> {
        self.client
            .get(format!("{}/v1/shares/{share_id}", self.base_url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(Into::into)
    }
}

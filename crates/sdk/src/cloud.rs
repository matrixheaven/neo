use neo_cloud_protocol::{
    BootstrapRequest, BootstrapResponse, CloudProfile, DeviceTokenLoginRequest, HealthResponse,
    ProfilePullResponse, ProfilePushRequest, ProfileStatusResponse,
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
}

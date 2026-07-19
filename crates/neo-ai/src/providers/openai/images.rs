use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::json;

use crate::providers::common::error::ProviderError;
use crate::providers::common::http::http_status_error;
use crate::{
    AiError, ImageData, ImageGenerationClient, ImageGenerationRequest, ImageGenerationResponse,
    ImageGenerationResponseImage,
};

#[derive(Clone)]
pub struct OpenAiImagesClient {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiImagesClient {
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl ImageGenerationClient for OpenAiImagesClient {
    async fn generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, AiError> {
        let url = format!("{}/images/generations", self.base_url);
        let body = json!({
            "model": request.model.model,
            "prompt": request.prompt,
            "size": request.size,
            "n": 1,
        });
        let response = self
            .client
            .post(url)
            .headers(headers(&self.api_key)?)
            .json(&body)
            .send()
            .await
            .map_err(ProviderError::Transport)
            .map_err(ProviderError::into_ai_error)?;
        if !response.status().is_success() {
            return Err(http_status_error(response).await.into_ai_error());
        }
        let response = response
            .json::<OpenAiImagesResponse>()
            .await
            .map_err(|err| AiError::Protocol {
                message: format!("invalid image response: {err}"),
            })?;
        let images = response
            .data
            .into_iter()
            .map(|image| {
                let data = match (image.b64_json, image.url) {
                    (Some(value), _) if !value.is_empty() => ImageData::Base64(value),
                    (_, Some(value)) if !value.is_empty() => ImageData::Url(value),
                    _ => {
                        return Err(AiError::Protocol {
                            message: "image response did not include b64_json or url".to_owned(),
                        });
                    }
                };
                Ok(ImageGenerationResponseImage {
                    mime_type: "image/png".to_owned(),
                    data,
                    revised_prompt: image.revised_prompt,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ImageGenerationResponse { images })
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiImagesResponse {
    data: Vec<OpenAiImage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiImage {
    #[serde(default)]
    b64_json: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    revised_prompt: Option<String>,
}

fn headers(api_key: &str) -> Result<HeaderMap, AiError> {
    let mut headers = HeaderMap::new();
    let authorization =
        HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| AiError::Protocol {
            message: format!("invalid authorization header: {err}"),
        })?;
    headers.insert(AUTHORIZATION, authorization);
    Ok(headers)
}

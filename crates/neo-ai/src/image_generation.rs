use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AiError, ImageData, ModelSpec};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImageGenerationRequest {
    pub model: ModelSpec,
    pub prompt: String,
    pub size: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImageGenerationResponse {
    pub images: Vec<ImageGenerationResponseImage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImageGenerationResponseImage {
    pub mime_type: String,
    pub data: ImageData,
    pub revised_prompt: Option<String>,
}

#[async_trait]
pub trait ImageGenerationClient: Send + Sync {
    async fn generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, AiError>;
}

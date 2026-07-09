pub mod auth;
pub mod catalog;
pub mod env_api_keys;
pub mod error;
pub mod error_info;
pub mod image_generation;
pub mod options;
pub mod providers;
pub mod reasoning;
pub mod registry;
pub mod stream;
pub mod tool_assembly;
pub mod tool_schema;
pub mod types;

pub use auth::{CredentialResolver, CredentialSource, ResolvedCredential};
pub use env_api_keys::{env_api_key_from, find_env_keys_from};
pub use error::AiError;
pub use error_info::{NeoErrorInfo, error_info};
pub use image_generation::{
    ImageGenerationClient, ImageGenerationRequest, ImageGenerationResponse,
    ImageGenerationResponseImage,
};
pub use options::{
    CacheRetention, ReasoningBudget, ReasoningCapability, ReasoningEffort, ReasoningSelection,
    RequestMetadata, RequestOptions,
};
pub use reasoning::{ReasoningContinuation, ReasoningPolicy, sanitize_reasoning_continuation};
pub use registry::{
    ModelRegistry, ProviderCredentialStatus, ProviderRegistry, ProviderResolver, ProviderSpec,
};
pub use stream::collect_tool_arguments;
pub use tool_schema::schema_for;
pub use types::*;

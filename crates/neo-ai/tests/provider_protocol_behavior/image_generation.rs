use std::time::Duration;

use neo_ai::{
    AiError, ApiKind, ImageData, ImageGenerationClient, ImageGenerationRequest,
    ImageGenerationResponseImage, ModelCapabilities, ModelSpec, ProviderId,
    providers::openai::images::OpenAiImagesClient,
};
use serde_json::json;

use super::http_server::{MockServer, json_response};

fn image_generation_request() -> ImageGenerationRequest {
    ImageGenerationRequest {
        model: ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "gpt-image-1".to_owned(),
            api: ApiKind::OpenAiResponse,
            capabilities: ModelCapabilities::vision_chat(),
        },
        prompt: "draw a quiet terminal".to_owned(),
        size: "1024x1024".to_owned(),
    }
}

#[tokio::test]
async fn openai_image_generation_client_serializes_request_and_decodes_base64_response() {
    let server = MockServer::start(vec![json_response(&json!({
        "created": 1_710_000_000,
        "data": [
            {
                "b64_json": "iVBORw0KGgo=",
                "revised_prompt": "draw a quiet terminal with soft light"
            }
        ]
    }))]);
    let client = OpenAiImagesClient::new(server.url.clone(), "test-key");

    let response = client
        .generate_image(image_generation_request())
        .await
        .expect("image generation should succeed");

    assert_eq!(
        response.images,
        vec![ImageGenerationResponseImage {
            mime_type: "image/png".to_owned(),
            data: ImageData::Base64("iVBORw0KGgo=".to_owned()),
            revised_prompt: Some("draw a quiet terminal with soft light".to_owned()),
        }]
    );
    let sent = server.requests().pop().expect("request");
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.path, "/images/generations");
    assert_eq!(
        sent.headers.get("authorization").unwrap(),
        "Bearer test-key"
    );
    assert_eq!(sent.body["model"], "gpt-image-1");
    assert_eq!(sent.body["prompt"], "draw a quiet terminal");
    assert_eq!(sent.body["size"], "1024x1024");
    assert_eq!(sent.body["n"], 1);
    assert!(sent.body.get("response_format").is_none());
}

#[tokio::test]
async fn openai_image_generation_client_preserves_rate_limit_details() {
    let body = json!({ "error": { "message": "slow down" } }).to_string();
    let server = MockServer::start(vec![format!(
        "HTTP/1.1 429 Too Many Requests\r\nretry-after: 7\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )]);
    let client = OpenAiImagesClient::new(server.url, "test-key");

    let error = client
        .generate_image(image_generation_request())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AiError::RateLimit {
            retry_after: Some(delay),
            message
        } if delay == Duration::from_secs(7) && message.contains("slow down")
    ));
}

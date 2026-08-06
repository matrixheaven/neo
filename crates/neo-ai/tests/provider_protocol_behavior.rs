//! Provider wire protocol behavior: request serialization, stream normalization, and error
//! mapping exercised against loopback HTTP servers.

#[path = "provider_protocol_behavior/anthropic.rs"]
mod anthropic;
#[path = "provider_protocol_behavior/google.rs"]
mod google;
#[path = "provider_protocol_behavior/http_server.rs"]
mod http_server;
#[path = "provider_protocol_behavior/image_generation.rs"]
mod image_generation;
#[path = "provider_protocol_behavior/openai_compatible.rs"]
mod openai_compatible;
#[path = "provider_protocol_behavior/openai_responses.rs"]
mod openai_responses;
#[path = "provider_protocol_behavior/stream_events.rs"]
mod stream_events;
#[path = "provider_protocol_behavior/tool_schema.rs"]
mod tool_schema;

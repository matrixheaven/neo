//! OpenAI-family provider wire clients.
//!
//! This module groups the three OpenAI-compatible API flavors:
//! - [`responses`]: OpenAI Responses API (`/responses` endpoint)
//! - [`compatible`]: OpenAI Chat Completions API (`/chat/completions` endpoint)
//! - [`images`]: OpenAI Images API (image generation)

pub mod compatible;
pub mod images;
pub mod responses;

//! Synchronous HTTP helpers for OpenAI-compatible surfaces.

/// HTTP/1.1 client.
pub mod client;
/// Chunked transfer decoding.
pub mod chunked;
/// Retry schedule helpers.
pub mod retry;

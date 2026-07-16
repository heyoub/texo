//! Synchronous HTTP helpers for OpenAI-compatible surfaces.

#[cfg(feature = "openrouter")]
mod codec;
#[cfg(feature = "openrouter")]
mod schedule;
mod types;

/// HTTP/1.1 client.
#[cfg(feature = "openrouter")]
pub mod client;
/// Chunked transfer decoding.
#[cfg(feature = "openrouter")]
pub mod chunked;
/// Request parser for the inbound server.
pub mod request;
/// Retry schedule helpers.
#[cfg(feature = "openrouter")]
pub mod retry;
/// Response helpers for the inbound server.
pub mod response;
/// Route handlers.
pub mod routes;
/// Blocking HTTP server.
pub mod server;
/// Server-sent events.
pub mod sse;

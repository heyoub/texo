//! texo MCP stdio server.

pub mod error;
pub mod protocol;
pub mod server;
pub mod tools;

pub use server::run_stdio;

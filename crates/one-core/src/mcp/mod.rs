//! MCP (Model Context Protocol) client implementation.
//!
//! Supports connecting to MCP servers, discovering tools, and executing them.
//! Transports: stdio (local), SSE (remote).

pub mod client;
pub mod config;
pub mod jsonrpc;
pub mod sse;
pub mod transport;

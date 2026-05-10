//! Model Context Protocol (MCP) endpoint.
//!
//! Exposes Uncloud as a remote MCP server so Claude.ai's connector and
//! `claude mcp add` can use it. JSON-RPC 2.0 over Streamable HTTP at
//! `/mcp`. Three read-only tools wrap the existing FileService and
//! SearchService; auth comes from the OAuth bearer plumbed in Phase 0
//! and is gated on `files:read`. See docs/mcp.md for the full design.

pub mod handler;
pub mod jsonrpc;
pub mod tools;

pub use handler::mcp_handler;

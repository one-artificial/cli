//! MCP resource tools — list and read resources from MCP servers.
//!
//! These tools provide access to MCP server resources (files, data, etc.)
//! without executing tools. Resources are read-only views of server state.
//!
//! Note: These are schema-only tools. Actual execution is handled by the
//! query engine, which has access to the MCP manager. The tools here
//! serve as placeholders whose schemas tell the AI how to call them.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

/// List resources available from connected MCP servers.
pub struct ListMcpResourcesTool;

impl Tool for ListMcpResourcesTool {
    fn name(&self) -> &str {
        "list_mcp_resources"
    }

    fn description(&self) -> &str {
        "List resources available from connected MCP servers. Resources are \
         read-only data exposed by servers (files, database entries, etc.)."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "Optional: filter resources by server name"
                }
            },
            "required": []
        })
    }

    fn execute(
        &self,
        _input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        // Execution is intercepted by the query engine (needs MCP manager access)
        Box::pin(async move {
            Ok(ToolResult::error(
                "list_mcp_resources execution not intercepted by query engine.",
            ))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&str> {
        Some("MCP resources list server data files")
    }
}

/// Read a specific resource by URI from an MCP server.
pub struct ReadMcpResourceTool;

impl Tool for ReadMcpResourceTool {
    fn name(&self) -> &str {
        "read_mcp_resource"
    }

    fn description(&self) -> &str {
        "Read a resource from an MCP server by URI. Use list_mcp_resources \
         first to discover available URIs."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "uri": {
                    "type": "string",
                    "description": "The resource URI to read (from list_mcp_resources)"
                }
            },
            "required": ["uri"]
        })
    }

    fn execute(
        &self,
        _input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        // Execution is intercepted by the query engine (needs MCP manager access)
        Box::pin(async move {
            Ok(ToolResult::error(
                "read_mcp_resource execution not intercepted by query engine.",
            ))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&str> {
        Some("MCP resource read fetch URI content")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_tool_properties() {
        let tool = ListMcpResourcesTool;
        assert_eq!(tool.name(), "list_mcp_resources");
        assert!(tool.is_read_only());
        assert!(tool.should_defer());
    }

    #[test]
    fn test_read_tool_properties() {
        let tool = ReadMcpResourceTool;
        assert_eq!(tool.name(), "read_mcp_resource");
        assert!(tool.is_read_only());
        assert!(tool.should_defer());
    }
}

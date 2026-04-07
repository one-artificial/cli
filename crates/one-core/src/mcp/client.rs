//! MCP client: manages connections, tool discovery, and tool execution.
//!
//! Each MCP server gets its own client instance. The McpManager coordinates
//! all connected servers and provides a unified tool interface.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::config::{McpServerConfig, load_mcp_configs};
use super::transport::StdioTransport;

/// Connection state for an MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Pending,
    Connected,
    Failed(String),
    Disabled,
}

/// A tool discovered from an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    /// Original tool name from the server.
    pub name: String,
    /// Fully qualified name: `mcp__servername__toolname`.
    pub qualified_name: String,
    /// Server this tool belongs to.
    pub server_name: String,
    /// Tool description.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: serde_json::Value,
}

impl McpTool {
    /// Build the tool schema in the format expected by AI providers.
    pub fn to_tool_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.qualified_name,
            "description": format!("[MCP: {}] {}", self.server_name, self.description),
            "input_schema": self.input_schema,
        })
    }
}

/// A connected MCP server with its transport and discovered capabilities.
pub struct McpConnection {
    pub name: String,
    pub state: ConnectionState,
    pub transport: Option<StdioTransport>,
    pub sse_transport: Option<super::sse::SseTransport>,
    pub tools: Vec<McpTool>,
}

impl McpConnection {
    /// Perform the MCP initialization handshake.
    async fn initialize(transport: &StdioTransport) -> Result<serde_json::Value> {
        let result = transport
            .request(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "roots": { "listChanged": true }
                    },
                    "clientInfo": {
                        "name": "one-cli",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
            )
            .await?;

        // Send initialized notification (required by MCP spec)
        transport.notify("notifications/initialized", None).await?;

        Ok(result)
    }

    /// Discover tools from the connected server.
    async fn discover_tools(transport: &StdioTransport, server_name: &str) -> Result<Vec<McpTool>> {
        let result = transport.request("tools/list", None).await?;

        let tools = result["tools"].as_array().cloned().unwrap_or_default();

        let mut mcp_tools = Vec::new();
        for tool in tools {
            let name = tool["name"].as_str().unwrap_or("").to_string();
            let description = tool["description"].as_str().unwrap_or("").to_string();
            let input_schema = tool["inputSchema"].clone();

            if name.is_empty() {
                continue;
            }

            let qualified_name = build_tool_name(server_name, &name);

            mcp_tools.push(McpTool {
                name,
                qualified_name,
                server_name: server_name.to_string(),
                description,
                input_schema,
            });
        }

        Ok(mcp_tools)
    }

    /// Send a request via whichever transport is connected (stdio or SSE).
    async fn send_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        if let Some(ref transport) = self.transport {
            transport.request(method, params).await
        } else if let Some(ref sse) = self.sse_transport {
            sse.request(method, params).await
        } else {
            anyhow::bail!("Server not connected (no transport)")
        }
    }

    /// Execute a tool on this server.
    pub async fn call_tool(&self, tool_name: &str, arguments: serde_json::Value) -> Result<String> {
        let result = self
            .send_request(
                "tools/call",
                Some(serde_json::json!({
                    "name": tool_name,
                    "arguments": arguments,
                })),
            )
            .await?;

        // Extract content from result (may be text or structured)
        if let Some(content) = result["content"].as_array() {
            let mut output = String::new();
            for block in content {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(text) = block["text"].as_str() {
                            if !output.is_empty() {
                                output.push('\n');
                            }
                            output.push_str(text);
                        }
                    }
                    Some("image") | Some("resource") => {
                        output.push_str("[binary content omitted]");
                    }
                    _ => {}
                }
            }
            Ok(output)
        } else {
            // Fallback: stringify the result
            Ok(serde_json::to_string_pretty(&result)?)
        }
    }

    /// List resources available from this server.
    pub async fn list_resources(&self) -> Result<Vec<McpResource>> {
        let result = self.send_request("resources/list", None).await?;

        let resources = result["resources"].as_array().cloned().unwrap_or_default();

        let mut mcp_resources = Vec::new();
        for res in resources {
            let uri = res["uri"].as_str().unwrap_or("").to_string();
            let name = res["name"].as_str().unwrap_or(&uri).to_string();
            let description = res["description"].as_str().unwrap_or("").to_string();
            let mime_type = res["mimeType"].as_str().unwrap_or("text/plain").to_string();

            if uri.is_empty() {
                continue;
            }

            mcp_resources.push(McpResource {
                uri,
                name,
                description,
                mime_type,
                server_name: self.name.clone(),
            });
        }

        Ok(mcp_resources)
    }

    /// Read a resource by URI from this server.
    pub async fn read_resource(&self, uri: &str) -> Result<String> {
        let result = self
            .send_request(
                "resources/read",
                Some(serde_json::json!({
                    "uri": uri,
                })),
            )
            .await?;

        // Extract content from result
        if let Some(contents) = result["contents"].as_array() {
            let mut output = String::new();
            for content in contents {
                if let Some(text) = content["text"].as_str() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(text);
                } else if content["blob"].is_string() {
                    output.push_str("[binary resource content]");
                }
            }
            Ok(output)
        } else {
            Ok(serde_json::to_string_pretty(&result)?)
        }
    }
}

/// A resource exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: String,
    pub server_name: String,
}

/// Build a qualified tool name: `mcp__servername__toolname`.
/// Normalizes names by replacing non-alphanumeric chars with underscores.
fn build_tool_name(server_name: &str, tool_name: &str) -> String {
    let norm_server = normalize_name(server_name);
    let norm_tool = normalize_name(tool_name);
    format!("mcp__{norm_server}__{norm_tool}")
}

/// Normalize a name for use in tool identifiers.
fn normalize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Parse a qualified MCP tool name back into (server_name, tool_name).
pub fn parse_tool_name(qualified: &str) -> Option<(String, String)> {
    let stripped = qualified.strip_prefix("mcp__")?;
    let (server, tool) = stripped.split_once("__")?;

    Some((server.to_string(), tool.to_string()))
}

// ─── MCP Manager ──────────────────────────────────────────────────

/// Manages all MCP server connections and provides a unified tool interface.
pub struct McpManager {
    connections: HashMap<String, McpConnection>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            connections: HashMap::new(),
        }
    }

    /// Load config and connect to all configured MCP servers for a project.
    pub async fn connect_all(&mut self, project_path: &str) {
        let configs = load_mcp_configs(project_path);

        for (name, config) in configs {
            tracing::info!("Connecting to MCP server: {name}");
            match self.connect_server(&name, &config).await {
                Ok(()) => tracing::info!("MCP server '{name}' connected"),
                Err(e) => {
                    tracing::warn!("MCP server '{name}' failed: {e}");
                    self.connections.insert(
                        name.clone(),
                        McpConnection {
                            name,
                            state: ConnectionState::Failed(e.to_string()),
                            transport: None,
                            sse_transport: None,
                            tools: Vec::new(),
                        },
                    );
                }
            }
        }
    }

    /// Connect to a single MCP server.
    async fn connect_server(&mut self, name: &str, config: &McpServerConfig) -> Result<()> {
        match config {
            McpServerConfig::Stdio { command, args, env } => {
                let transport = StdioTransport::spawn(command, args, env).await?;

                // Initialize (MCP handshake)
                McpConnection::initialize(&transport).await?;

                // Discover tools
                let tools = McpConnection::discover_tools(&transport, name).await?;
                let tool_count = tools.len();

                self.connections.insert(
                    name.to_string(),
                    McpConnection {
                        name: name.to_string(),
                        state: ConnectionState::Connected,
                        transport: Some(transport),
                        sse_transport: None,
                        tools,
                    },
                );

                tracing::info!("MCP '{name}': discovered {tool_count} tools");
                Ok(())
            }
            McpServerConfig::Remote {
                transport_type,
                url,
                headers,
            } => {
                match transport_type.as_str() {
                    "sse" => {
                        // Headers are already expanded during config loading
                        let expanded_headers = headers.clone();

                        let sse = super::sse::SseTransport::connect(url, &expanded_headers).await?;

                        // Initialize
                        let init_result = sse
                            .request(
                                "initialize",
                                Some(serde_json::json!({
                                    "protocolVersion": "2024-11-05",
                                    "capabilities": {},
                                    "clientInfo": {
                                        "name": "one-cli",
                                        "version": env!("CARGO_PKG_VERSION")
                                    }
                                })),
                            )
                            .await?;

                        sse.notify("notifications/initialized", None).await?;

                        // Discover tools
                        let tools_result = sse.request("tools/list", None).await?;
                        let raw_tools = tools_result["tools"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        let mut tools = Vec::new();
                        for tool in raw_tools {
                            let tname = tool["name"].as_str().unwrap_or("").to_string();
                            if tname.is_empty() {
                                continue;
                            }
                            tools.push(McpTool {
                                qualified_name: build_tool_name(name, &tname),
                                description: tool["description"].as_str().unwrap_or("").to_string(),
                                input_schema: tool["inputSchema"].clone(),
                                server_name: name.to_string(),
                                name: tname,
                            });
                        }

                        let tool_count = tools.len();
                        self.connections.insert(
                            name.to_string(),
                            McpConnection {
                                name: name.to_string(),
                                state: ConnectionState::Connected,
                                transport: None,
                                sse_transport: Some(sse),
                                tools,
                            },
                        );

                        tracing::info!("MCP SSE '{name}': discovered {tool_count} tools");
                        let _ = &init_result; // used for handshake
                        Ok(())
                    }
                    _ => {
                        anyhow::bail!(
                            "MCP transport '{transport_type}' not supported. Use 'sse' for remote servers."
                        )
                    }
                }
            }
        }
    }

    /// Get all discovered tools across all connected servers.
    pub fn all_tools(&self) -> Vec<&McpTool> {
        self.connections
            .values()
            .filter(|c| c.state == ConnectionState::Connected)
            .flat_map(|c| &c.tools)
            .collect()
    }

    /// Get tool schemas for all discovered tools (for AI system prompt).
    pub fn tool_schemas(&self) -> Vec<serde_json::Value> {
        self.all_tools()
            .iter()
            .map(|t| t.to_tool_schema())
            .collect()
    }

    /// Execute an MCP tool by its qualified name.
    pub async fn call_tool(
        &self,
        qualified_name: &str,
        arguments: serde_json::Value,
    ) -> Result<String> {
        let (server_name, tool_name) = parse_tool_name(qualified_name)
            .context(format!("Invalid MCP tool name: {qualified_name}"))?;

        let connection = self
            .connections
            .get(&server_name)
            .context(format!("MCP server '{server_name}' not found"))?;

        if connection.state != ConnectionState::Connected {
            anyhow::bail!("MCP server '{server_name}' is not connected");
        }

        // Find the original tool name (before normalization)
        let original_name = connection
            .tools
            .iter()
            .find(|t| normalize_name(&t.name) == tool_name)
            .map(|t| t.name.clone())
            .unwrap_or(tool_name);

        connection.call_tool(&original_name, arguments).await
    }

    /// List all resources across all connected servers.
    pub async fn list_resources(&self) -> Vec<McpResource> {
        let mut resources = Vec::new();
        for conn in self.connections.values() {
            if conn.state == ConnectionState::Connected
                && let Ok(server_resources) = conn.list_resources().await
            {
                resources.extend(server_resources);
            }
        }
        resources
    }

    /// Read a resource by URI from the appropriate server.
    pub async fn read_resource(&self, uri: &str) -> Result<String> {
        // Find which server owns this URI by checking all connected servers
        for conn in self.connections.values() {
            if conn.state == ConnectionState::Connected
                && let Ok(resources) = conn.list_resources().await
                && resources.iter().any(|r| r.uri == uri)
            {
                return conn.read_resource(uri).await;
            }
        }
        anyhow::bail!("Resource not found: {uri}")
    }

    /// Check if a qualified tool name belongs to an MCP server.
    pub fn is_mcp_tool(name: &str) -> bool {
        name.starts_with("mcp__")
    }

    /// Get connection status for all servers.
    pub fn server_status(&self) -> Vec<(String, ConnectionState)> {
        self.connections
            .iter()
            .map(|(name, conn)| (name.clone(), conn.state.clone()))
            .collect()
    }

    /// Shut down all connected servers.
    pub async fn shutdown_all(&self) {
        for conn in self.connections.values() {
            if let Some(ref transport) = conn.transport {
                let _ = transport.shutdown().await;
            }
        }
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_tool_name() {
        assert_eq!(
            build_tool_name("filesystem", "read_file"),
            "mcp__filesystem__read_file"
        );
        assert_eq!(
            build_tool_name("my-server", "do.something"),
            "mcp__my_server__do_something"
        );
    }

    #[test]
    fn test_parse_tool_name() {
        let (server, tool) = parse_tool_name("mcp__filesystem__read_file").unwrap();
        assert_eq!(server, "filesystem");
        assert_eq!(tool, "read_file");
    }

    #[test]
    fn test_parse_tool_name_invalid() {
        assert!(parse_tool_name("not_mcp_tool").is_none());
        assert!(parse_tool_name("mcp__only_one_part").is_none());
    }

    #[test]
    fn test_is_mcp_tool() {
        assert!(McpManager::is_mcp_tool("mcp__server__tool"));
        assert!(!McpManager::is_mcp_tool("file_read"));
        assert!(!McpManager::is_mcp_tool("bash"));
    }

    #[test]
    fn test_tool_schema_format() {
        let tool = McpTool {
            name: "read_file".to_string(),
            qualified_name: "mcp__fs__read_file".to_string(),
            server_name: "fs".to_string(),
            description: "Read a file".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }),
        };

        let schema = tool.to_tool_schema();
        assert_eq!(schema["name"], "mcp__fs__read_file");
        assert!(
            schema["description"]
                .as_str()
                .unwrap()
                .contains("[MCP: fs]")
        );
    }
}

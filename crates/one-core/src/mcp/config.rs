//! MCP server configuration loading and parsing.
//!
//! Reads from:
//! 1. `.mcp.json` in the project directory (highest priority)
//! 2. `~/.one/settings.json` global config
//!
//! Supports env var expansion in config values: `${VAR_NAME}`.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Configuration for a single MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpServerConfig {
    /// Stdio transport: spawn a subprocess.
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    /// Remote transport (SSE, HTTP, WebSocket).
    Remote {
        #[serde(rename = "type")]
        transport_type: String,
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

impl McpServerConfig {
    /// Whether this is a stdio (local process) server.
    pub fn is_stdio(&self) -> bool {
        matches!(self, McpServerConfig::Stdio { .. })
    }
}

/// Top-level MCP config file format (`.mcp.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfigFile {
    #[serde(default, rename = "mcpServers")]
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

/// Load MCP server configs from all sources for a given project path.
pub fn load_mcp_configs(project_path: &str) -> HashMap<String, McpServerConfig> {
    let mut configs = HashMap::new();

    // 1. Global config: ~/.one/mcp.json
    if let Some(home) = dirs_next::home_dir() {
        let global_path = home.join(".one").join("mcp.json");
        if let Some(file_configs) = load_config_file(&global_path) {
            configs.extend(file_configs);
        }
    }

    // 2. Project config: <project>/.mcp.json (overrides global)
    let project_mcp = Path::new(project_path).join(".mcp.json");
    if let Some(file_configs) = load_config_file(&project_mcp) {
        configs.extend(file_configs);
    }

    // Expand env vars in all configs
    for config in configs.values_mut() {
        expand_env_vars(config);
    }

    configs
}

/// Load a single MCP config file.
fn load_config_file(path: &Path) -> Option<HashMap<String, McpServerConfig>> {
    let content = std::fs::read_to_string(path).ok()?;
    let parsed: McpConfigFile = serde_json::from_str(&content).ok()?;
    Some(parsed.mcp_servers)
}

/// Expand `${VAR_NAME}` patterns in config values.
fn expand_env_vars(config: &mut McpServerConfig) {
    match config {
        McpServerConfig::Stdio { args, env, command } => {
            *command = expand_env_string(command);
            for arg in args.iter_mut() {
                *arg = expand_env_string(arg);
            }
            for val in env.values_mut() {
                *val = expand_env_string(val);
            }
        }
        McpServerConfig::Remote { url, headers, .. } => {
            *url = expand_env_string(url);
            for val in headers.values_mut() {
                *val = expand_env_string(val);
            }
        }
    }
}

/// Replace `${VAR}` with environment variable values.
fn expand_env_string(s: &str) -> String {
    let mut result = s.to_string();
    // Find all ${...} patterns
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let replacement = std::env::var(var_name).unwrap_or_default();
            result = format!(
                "{}{}{}",
                &result[..start],
                replacement,
                &result[start + end + 1..]
            );
        } else {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stdio_config() {
        let json = r#"{
            "mcpServers": {
                "filesystem": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem"],
                    "env": {"LOG_LEVEL": "info"}
                }
            }
        }"#;
        let config: McpConfigFile = serde_json::from_str(json).unwrap();
        assert!(config.mcp_servers.contains_key("filesystem"));
        assert!(config.mcp_servers["filesystem"].is_stdio());
    }

    #[test]
    fn test_parse_remote_config() {
        let json = r#"{
            "mcpServers": {
                "remote": {
                    "type": "sse",
                    "url": "https://mcp.example.com/sse",
                    "headers": {"X-API-Key": "test"}
                }
            }
        }"#;
        let config: McpConfigFile = serde_json::from_str(json).unwrap();
        assert!(!config.mcp_servers["remote"].is_stdio());
    }

    #[test]
    fn test_expand_env_vars() {
        // SAFETY: test runs single-threaded, no concurrent env access
        unsafe { std::env::set_var("TEST_MCP_VAR", "expanded_value") };
        let result = expand_env_string("prefix-${TEST_MCP_VAR}-suffix");
        assert_eq!(result, "prefix-expanded_value-suffix");
        unsafe { std::env::remove_var("TEST_MCP_VAR") };
    }

    #[test]
    fn test_expand_missing_env_var() {
        let result = expand_env_string("${NONEXISTENT_VAR_12345}");
        assert_eq!(result, ""); // Missing vars expand to empty string
    }
}

//! Settings file loader for permission rules and other JSON-based config.
//!
//! Reads from (in priority order):
//! 1. CLI flags (highest priority)
//! 2. `.one/settings.local.json` in project dir (git-ignored)
//! 3. `.one/settings.json` in project dir (committed)
//! 4. `~/.one/settings.json` (global user settings)
//!
//! Mirrors Claude Code's settings.json schema.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::permissions::{PermissionEngine, PermissionMode, PermissionsConfig, RuleSource};

/// Full settings file schema.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub permissions: PermissionsConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
}

/// Hook configuration — shell commands that run on tool events.
/// Matches Claude Code's settings.json hook schema.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct HooksConfig {
    #[serde(default)]
    pub pre_tool_use: Vec<HookEntry>,
    #[serde(default)]
    pub post_tool_use: Vec<HookEntry>,
    #[serde(default)]
    pub post_response: Vec<HookEntry>,
    #[serde(default)]
    pub session_start: Vec<HookEntry>,
    /// Fires when the AI completes its response (for completion validation).
    #[serde(default)]
    pub stop: Vec<HookEntry>,
    /// Fires when the user submits a prompt.
    #[serde(default)]
    pub user_prompt_submit: Vec<HookEntry>,
}

/// A single hook entry — a shell command with optional matcher.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEntry {
    /// Shell command to execute.
    pub command: String,
    /// Optional matcher: only runs if the tool call matches this pattern.
    /// Uses permission rule syntax (e.g., "Bash(git *)", "Read(*.ts)").
    #[serde(rename = "if")]
    pub matcher: Option<String>,
    /// Timeout in seconds (default: 30).
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Execute hooks for an event. Returns combined output of all hooks.
pub async fn execute_hooks(
    hooks: &[HookEntry],
    tool_name: Option<&str>,
    tool_input: Option<&str>,
    working_dir: &str,
) -> Vec<(String, bool)> {
    let mut results = Vec::new();

    for hook in hooks {
        // Check matcher if present
        if let Some(ref matcher) = hook.matcher {
            let matches = match tool_name {
                Some(name) => {
                    // Support pipe-separated patterns: "Edit|Write|MultiEdit"
                    let patterns: Vec<&str> = matcher.split('|').collect();
                    patterns.iter().any(|pattern| {
                        let pattern = pattern.trim();
                        if pattern.contains('(') {
                            // Pattern like "Bash(git *)" — check name and input
                            let parts: Vec<&str> = pattern.splitn(2, '(').collect();
                            let hook_name = parts[0];
                            let hook_pattern = parts.get(1).map(|p| p.trim_end_matches(')'));

                            name == hook_name
                                && hook_pattern
                                    .map(|p| {
                                        let input = tool_input.unwrap_or("");
                                        if p.ends_with('*') {
                                            input.starts_with(p.trim_end_matches('*'))
                                        } else {
                                            input.contains(p)
                                        }
                                    })
                                    .unwrap_or(true)
                        } else {
                            name == pattern
                        }
                    })
                }
                None => true,
            };

            if !matches {
                continue;
            }
        }

        let timeout_secs = hook.timeout.unwrap_or(30);
        // Expand ${CLAUDE_PLUGIN_ROOT} in the command if present
        let command = hook.command.replace("${CLAUDE_PLUGIN_ROOT}", working_dir);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            tokio::process::Command::new("bash")
                .arg("-c")
                .arg(&command)
                .current_dir(working_dir)
                .env("TOOL_NAME", tool_name.unwrap_or(""))
                .env("TOOL_INPUT", tool_input.unwrap_or(""))
                .env("WORKING_DIR", working_dir)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let is_error = !output.status.success();
                results.push((stdout, is_error));
            }
            Ok(Err(e)) => {
                results.push((format!("Hook failed: {e}"), true));
            }
            Err(_) => {
                results.push((format!("Hook timed out after {timeout_secs}s"), true));
            }
        }
    }

    results
}

/// Load hooks from all settings files.
pub fn load_hooks(project_dir: &str) -> HooksConfig {
    let mut hooks = HooksConfig::default();

    // Global hooks
    if let Some(home) = dirs_next::home_dir() {
        let global_path = home.join(".one").join("settings.json");
        if let Some(settings) = load_settings_file(&global_path) {
            merge_hooks(&mut hooks, &settings.hooks);
        }
    }

    // Project hooks
    let project_path = PathBuf::from(project_dir)
        .join(".one")
        .join("settings.json");
    if let Some(settings) = load_settings_file(&project_path) {
        merge_hooks(&mut hooks, &settings.hooks);
    }

    // Local hooks
    let local_path = PathBuf::from(project_dir)
        .join(".one")
        .join("settings.local.json");
    if let Some(settings) = load_settings_file(&local_path) {
        merge_hooks(&mut hooks, &settings.hooks);
    }

    hooks
}

fn merge_hooks(target: &mut HooksConfig, source: &HooksConfig) {
    target.pre_tool_use.extend(source.pre_tool_use.clone());
    target.post_tool_use.extend(source.post_tool_use.clone());
    target.post_response.extend(source.post_response.clone());
    target.session_start.extend(source.session_start.clone());
    target.stop.extend(source.stop.clone());
    target
        .user_prompt_submit
        .extend(source.user_prompt_submit.clone());
}

/// Load settings from all sources and build a configured PermissionEngine.
pub fn load_permission_engine(project_dir: &str) -> PermissionEngine {
    let mut mode = PermissionMode::Default;
    let mut engine = PermissionEngine::new(mode);

    // 1. Global user settings: ~/.one/settings.json
    if let Some(home) = dirs_next::home_dir() {
        let global_path = home.join(".one").join("settings.json");
        if let Some(settings) = load_settings_file(&global_path) {
            if let Some(m) = settings.permissions.default_mode {
                mode = m;
                engine = PermissionEngine::new(mode);
            }
            engine.load_rules(&settings.permissions, RuleSource::UserSettings);
            tracing::debug!("Loaded global settings from {}", global_path.display());
        }
    }

    // 2. Project settings: <project>/.one/settings.json
    let project_settings = PathBuf::from(project_dir)
        .join(".one")
        .join("settings.json");
    if let Some(settings) = load_settings_file(&project_settings) {
        if let Some(m) = settings.permissions.default_mode {
            mode = m;
            engine = PermissionEngine::new(mode);
            // Re-load global rules with new mode
            if let Some(home) = dirs_next::home_dir() {
                let global_path = home.join(".one").join("settings.json");
                if let Some(global) = load_settings_file(&global_path) {
                    engine.load_rules(&global.permissions, RuleSource::UserSettings);
                }
            }
        }
        engine.load_rules(&settings.permissions, RuleSource::ProjectSettings);
        tracing::debug!(
            "Loaded project settings from {}",
            project_settings.display()
        );
    }

    // 3. Local settings: <project>/.one/settings.local.json (git-ignored)
    let local_settings = PathBuf::from(project_dir)
        .join(".one")
        .join("settings.local.json");
    if let Some(settings) = load_settings_file(&local_settings) {
        engine.load_rules(&settings.permissions, RuleSource::LocalSettings);
        tracing::debug!("Loaded local settings from {}", local_settings.display());
    }

    engine
}

/// Load a single settings file. Returns None if file doesn't exist or can't be parsed.
fn load_settings_file(path: &Path) -> Option<Settings> {
    let content = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Settings>(&content) {
        Ok(settings) => Some(settings),
        Err(e) => {
            tracing::warn!("Failed to parse {}: {e}", path.display());
            None
        }
    }
}

/// Save settings to a file (for writing back user preferences).
pub fn save_settings(path: &Path, settings: &Settings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(settings)?;
    std::fs::write(path, content)?;
    Ok(())
}

/// Get the global settings file path.
pub fn global_settings_path() -> PathBuf {
    dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".one")
        .join("settings.json")
}

/// Get the project settings file path.
pub fn project_settings_path(project_dir: &str) -> PathBuf {
    PathBuf::from(project_dir)
        .join(".one")
        .join("settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_settings() {
        let json = r#"{
            "permissions": {
                "allow": ["file_read", "grep", "glob", "bash(git:*)"],
                "deny": ["bash(rm -rf)"],
                "defaultMode": "default"
            }
        }"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.permissions.allow.len(), 4);
        assert_eq!(settings.permissions.deny.len(), 1);
        assert_eq!(
            settings.permissions.default_mode,
            Some(PermissionMode::Default)
        );
    }

    #[test]
    fn test_parse_empty_settings() {
        let json = "{}";
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert!(settings.permissions.allow.is_empty());
        assert!(settings.permissions.deny.is_empty());
    }

    #[test]
    fn test_parse_accept_edits_mode() {
        let json = r#"{"permissions": {"defaultMode": "acceptEdits"}}"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(
            settings.permissions.default_mode,
            Some(PermissionMode::AcceptEdits)
        );
    }

    #[test]
    fn test_parse_hooks() {
        let json = r#"{
            "hooks": {
                "PreToolUse": [
                    {"command": "echo pre", "if": "Bash"},
                    {"command": "lint.sh", "if": "Edit(*.rs)", "timeout": 10}
                ],
                "PostResponse": [
                    {"command": "notify-send done"}
                ]
            }
        }"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.hooks.pre_tool_use.len(), 2);
        assert_eq!(settings.hooks.post_response.len(), 1);
        assert_eq!(settings.hooks.pre_tool_use[0].command, "echo pre");
        assert_eq!(
            settings.hooks.pre_tool_use[0].matcher,
            Some("Bash".to_string())
        );
        assert_eq!(settings.hooks.pre_tool_use[1].timeout, Some(10));
    }

    #[test]
    fn test_hooks_empty_by_default() {
        let json = "{}";
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert!(settings.hooks.pre_tool_use.is_empty());
        assert!(settings.hooks.post_tool_use.is_empty());
        assert!(settings.hooks.post_response.is_empty());
        assert!(settings.hooks.session_start.is_empty());
        assert!(settings.hooks.stop.is_empty());
        assert!(settings.hooks.user_prompt_submit.is_empty());
    }

    #[tokio::test]
    async fn test_hook_pipe_matcher() {
        // Test that pipe-separated matchers work (CC format: "Edit|Write|MultiEdit")
        let hook = HookEntry {
            command: "echo matched".to_string(),
            matcher: Some("Edit|Write".to_string()),
            timeout: Some(5),
        };

        // Should match "Edit"
        let results = execute_hooks(std::slice::from_ref(&hook), Some("Edit"), None, "/tmp").await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].1); // not an error

        // Should match "Write"
        let results = execute_hooks(std::slice::from_ref(&hook), Some("Write"), None, "/tmp").await;
        assert_eq!(results.len(), 1);

        // Should NOT match "Bash"
        let results = execute_hooks(&[hook], Some("Bash"), None, "/tmp").await;
        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    async fn test_hook_plugin_root_expansion() {
        // ${CLAUDE_PLUGIN_ROOT} should be expanded to working_dir
        let hook = HookEntry {
            command: "echo ${CLAUDE_PLUGIN_ROOT}".to_string(),
            matcher: None,
            timeout: Some(5),
        };

        let results = execute_hooks(&[hook], None, None, "/tmp").await;
        assert_eq!(results.len(), 1);
        assert!(results[0].0.contains("/tmp"));
    }
}

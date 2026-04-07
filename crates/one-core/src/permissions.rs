//! Permission system for tool approval, dangerous command detection, and file safety.
//!
//! Mirrors Claude Code's permission architecture:
//! - Permission modes (default, acceptEdits, bypassPermissions, dontAsk, plan)
//! - Rule-based allow/deny/ask with source tracking
//! - Dangerous command detection (destructive git ops, rm -rf, etc.)
//! - Sensitive file protection (.env, credentials, shell configs)
//! - Pattern matching: `ToolName` or `ToolName(pattern)`

use serde::{Deserialize, Serialize};

// ─── Permission Mode ───────────────────────────────────────────────

/// Controls the default permission behavior for the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    /// Prompt user for confirmation on non-read-only tools (default).
    #[default]
    Default,
    /// Auto-approve file edits but still ask for bash/shell commands.
    AcceptEdits,
    /// Skip all permission checks (dangerous — for trusted environments).
    BypassPermissions,
    /// Auto-deny all tool use (read-only mode).
    DontAsk,
    /// Plan mode — tools are described but not executed.
    Plan,
}

// ─── Permission Behavior ───────────────────────────────────────────

/// The action to take for a given tool use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionBehavior {
    Allow,
    Deny,
    Ask,
}

// ─── Rule Source ───────────────────────────────────────────────────

/// Where a permission rule originated (highest to lowest priority).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase")]
pub enum RuleSource {
    /// CLI arguments (highest priority).
    CliArg,
    /// Per-session temporary rules.
    Session,
    /// Managed/enterprise policy settings (read-only).
    PolicySettings,
    /// User global settings (~/.one/settings.json).
    UserSettings,
    /// Project settings (./.one/settings.json in repo root).
    ProjectSettings,
    /// Local settings (./.one/settings.local.json, git-ignored).
    LocalSettings,
}

// ─── Permission Rule ──────────────────────────────────────────────

/// A single permission rule: tool name + optional pattern.
///
/// Format: `ToolName` or `ToolName(pattern)`
/// Examples:
/// - `bash` — matches all bash commands
/// - `bash(git commit)` — matches bash commands containing "git commit"
/// - `file_edit(/src/**/*)` — matches edits to files under /src/
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub source: RuleSource,
    pub behavior: PermissionBehavior,
    pub tool_name: String,
    pub pattern: Option<String>,
}

impl PermissionRule {
    /// Parse a rule string like "bash" or "bash(git commit:*)" into components.
    pub fn parse(rule_str: &str, source: RuleSource, behavior: PermissionBehavior) -> Self {
        if let Some(paren_start) = rule_str.find('(')
            && rule_str.ends_with(')')
        {
            let tool_name = rule_str[..paren_start].to_string();
            let pattern = rule_str[paren_start + 1..rule_str.len() - 1].to_string();
            return Self {
                source,
                behavior,
                tool_name,
                pattern: Some(pattern),
            };
        }
        Self {
            source,
            behavior,
            tool_name: rule_str.to_string(),
            pattern: None,
        }
    }

    /// Check if this rule matches a given tool name and input context.
    pub fn matches(&self, tool_name: &str, input_context: &str) -> bool {
        if !self.tool_name.eq_ignore_ascii_case(tool_name) {
            return false;
        }
        match &self.pattern {
            None => true, // No pattern = matches all uses of this tool
            Some(pat) => {
                if pat.ends_with(":*") || pat.ends_with("*") {
                    // Prefix match: "git:*" matches "git commit", "git push", etc.
                    let prefix = pat.trim_end_matches(":*").trim_end_matches('*');
                    input_context
                        .to_lowercase()
                        .contains(&prefix.to_lowercase())
                } else {
                    // Exact substring match
                    input_context.to_lowercase().contains(&pat.to_lowercase())
                }
            }
        }
    }
}

// ─── Permission Decision ──────────────────────────────────────────

/// Result of evaluating permissions for a tool use.
#[derive(Debug, Clone)]
pub struct PermissionDecision {
    pub behavior: PermissionBehavior,
    pub reason: String,
    pub matched_rule: Option<PermissionRule>,
    pub warning: Option<String>,
}

// ─── Permissions Config ───────────────────────────────────────────

/// Permission rules loaded from settings files.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionsConfig {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default)]
    pub ask: Vec<String>,
    #[serde(default, rename = "defaultMode")]
    pub default_mode: Option<PermissionMode>,
}

// ─── Permission Engine ────────────────────────────────────────────

/// The main permission evaluation engine.
pub struct PermissionEngine {
    pub mode: PermissionMode,
    rules: Vec<PermissionRule>,
    read_only_tools: Vec<String>,
}

impl PermissionEngine {
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            rules: Vec::new(),
            read_only_tools: vec![
                "file_read".to_string(),
                "grep".to_string(),
                "glob".to_string(),
            ],
        }
    }

    /// Load rules from a PermissionsConfig with the given source.
    pub fn load_rules(&mut self, config: &PermissionsConfig, source: RuleSource) {
        for rule_str in &config.allow {
            self.rules.push(PermissionRule::parse(
                rule_str,
                source,
                PermissionBehavior::Allow,
            ));
        }
        for rule_str in &config.deny {
            self.rules.push(PermissionRule::parse(
                rule_str,
                source,
                PermissionBehavior::Deny,
            ));
        }
        for rule_str in &config.ask {
            self.rules.push(PermissionRule::parse(
                rule_str,
                source,
                PermissionBehavior::Ask,
            ));
        }
    }

    /// Add a single session-scoped rule (e.g., from user granting permission at a prompt).
    pub fn add_session_rule(
        &mut self,
        tool_name: &str,
        pattern: Option<&str>,
        behavior: PermissionBehavior,
    ) {
        self.rules.push(PermissionRule {
            source: RuleSource::Session,
            behavior,
            tool_name: tool_name.to_string(),
            pattern: pattern.map(String::from),
        });
    }

    /// Evaluate whether a tool use should be allowed, denied, or prompted.
    pub fn check(&self, tool_name: &str, input: &serde_json::Value) -> PermissionDecision {
        let input_context = Self::extract_input_context(tool_name, input);

        // Check for dangerous command warnings (informational, doesn't block)
        let warning = if tool_name == "bash" {
            detect_dangerous_command(&input_context)
        } else {
            detect_sensitive_file(tool_name, input)
        };

        // 1. Check explicit rules (highest priority source wins)
        // Rules are evaluated in insertion order; first match wins.
        // Since rules are loaded source-by-source (CLI > session > policy > user > project > local),
        // higher-priority sources naturally come first.
        for rule in &self.rules {
            if rule.matches(tool_name, &input_context) {
                return PermissionDecision {
                    behavior: rule.behavior,
                    reason: format!(
                        "Matched {} rule from {:?}: {}{}",
                        match rule.behavior {
                            PermissionBehavior::Allow => "allow",
                            PermissionBehavior::Deny => "deny",
                            PermissionBehavior::Ask => "ask",
                        },
                        rule.source,
                        rule.tool_name,
                        rule.pattern
                            .as_ref()
                            .map(|p| format!("({})", p))
                            .unwrap_or_default()
                    ),
                    matched_rule: Some(rule.clone()),
                    warning,
                };
            }
        }

        // 2. Apply mode-based defaults
        let behavior = match self.mode {
            PermissionMode::BypassPermissions => PermissionBehavior::Allow,
            PermissionMode::DontAsk | PermissionMode::Plan => PermissionBehavior::Deny,
            PermissionMode::AcceptEdits => {
                if tool_name == "file_edit"
                    || tool_name == "file_write"
                    || self.read_only_tools.iter().any(|t| t == tool_name)
                {
                    PermissionBehavior::Allow
                } else {
                    PermissionBehavior::Ask
                }
            }
            PermissionMode::Default => {
                if self.read_only_tools.iter().any(|t| t == tool_name) {
                    PermissionBehavior::Allow
                } else {
                    PermissionBehavior::Ask
                }
            }
        };

        PermissionDecision {
            behavior,
            reason: format!("Default for {:?} mode", self.mode),
            matched_rule: None,
            warning,
        }
    }

    /// Extract a human-readable context string from tool input for pattern matching.
    pub fn extract_input_context(tool_name: &str, input: &serde_json::Value) -> String {
        match tool_name {
            "Bash" | "bash" => input["command"].as_str().unwrap_or("").to_string(),
            "Read" | "file_read" => input["file_path"].as_str().unwrap_or("").to_string(),
            "Write" | "file_write" => {
                let path = input["file_path"].as_str().unwrap_or("");
                let content = input["content"].as_str().unwrap_or("");
                let lines = content.lines().count();
                format!("{path} ({lines} lines)")
            }
            "Edit" | "file_edit" => {
                let path = input["file_path"].as_str().unwrap_or("");
                let old = input["old_string"].as_str().unwrap_or("");
                let preview = if old.len() > 40 {
                    format!("{}...", &old[..40])
                } else {
                    old.to_string()
                };
                format!("{path}: \"{preview}\"")
            }
            "Grep" | "grep" => {
                let pattern = input["pattern"].as_str().unwrap_or("");
                let path = input["path"].as_str().unwrap_or(".");
                format!("{pattern} in {path}")
            }
            "Glob" | "glob" => input["pattern"].as_str().unwrap_or("").to_string(),
            "Agent" | "agent" => {
                let desc = input["description"].as_str().unwrap_or("");
                let agent_type = input["subagent_type"].as_str().unwrap_or("general");
                format!("{agent_type}: {desc}")
            }
            "WebSearch" | "web_search" => input["query"].as_str().unwrap_or("").to_string(),
            "WebFetch" | "web_fetch" => input["url"].as_str().unwrap_or("").to_string(),
            _ => {
                // Fallback: show a compact summary of known fields
                let mut parts = Vec::new();
                if let Some(path) = input["file_path"].as_str() {
                    parts.push(path.to_string());
                }
                if let Some(cmd) = input["command"].as_str() {
                    parts.push(cmd.to_string());
                }
                if parts.is_empty() {
                    let json = serde_json::to_string(input).unwrap_or_default();
                    if json.len() > 80 {
                        format!("{}...", &json[..80])
                    } else {
                        json
                    }
                } else {
                    parts.join(" ")
                }
            }
        }
    }
}

// ─── Dangerous Command Detection ──────────────────────────────────

/// Destructive git operations that warrant a warning.
const DANGEROUS_GIT_PATTERNS: &[(&str, &str)] = &[
    ("git reset --hard", "discards all uncommitted changes"),
    ("git push --force", "overwrites remote history"),
    ("git push -f", "overwrites remote history"),
    ("git clean -f", "permanently deletes untracked files"),
    (
        "git checkout .",
        "discards all changes in working directory",
    ),
    (
        "git checkout -- .",
        "discards all changes in working directory",
    ),
    ("git restore .", "discards all changes in working directory"),
    ("git stash drop", "removes stashed changes"),
    ("git stash clear", "removes all stashed changes"),
    ("git branch -D", "force-deletes a branch"),
    ("git commit --no-verify", "skips pre-commit hooks"),
    ("git push --no-verify", "skips pre-push hooks"),
    ("git commit --amend", "rewrites the last commit"),
    ("git rebase -i", "rewrites commit history interactively"),
];

/// Destructive shell commands.
const DANGEROUS_SHELL_PATTERNS: &[(&str, &str)] = &[
    ("rm -rf /", "deletes entire filesystem"),
    ("rm -rf ~", "deletes home directory"),
    ("rm -rf *", "deletes everything in current directory"),
    ("rm -rf .", "deletes current directory contents"),
    ("DROP TABLE", "deletes database table"),
    ("DELETE FROM", "deletes database records"),
    ("kubectl delete", "deletes Kubernetes resources"),
    ("terraform destroy", "destroys infrastructure"),
    ("docker system prune -a", "removes all Docker resources"),
];

/// Check if a bash command contains dangerous patterns.
/// Returns a warning message if found, None otherwise.
pub fn detect_dangerous_command(command: &str) -> Option<String> {
    let lower = command.to_lowercase();

    for (pattern, description) in DANGEROUS_GIT_PATTERNS {
        if lower.contains(&pattern.to_lowercase()) {
            return Some(format!(
                "Warning: `{}` — {}. This operation may be irreversible.",
                pattern, description
            ));
        }
    }

    for (pattern, description) in DANGEROUS_SHELL_PATTERNS {
        if lower.contains(&pattern.to_lowercase()) {
            return Some(format!(
                "Warning: `{}` — {}. This operation may be irreversible.",
                pattern, description
            ));
        }
    }

    None
}

// ─── Sensitive File Detection ─────────────────────────────────────

/// Files that contain credentials or sensitive configuration.
const SENSITIVE_FILES: &[&str] = &[
    ".env",
    ".env.local",
    ".env.production",
    ".env.development",
    "credentials.json",
    "service-account.json",
    ".npmrc",
    ".pypirc",
    ".netrc",
    ".docker/config.json",
];

/// Directories that are protected from modification.
const PROTECTED_DIRS: &[&str] = &[".git", ".vscode", ".idea", ".claude", ".one"];

/// Shell config files that shouldn't be modified without warning.
const SHELL_CONFIGS: &[&str] = &[
    ".bashrc",
    ".bash_profile",
    ".zshrc",
    ".zprofile",
    ".profile",
    ".gitconfig",
    ".gitmodules",
    ".ssh/config",
    ".ripgreprc",
];

/// Check if a tool operation targets a sensitive file.
/// Returns a warning message if detected.
pub fn detect_sensitive_file(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    // Only check write/edit operations
    if tool_name != "file_write" && tool_name != "file_edit" {
        return None;
    }

    let file_path = input["file_path"].as_str()?;
    let path = std::path::Path::new(file_path);

    // Check for path traversal attacks
    let path_str = file_path.to_lowercase();
    if path_str.contains("../") || path_str.contains("..\\") {
        return Some(
            "Warning: path contains traversal (`../`). Verify the target is intentional."
                .to_string(),
        );
    }

    // Check filename against sensitive files
    if let Some(file_name) = path.file_name().and_then(|f| f.to_str()) {
        let lower_name = file_name.to_lowercase();
        for sensitive in SENSITIVE_FILES {
            if lower_name == sensitive.to_lowercase() {
                return Some(format!(
                    "Warning: `{}` may contain credentials or secrets. Verify this write is safe.",
                    file_name
                ));
            }
        }
        for config in SHELL_CONFIGS {
            if lower_name == config.to_lowercase() {
                return Some(format!(
                    "Warning: `{}` is a shell configuration file. Changes may affect your environment.",
                    file_name
                ));
            }
        }
    }

    // Check if any path component is a protected directory
    for component in path.components() {
        if let std::path::Component::Normal(name) = component
            && let Some(name_str) = name.to_str()
        {
            let lower = name_str.to_lowercase();
            for protected in PROTECTED_DIRS {
                if lower == protected.to_lowercase() {
                    return Some(format!(
                        "Warning: modifying files in `{}` directory. This may affect tool configuration.",
                        protected
                    ));
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_rule_parse_simple() {
        let rule =
            PermissionRule::parse("bash", RuleSource::UserSettings, PermissionBehavior::Allow);
        assert_eq!(rule.tool_name, "bash");
        assert!(rule.pattern.is_none());
    }

    #[test]
    fn test_permission_rule_parse_with_pattern() {
        let rule = PermissionRule::parse(
            "bash(git commit)",
            RuleSource::Session,
            PermissionBehavior::Allow,
        );
        assert_eq!(rule.tool_name, "bash");
        assert_eq!(rule.pattern.as_deref(), Some("git commit"));
    }

    #[test]
    fn test_rule_matches_simple() {
        let rule =
            PermissionRule::parse("bash", RuleSource::UserSettings, PermissionBehavior::Allow);
        assert!(rule.matches("bash", "git commit -m 'test'"));
        assert!(!rule.matches("file_read", "anything"));
    }

    #[test]
    fn test_rule_matches_pattern() {
        let rule = PermissionRule::parse(
            "bash(git commit)",
            RuleSource::Session,
            PermissionBehavior::Allow,
        );
        assert!(rule.matches("bash", "git commit -m 'test'"));
        assert!(!rule.matches("bash", "rm -rf /"));
    }

    #[test]
    fn test_rule_matches_wildcard() {
        let rule = PermissionRule::parse(
            "bash(git:*)",
            RuleSource::UserSettings,
            PermissionBehavior::Allow,
        );
        assert!(rule.matches("bash", "git push origin main"));
        assert!(rule.matches("bash", "git commit -m 'test'"));
        assert!(!rule.matches("bash", "npm install"));
    }

    #[test]
    fn test_dangerous_git_command() {
        assert!(detect_dangerous_command("git reset --hard HEAD~1").is_some());
        assert!(detect_dangerous_command("git push --force origin main").is_some());
        assert!(detect_dangerous_command("git push -f").is_some());
        assert!(detect_dangerous_command("git clean -fd").is_some());
        assert!(detect_dangerous_command("git commit --amend").is_some());
        assert!(detect_dangerous_command("git commit -m 'safe'").is_none());
        assert!(detect_dangerous_command("git push origin main").is_none());
    }

    #[test]
    fn test_dangerous_shell_command() {
        assert!(detect_dangerous_command("rm -rf /").is_some());
        assert!(detect_dangerous_command("rm -rf ~").is_some());
        assert!(detect_dangerous_command("DROP TABLE users").is_some());
        assert!(detect_dangerous_command("kubectl delete pod my-pod").is_some());
        assert!(detect_dangerous_command("ls -la").is_none());
    }

    #[test]
    fn test_sensitive_file_detection() {
        let input = serde_json::json!({"file_path": "/home/user/.env"});
        assert!(detect_sensitive_file("file_write", &input).is_some());
        assert!(detect_sensitive_file("file_read", &input).is_none()); // reads are fine

        let input = serde_json::json!({"file_path": "/project/.git/config"});
        assert!(detect_sensitive_file("file_edit", &input).is_some());

        let input = serde_json::json!({"file_path": "/home/user/.bashrc"});
        assert!(detect_sensitive_file("file_write", &input).is_some());
    }

    #[test]
    fn test_path_traversal_detection() {
        let input = serde_json::json!({"file_path": "/project/../../../etc/passwd"});
        assert!(detect_sensitive_file("file_write", &input).is_some());
    }

    #[test]
    fn test_permission_engine_default_mode() {
        let engine = PermissionEngine::new(PermissionMode::Default);

        // Read-only tools always allowed
        let read_input = serde_json::json!({"file_path": "/test.txt"});
        let decision = engine.check("file_read", &read_input);
        assert_eq!(decision.behavior, PermissionBehavior::Allow);

        // Write tools require asking
        let write_input = serde_json::json!({"file_path": "/test.txt", "content": "hello"});
        let decision = engine.check("file_write", &write_input);
        assert_eq!(decision.behavior, PermissionBehavior::Ask);

        // Bash requires asking
        let bash_input = serde_json::json!({"command": "ls"});
        let decision = engine.check("bash", &bash_input);
        assert_eq!(decision.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn test_permission_engine_accept_edits_mode() {
        let engine = PermissionEngine::new(PermissionMode::AcceptEdits);

        // Edits auto-approved
        let edit_input =
            serde_json::json!({"file_path": "/test.txt", "old_string": "a", "new_string": "b"});
        let decision = engine.check("file_edit", &edit_input);
        assert_eq!(decision.behavior, PermissionBehavior::Allow);

        // Bash still requires asking
        let bash_input = serde_json::json!({"command": "npm install"});
        let decision = engine.check("bash", &bash_input);
        assert_eq!(decision.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn test_permission_engine_bypass_mode() {
        let engine = PermissionEngine::new(PermissionMode::BypassPermissions);

        let bash_input = serde_json::json!({"command": "rm -rf /"});
        let decision = engine.check("bash", &bash_input);
        assert_eq!(decision.behavior, PermissionBehavior::Allow);
        assert!(decision.warning.is_some()); // Warning still present even in bypass
    }

    #[test]
    fn test_explicit_rules_override_mode() {
        let mut engine = PermissionEngine::new(PermissionMode::Default);

        // Add allow rule for git commands
        engine.add_session_rule("bash", Some("git:*"), PermissionBehavior::Allow);

        let git_input = serde_json::json!({"command": "git status"});
        let decision = engine.check("bash", &git_input);
        assert_eq!(decision.behavior, PermissionBehavior::Allow);

        // Non-git bash still asks
        let npm_input = serde_json::json!({"command": "npm install"});
        let decision = engine.check("bash", &npm_input);
        assert_eq!(decision.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn test_deny_rules_block() {
        let mut engine = PermissionEngine::new(PermissionMode::BypassPermissions);

        // Even in bypass mode, explicit deny rules should block
        engine.add_session_rule("bash", Some("rm -rf"), PermissionBehavior::Deny);

        let dangerous_input = serde_json::json!({"command": "rm -rf /"});
        let decision = engine.check("bash", &dangerous_input);
        assert_eq!(decision.behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn test_dangerous_command_warning_in_decision() {
        let engine = PermissionEngine::new(PermissionMode::Default);

        let input = serde_json::json!({"command": "git push --force origin main"});
        let decision = engine.check("bash", &input);
        assert!(decision.warning.is_some());
        assert!(
            decision
                .warning
                .unwrap()
                .contains("overwrites remote history")
        );
    }
}

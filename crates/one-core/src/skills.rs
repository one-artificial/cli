//! User-installable skills — custom slash commands from markdown files.
//!
//! Skills are loaded from:
//! 1. ~/.one/commands/*.md (user-level)
//! 2. .one/commands/*.md (project-level)
//! 3. .claude/commands/*.md (CC-compatible)
//!
//! Each .md file becomes a slash command. The filename (without extension)
//! is the command name. The file content is the prompt template.
//!
//! Frontmatter (optional):
//! ```text
//! ---
//! description: Short description for autocomplete
//! allowed-tools: Bash(git add:*), Bash(git commit:*)
//! ---
//! ```
//!
//! ## Dynamic context interpolation
//!
//! Skills support `!` followed by a backtick-wrapped command that gets
//! executed at invocation time, with stdout substituted inline:
//!
//! ```text
//! Current branch: !`git branch --show-current`
//! ```
//!
//! ## Arguments
//!
//! `$ARGUMENTS` in the prompt body is replaced with any text the user
//! provides after the slash command name (e.g., `/review PR#123`).

use std::path::{Path, PathBuf};

/// A loaded custom skill (slash command).
#[derive(Debug, Clone)]
pub struct Skill {
    /// Command name (e.g., "review" → /review)
    pub name: String,
    /// Short description for autocomplete/help
    pub description: String,
    /// The prompt template (before interpolation)
    pub prompt: String,
    /// Source path of the skill file
    pub source: PathBuf,
    /// Tool restrictions (empty = all tools allowed).
    /// Patterns like `Bash(git add:*)`, `file_read`, etc.
    pub allowed_tools: Vec<String>,
    /// Hint text for arguments (shown in autocomplete/help).
    pub argument_hint: Option<String>,
}

/// Load all custom skills, searched in priority order (highest priority last, wins on conflict):
///
/// 1. Profile — `~/.one/commands/` and `~/.claude/commands/`
/// 2. Git root — `<git-root>/.one/commands/` and `<git-root>/.claude/commands/`
///    (skipped when git root == project_dir; enables monorepo shared skills)
/// 3. Project — `<project_dir>/.one/commands/` and `<project_dir>/.claude/commands/`
///
/// When the same skill name appears at multiple levels, the more specific level wins
/// (project > git-root > profile).
pub fn load_skills(project_dir: &str) -> Vec<Skill> {
    // Collect in priority order: profile first (lowest), project last (highest).
    // Within each tier: platform-specific tools first, open/One-native last.
    // Dedup keeps the last occurrence so more-specific and open-standard entries win.
    // Priority: One > CC > Gemini (open standards outrank platform standards).
    let mut all: Vec<Skill> = Vec::new();

    // 1. Profile-level — platform tools first, then One (wins)
    if let Some(home) = dirs_next::home_dir() {
        all.extend(load_skills_from_dir(&home.join(".gemini").join("commands")));
        all.extend(load_skills_from_dir(&home.join(".claude").join("commands")));
        all.extend(load_skills_from_dir(&home.join(".one").join("commands")));
    }

    // 2. Git-root level — platform tools first, then One (wins)
    let project_path = PathBuf::from(project_dir);
    if let Some(git_root) = find_git_root(&project_path)
        && git_root != project_path
    {
        all.extend(load_skills_from_dir(
            &git_root.join(".gemini").join("commands"),
        ));
        all.extend(load_skills_from_dir(
            &git_root.join(".claude").join("commands"),
        ));
        all.extend(load_skills_from_dir(
            &git_root.join(".one").join("commands"),
        ));
    }

    // 3. Project-level — platform tools first, then One (wins, highest priority)
    all.extend(load_skills_from_dir(
        &project_path.join(".gemini").join("commands"),
    ));
    all.extend(load_skills_from_dir(
        &project_path.join(".claude").join("commands"),
    ));
    all.extend(load_skills_from_dir(
        &project_path.join(".one").join("commands"),
    ));

    // Dedup: iterate in reverse so the last-added (highest-priority) occurrence wins.
    let mut seen = std::collections::HashSet::new();
    all.reverse();
    all.retain(|s| seen.insert(s.name.clone()));
    all.reverse();

    all
}

/// Walk up from `start` to find the nearest directory containing `.git`.
/// Returns `None` if no git repository is found before the filesystem root.
fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Load skills from a directory, including subdirectories.
/// Supports both flat structure (commands/*.md) and plugin structure
/// (plugins/*/commands/*.md).
fn load_skills_from_dir(dir: &Path) -> Vec<Skill> {
    let mut skills = Vec::new();

    if !dir.is_dir() {
        return skills;
    }

    collect_skills_recursive(dir, &mut skills);
    skills
}

/// Recursively collect .md skill files from a directory tree.
fn collect_skills_recursive(dir: &Path, skills: &mut Vec<Skill>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            // Recurse into subdirectories (supports plugin/commands/ structure)
            collect_skills_recursive(&path, skills);
            continue;
        }

        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        // Skip README.md files in plugin directories
        if path.file_name().and_then(|n| n.to_str()) == Some("README.md") {
            continue;
        }

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        if name.is_empty() {
            continue;
        }

        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };

        let parsed = parse_skill_file(&content, &name);

        skills.push(Skill {
            name,
            description: parsed.description,
            prompt: parsed.prompt,
            source: path,
            allowed_tools: parsed.allowed_tools,
            argument_hint: parsed.argument_hint,
        });
    }
}

/// Parsed frontmatter from a skill file.
struct SkillFrontmatter {
    description: String,
    allowed_tools: Vec<String>,
    argument_hint: Option<String>,
    prompt: String,
}

/// Parse a skill file: extract optional frontmatter and the prompt body.
fn parse_skill_file(content: &str, default_name: &str) -> SkillFrontmatter {
    let trimmed = content.trim();

    if let Some(after_start) = trimmed.strip_prefix("---") {
        // Has frontmatter
        if let Some(end) = after_start.find("---") {
            let frontmatter = &after_start[..end];
            let body = after_start[end + 3..].trim();

            let description = frontmatter
                .lines()
                .find_map(|line| {
                    let line = line.trim();
                    line.strip_prefix("description:")
                        .map(|d| d.trim().trim_matches('"').trim_matches('\'').to_string())
                })
                .unwrap_or_else(|| format!("Custom command: /{default_name}"));

            let allowed_tools = frontmatter
                .lines()
                .find_map(|line| {
                    let line = line.trim();
                    line.strip_prefix("allowed-tools:").map(|tools| {
                        let tools = tools.trim();
                        // Handle JSON array format: ["Read", "Write", ...]
                        if tools.starts_with('[') {
                            parse_json_string_array(tools)
                        } else {
                            // Comma-separated format: Bash(git add:*), file_read
                            tools
                                .split(',')
                                .map(|t| t.trim().to_string())
                                .filter(|t| !t.is_empty())
                                .collect::<Vec<_>>()
                        }
                    })
                })
                .unwrap_or_default();

            let argument_hint = frontmatter
                .lines()
                .find_map(|line| {
                    let line = line.trim();
                    line.strip_prefix("argument-hint:")
                        .map(|h| h.trim().trim_matches('"').trim_matches('\'').to_string())
                })
                .filter(|h| !h.is_empty());

            return SkillFrontmatter {
                description,
                allowed_tools,
                argument_hint,
                prompt: body.to_string(),
            };
        }
    }

    // No frontmatter — entire content is the prompt
    SkillFrontmatter {
        description: format!("Custom command: /{default_name}"),
        allowed_tools: Vec::new(),
        argument_hint: None,
        prompt: trimmed.to_string(),
    }
}

/// Parse a JSON-like string array: `["Read", "Write", "Bash(git:*)"]`
fn parse_json_string_array(s: &str) -> Vec<String> {
    // Simple parser for ["a", "b", "c"] — avoids pulling in serde for frontmatter
    let inner = s.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|t| {
            t.trim()
                .trim_matches('"')
                .trim_matches('\'')
                .trim()
                .to_string()
        })
        .filter(|t| !t.is_empty())
        .collect()
}

/// Expand `!`command`` patterns in a skill prompt.
///
/// Each `!`command`` is replaced with the command's stdout (trimmed).
/// If the command fails, the pattern is replaced with `<error: ...>`.
///
/// This runs synchronously (blocking) since skills are loaded at command
/// invocation time, not at startup.
pub fn interpolate_commands(prompt: &str, working_dir: &str) -> String {
    let mut result = String::with_capacity(prompt.len());
    let mut chars = prompt.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '!' && chars.peek() == Some(&'`') {
            // Consume the opening backtick
            chars.next();
            // Collect until closing backtick
            let mut cmd = String::new();
            let mut found_close = false;
            for ch in chars.by_ref() {
                if ch == '`' {
                    found_close = true;
                    break;
                }
                cmd.push(ch);
            }
            if found_close && !cmd.is_empty() {
                // Execute the command
                match std::process::Command::new("sh")
                    .args(["-c", &cmd])
                    .current_dir(working_dir)
                    .output()
                {
                    Ok(output) if output.status.success() => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        result.push_str(stdout.trim());
                    }
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        result.push_str(&format!("<error: {}>", stderr.trim()));
                    }
                    Err(e) => {
                        result.push_str(&format!("<error: {e}>"));
                    }
                }
            } else {
                // Malformed — output as-is
                result.push('!');
                result.push('`');
                result.push_str(&cmd);
                if !found_close {
                    // unclosed backtick
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Substitute `$ARGUMENTS` in a skill prompt with the user-provided arguments.
pub fn substitute_arguments(prompt: &str, arguments: &str) -> String {
    prompt.replace("$ARGUMENTS", arguments)
}

/// Prepare a skill prompt for execution: substitute arguments, then
/// interpolate shell commands.
pub fn prepare_skill_prompt(skill: &Skill, arguments: &str, working_dir: &str) -> String {
    let with_args = substitute_arguments(&skill.prompt, arguments);
    interpolate_commands(&with_args, working_dir)
}

/// Expand `@path` file references in a user message.
///
/// When a user types `@path/to/file.rs` or `@~/file.txt`, the file contents
/// are read and appended as context blocks at the end of the message.
/// Inspired by the @ mention pattern in modern coding tools.
///
/// Rules:
/// - `@` must be at start of message or preceded by whitespace
/// - The path extends until the next whitespace character
/// - Relative paths are resolved against `working_dir`
/// - `~` is expanded to the home directory
/// - If the file doesn't exist, the `@path` is left unchanged
/// - Files over 100KB are truncated with a note
pub fn expand_at_mentions(message: &str, working_dir: &str) -> String {
    let mut result_text = String::new();
    let mut file_contents: Vec<(String, String)> = Vec::new();

    let words: Vec<&str> = message.split_whitespace().collect();
    let mut first = true;

    for word in &words {
        if !first {
            result_text.push(' ');
        }
        first = false;

        if let Some(path_str) = word.strip_prefix('@') {
            if path_str.is_empty() {
                result_text.push_str(word);
                continue;
            }

            // Expand ~ to home dir
            let expanded = if let Some(rest) = path_str.strip_prefix('~') {
                if let Some(home) = dirs_next::home_dir() {
                    let rest = rest.strip_prefix('/').unwrap_or(rest);
                    home.join(rest)
                } else {
                    std::path::PathBuf::from(path_str)
                }
            } else if std::path::Path::new(path_str).is_absolute() {
                std::path::PathBuf::from(path_str)
            } else {
                std::path::PathBuf::from(working_dir).join(path_str)
            };

            if expanded.is_file() {
                // Read the file
                match std::fs::read_to_string(&expanded) {
                    Ok(content) => {
                        let display_path = path_str.to_string();
                        let truncated = if content.len() > 100_000 {
                            format!(
                                "{}\n\n... (truncated, file is {} bytes)",
                                &content[..100_000],
                                content.len()
                            )
                        } else {
                            content
                        };
                        file_contents.push((display_path, truncated));
                        // Keep the @reference in text for readability
                        result_text.push_str(word);
                    }
                    Err(_) => {
                        result_text.push_str(word);
                    }
                }
            } else {
                // Not a file — leave as-is
                result_text.push_str(word);
            }
        } else {
            result_text.push_str(word);
        }
    }

    // Append file contents as context blocks
    if !file_contents.is_empty() {
        result_text.push_str("\n\n---\n\n## Referenced Files\n");
        for (path, content) in &file_contents {
            // Detect extension for syntax highlighting
            let ext = std::path::Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            result_text.push_str(&format!("\n### {path}\n```{ext}\n{content}\n```\n"));
        }
    }

    result_text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_with_frontmatter() {
        let content = "---\ndescription: Run code review\n---\nReview the changes in this PR.";
        let parsed = parse_skill_file(content, "review");
        assert_eq!(parsed.description, "Run code review");
        assert_eq!(parsed.prompt, "Review the changes in this PR.");
        assert!(parsed.allowed_tools.is_empty());
    }

    #[test]
    fn test_parse_without_frontmatter() {
        let content = "Analyze this codebase for security issues.";
        let parsed = parse_skill_file(content, "audit");
        assert_eq!(parsed.description, "Custom command: /audit");
        assert_eq!(parsed.prompt, "Analyze this codebase for security issues.");
    }

    #[test]
    fn test_parse_empty_frontmatter() {
        let content = "---\n---\nDo the thing.";
        let parsed = parse_skill_file(content, "thing");
        assert_eq!(parsed.description, "Custom command: /thing");
        assert_eq!(parsed.prompt, "Do the thing.");
    }

    #[test]
    fn test_parse_allowed_tools() {
        let content = "---\nallowed-tools: Bash(git add:*), Bash(git commit:*), file_read\ndescription: Commit helper\n---\nCommit changes.";
        let parsed = parse_skill_file(content, "commit");
        assert_eq!(parsed.description, "Commit helper");
        assert_eq!(
            parsed.allowed_tools,
            vec!["Bash(git add:*)", "Bash(git commit:*)", "file_read",]
        );
    }

    #[test]
    fn test_interpolate_commands_echo() {
        let prompt = "Branch: !`echo main`";
        let result = interpolate_commands(prompt, ".");
        assert_eq!(result, "Branch: main");
    }

    #[test]
    fn test_interpolate_commands_no_pattern() {
        let prompt = "No commands here, just `code blocks`.";
        let result = interpolate_commands(prompt, ".");
        assert_eq!(result, "No commands here, just `code blocks`.");
    }

    #[test]
    fn test_interpolate_commands_multiple() {
        let prompt = "A: !`echo hello` B: !`echo world`";
        let result = interpolate_commands(prompt, ".");
        assert_eq!(result, "A: hello B: world");
    }

    #[test]
    fn test_interpolate_commands_failure() {
        let prompt = "Result: !`false`";
        let result = interpolate_commands(prompt, ".");
        assert!(result.starts_with("Result: <error:"));
    }

    #[test]
    fn test_substitute_arguments() {
        let prompt = "Review PR $ARGUMENTS for issues.";
        let result = substitute_arguments(prompt, "#123");
        assert_eq!(result, "Review PR #123 for issues.");
    }

    #[test]
    fn test_substitute_arguments_empty() {
        let prompt = "Do the thing with $ARGUMENTS.";
        let result = substitute_arguments(prompt, "");
        assert_eq!(result, "Do the thing with .");
    }

    #[test]
    fn test_substitute_arguments_multiple() {
        let prompt = "$ARGUMENTS is here and $ARGUMENTS is there.";
        let result = substitute_arguments(prompt, "foo");
        assert_eq!(result, "foo is here and foo is there.");
    }

    #[test]
    fn test_parse_allowed_tools_json_array() {
        let content = "---\nallowed-tools: [\"Read\", \"Write\", \"Bash(git:*)\"]\ndescription: Hookify\n---\nDo the thing.";
        let parsed = parse_skill_file(content, "hookify");
        assert_eq!(parsed.allowed_tools, vec!["Read", "Write", "Bash(git:*)"]);
    }

    #[test]
    fn test_parse_argument_hint() {
        let content =
            "---\ndescription: Review code\nargument-hint: Optional PR number\n---\nReview.";
        let parsed = parse_skill_file(content, "review");
        assert_eq!(parsed.argument_hint.as_deref(), Some("Optional PR number"));
    }

    #[test]
    fn test_parse_argument_hint_missing() {
        let content = "---\ndescription: Simple\n---\nDo it.";
        let parsed = parse_skill_file(content, "simple");
        assert!(parsed.argument_hint.is_none());
    }

    #[test]
    fn test_expand_at_mentions_no_refs() {
        let result = expand_at_mentions("hello world", ".");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_expand_at_mentions_nonexistent_file() {
        let result = expand_at_mentions("check @nonexistent.xyz for issues", ".");
        assert_eq!(result, "check @nonexistent.xyz for issues");
        assert!(!result.contains("Referenced Files"));
    }

    #[test]
    fn test_expand_at_mentions_real_file() {
        // Use Cargo.toml which exists at the workspace root
        let result = expand_at_mentions("look at @Cargo.toml", ".");
        assert!(result.contains("Referenced Files"));
        assert!(result.contains("### Cargo.toml"));
        assert!(result.contains("```toml"));
    }

    #[test]
    fn test_expand_at_mentions_bare_at() {
        // Just @ by itself should not be treated as a file reference
        let result = expand_at_mentions("email me @ home", ".");
        assert_eq!(result, "email me @ home");
    }
}

//! Persistent memory system for storing context across sessions.
//!
//! Stores user preferences, project context, feedback, and references
//! as individual markdown files with frontmatter metadata.
//!
//! Memory types:
//! - user: role, goals, preferences
//! - feedback: what to avoid/repeat
//! - project: ongoing work context
//! - reference: pointers to external resources

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// A single memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub memory_type: MemoryType,
    pub content: String,
    /// File path where this memory is stored.
    #[serde(skip)]
    pub file_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    User,
    Feedback,
    Project,
    Reference,
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryType::User => write!(f, "user"),
            MemoryType::Feedback => write!(f, "feedback"),
            MemoryType::Project => write!(f, "project"),
            MemoryType::Reference => write!(f, "reference"),
        }
    }
}

/// The memory index (MEMORY.md) — maps titles to file paths.
#[derive(Debug, Clone, Default)]
pub struct MemoryIndex {
    pub entries: Vec<MemoryIndexEntry>,
}

#[derive(Debug, Clone)]
pub struct MemoryIndexEntry {
    pub title: String,
    pub file_name: String,
    pub description: String,
}

/// Manager for reading and writing memories.
pub struct MemoryStore {
    /// Base directory for memories (e.g. ~/.one/memory/ or project-specific)
    base_dir: PathBuf,
}

impl MemoryStore {
    /// Create a memory store for a specific project.
    pub fn for_project(project_dir: &str) -> Self {
        let hash = project_dir.replace(['/', '\\'], "-");
        let base_dir = dirs_next::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".one")
            .join("memory")
            .join(&hash);
        Self { base_dir }
    }

    /// Create a global memory store (not project-specific).
    pub fn global() -> Self {
        let base_dir = dirs_next::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".one")
            .join("memory")
            .join("global");
        Self { base_dir }
    }

    /// Save a memory to disk. Creates the file and updates MEMORY.md index.
    pub fn save(&self, memory: &Memory) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.base_dir)?;

        // Generate filename from memory name
        let file_name = slugify(&memory.name) + ".md";
        let file_path = self.base_dir.join(&file_name);

        // Write the memory file with frontmatter
        let content = format!(
            "---\nname: {}\ndescription: {}\ntype: {}\n---\n\n{}",
            memory.name, memory.description, memory.memory_type, memory.content
        );
        std::fs::write(&file_path, content)?;

        // Update the index
        self.update_index(&MemoryIndexEntry {
            title: memory.name.clone(),
            file_name: file_name.clone(),
            description: memory.description.clone(),
        })?;

        Ok(file_path)
    }

    /// Load all memories from disk.
    pub fn load_all(&self) -> Vec<Memory> {
        let mut memories = Vec::new();

        if !self.base_dir.exists() {
            return memories;
        }

        let entries = match std::fs::read_dir(&self.base_dir) {
            Ok(e) => e,
            Err(_) => return memories,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "md")
                && path.file_name().is_some_and(|n| n != "MEMORY.md")
                && let Some(memory) = self.load_file(&path)
            {
                memories.push(memory);
            }
        }

        memories
    }

    /// Load a single memory file.
    fn load_file(&self, path: &Path) -> Option<Memory> {
        let content = std::fs::read_to_string(path).ok()?;
        parse_memory_file(&content, path)
    }

    /// Find a memory by name (case-insensitive partial match).
    pub fn find(&self, query: &str) -> Vec<Memory> {
        let lower = query.to_lowercase();
        self.load_all()
            .into_iter()
            .filter(|m| {
                m.name.to_lowercase().contains(&lower)
                    || m.description.to_lowercase().contains(&lower)
            })
            .collect()
    }

    /// Delete a memory by name.
    pub fn delete(&self, name: &str) -> Result<bool> {
        let memories = self.find(name);
        if memories.is_empty() {
            return Ok(false);
        }

        for memory in &memories {
            if memory.file_path.exists() {
                std::fs::remove_file(&memory.file_path)?;
            }
        }

        // Rebuild index
        self.rebuild_index()?;
        Ok(true)
    }

    /// Load the MEMORY.md index.
    pub fn load_index(&self) -> MemoryIndex {
        let index_path = self.base_dir.join("MEMORY.md");
        let content = match std::fs::read_to_string(&index_path) {
            Ok(c) => c,
            Err(_) => return MemoryIndex::default(),
        };

        let mut entries = Vec::new();
        for line in content.lines() {
            // Parse: - [Title](file.md) — description
            if let Some(rest) = line.strip_prefix("- [")
                && let Some(bracket_end) = rest.find("](")
            {
                let title = rest[..bracket_end].to_string();
                let after_bracket = &rest[bracket_end + 2..];
                if let Some(paren_end) = after_bracket.find(')') {
                    let file_name = after_bracket[..paren_end].to_string();
                    let description = after_bracket[paren_end + 1..]
                        .trim_start_matches(" — ")
                        .trim_start_matches(" - ")
                        .trim()
                        .to_string();
                    entries.push(MemoryIndexEntry {
                        title,
                        file_name,
                        description,
                    });
                }
            }
        }

        MemoryIndex { entries }
    }

    /// Update the MEMORY.md index with a new or updated entry.
    fn update_index(&self, entry: &MemoryIndexEntry) -> Result<()> {
        let mut index = self.load_index();

        // Replace existing entry with same file_name, or append
        let existing = index
            .entries
            .iter()
            .position(|e| e.file_name == entry.file_name);
        if let Some(pos) = existing {
            index.entries[pos] = entry.clone();
        } else {
            index.entries.push(entry.clone());
        }

        self.write_index(&index)
    }

    /// Rebuild the index from all memory files on disk.
    fn rebuild_index(&self) -> Result<()> {
        let memories = self.load_all();
        let index = MemoryIndex {
            entries: memories
                .iter()
                .map(|m| {
                    let file_name = m
                        .file_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    MemoryIndexEntry {
                        title: m.name.clone(),
                        file_name,
                        description: m.description.clone(),
                    }
                })
                .collect(),
        };
        self.write_index(&index)
    }

    /// Write the MEMORY.md index file.
    fn write_index(&self, index: &MemoryIndex) -> Result<()> {
        let index_path = self.base_dir.join("MEMORY.md");
        let mut content = String::new();
        for entry in &index.entries {
            content.push_str(&format!(
                "- [{}]({}) — {}\n",
                entry.title, entry.file_name, entry.description
            ));
        }
        std::fs::write(index_path, content)?;
        Ok(())
    }

    /// Build a summary of all memories for inclusion in the system prompt.
    pub fn system_prompt_context(&self) -> String {
        let memories = self.load_all();
        if memories.is_empty() {
            return String::new();
        }

        let mut sections: HashMap<String, Vec<String>> = HashMap::new();

        for memory in &memories {
            let type_name = memory.memory_type.to_string();
            sections
                .entry(type_name)
                .or_default()
                .push(format!("**{}**: {}", memory.name, memory.content));
        }

        let mut prompt = String::from("# Remembered Context\n\n");
        for (type_name, entries) in &sections {
            prompt.push_str(&format!("## {} memories\n\n", type_name));
            for entry in entries {
                prompt.push_str(entry);
                prompt.push_str("\n\n");
            }
        }

        prompt
    }
}

/// Parse a memory file with frontmatter.
fn parse_memory_file(content: &str, path: &Path) -> Option<Memory> {
    if !content.starts_with("---\n") {
        return None;
    }

    let rest = &content[4..];
    let end = rest.find("\n---")?;
    let frontmatter = &rest[..end];
    let body = rest[end + 4..].trim().to_string();

    let mut name = String::new();
    let mut description = String::new();
    let mut memory_type = MemoryType::Project;

    for line in frontmatter.lines() {
        if let Some(val) = line.strip_prefix("name: ") {
            name = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("description: ") {
            description = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("type: ") {
            memory_type = match val.trim() {
                "user" => MemoryType::User,
                "feedback" => MemoryType::Feedback,
                "project" => MemoryType::Project,
                "reference" => MemoryType::Reference,
                _ => MemoryType::Project,
            };
        }
    }

    if name.is_empty() {
        return None;
    }

    Some(Memory {
        name,
        description,
        memory_type,
        content: body,
        file_path: path.to_path_buf(),
    })
}

/// Convert a name to a filename-safe slug.
fn slugify(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

// ─── Auto-Memory Triggers ─────────────────────────────────────

/// Analyze a user message for memory-worthy patterns.
/// Returns a suggested memory if a trigger is detected.
pub fn detect_memory_trigger(user_message: &str) -> Option<Memory> {
    let lower = user_message.to_lowercase();

    // Correction patterns → feedback memory
    let correction_patterns = [
        "don't do that",
        "stop doing",
        "don't use",
        "never use",
        "no not that",
        "that's wrong",
        "not like that",
        "please don't",
        "don't add",
        "don't create",
        "stop adding",
        "avoid using",
        "prefer using",
        "always use",
        "use this instead",
        "from now on",
        "in the future",
        "going forward",
        "remember that",
        "keep in mind",
    ];

    for pattern in &correction_patterns {
        if lower.contains(pattern) {
            return Some(Memory {
                name: extract_memory_name(user_message, 6),
                description: user_message.to_string(),
                memory_type: MemoryType::Feedback,
                content: user_message.to_string(),
                file_path: PathBuf::new(),
            });
        }
    }

    // User role/context patterns → user memory
    let role_patterns = [
        "i'm a ",
        "i am a ",
        "my role is",
        "i work on",
        "i work with",
        "my team",
        "i prefer",
        "i like to",
        "my stack is",
        "we use ",
    ];

    for pattern in &role_patterns {
        if lower.contains(pattern) {
            return Some(Memory {
                name: extract_memory_name(user_message, 5),
                description: user_message.to_string(),
                memory_type: MemoryType::User,
                content: user_message.to_string(),
                file_path: PathBuf::new(),
            });
        }
    }

    // Project context patterns → project memory
    let project_patterns = [
        "the deadline is",
        "we're working on",
        "the goal is",
        "we need to",
        "the project",
        "this repo",
        "this codebase",
        "we're deploying",
        "our ci",
        "our pipeline",
    ];

    for pattern in &project_patterns {
        if lower.contains(pattern) {
            return Some(Memory {
                name: extract_memory_name(user_message, 5),
                description: user_message.to_string(),
                memory_type: MemoryType::Project,
                content: user_message.to_string(),
                file_path: PathBuf::new(),
            });
        }
    }

    None
}

/// Extract a short name from the first N words of a message.
fn extract_memory_name(message: &str, word_count: usize) -> String {
    message
        .split_whitespace()
        .take(word_count)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_memory_file() {
        let content = "---\nname: Test Memory\ndescription: A test\ntype: user\n---\n\nHello world";
        let memory = parse_memory_file(content, Path::new("/tmp/test.md")).unwrap();
        assert_eq!(memory.name, "Test Memory");
        assert_eq!(memory.description, "A test");
        assert_eq!(memory.memory_type, MemoryType::User);
        assert_eq!(memory.content, "Hello world");
    }

    #[test]
    fn test_parse_memory_missing_frontmatter() {
        let content = "Just plain text";
        assert!(parse_memory_file(content, Path::new("/tmp/test.md")).is_none());
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Hello World"), "hello_world");
        assert_eq!(slugify("user-role"), "user_role");
        assert_eq!(slugify("My Project (v2)"), "my_project__v2");
    }

    #[test]
    fn test_memory_store_save_load() {
        let dir = std::env::temp_dir().join("one-test-memory");
        let _ = std::fs::remove_dir_all(&dir);

        let store = MemoryStore {
            base_dir: dir.clone(),
        };

        let memory = Memory {
            name: "Test".to_string(),
            description: "A test memory".to_string(),
            memory_type: MemoryType::User,
            content: "Hello from test".to_string(),
            file_path: PathBuf::new(),
        };

        store.save(&memory).unwrap();

        let loaded = store.load_all();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "Test");
        assert_eq!(loaded[0].content, "Hello from test");

        // Check index
        let index = store.load_index();
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].title, "Test");

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_memory_store_find() {
        let dir = std::env::temp_dir().join("one-test-memory-find");
        let _ = std::fs::remove_dir_all(&dir);

        let store = MemoryStore {
            base_dir: dir.clone(),
        };

        store
            .save(&Memory {
                name: "User Role".to_string(),
                description: "User is a senior dev".to_string(),
                memory_type: MemoryType::User,
                content: "Senior Rust developer".to_string(),
                file_path: PathBuf::new(),
            })
            .unwrap();

        store
            .save(&Memory {
                name: "Project Context".to_string(),
                description: "Working on CLI".to_string(),
                memory_type: MemoryType::Project,
                content: "Building One CLI".to_string(),
                file_path: PathBuf::new(),
            })
            .unwrap();

        let results = store.find("user");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "User Role");

        let results = store.find("cli");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Project Context");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_system_prompt_context() {
        let dir = std::env::temp_dir().join("one-test-memory-prompt");
        let _ = std::fs::remove_dir_all(&dir);

        let store = MemoryStore {
            base_dir: dir.clone(),
        };

        store
            .save(&Memory {
                name: "Preference".to_string(),
                description: "User likes concise output".to_string(),
                memory_type: MemoryType::Feedback,
                content: "Keep responses short".to_string(),
                file_path: PathBuf::new(),
            })
            .unwrap();

        let context = store.system_prompt_context();
        assert!(context.contains("Remembered Context"));
        assert!(context.contains("Keep responses short"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_correction_trigger() {
        let result = detect_memory_trigger("don't use tabs, always use spaces");
        assert!(result.is_some());
        let mem = result.unwrap();
        assert_eq!(mem.memory_type, MemoryType::Feedback);
    }

    #[test]
    fn test_detect_role_trigger() {
        let result = detect_memory_trigger("I'm a senior Rust developer working on CLI tools");
        assert!(result.is_some());
        let mem = result.unwrap();
        assert_eq!(mem.memory_type, MemoryType::User);
    }

    #[test]
    fn test_detect_project_trigger() {
        let result = detect_memory_trigger("the deadline is next Friday for the v2 release");
        assert!(result.is_some());
        let mem = result.unwrap();
        assert_eq!(mem.memory_type, MemoryType::Project);
    }

    #[test]
    fn test_no_trigger_normal_message() {
        let result = detect_memory_trigger("read src/main.rs and explain the architecture");
        assert!(result.is_none());
    }
}

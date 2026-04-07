use std::collections::HashMap;

use crate::Tool;
use crate::tool_search::DeferredToolInfo;

/// Registry of all available tools, keyed by name.
/// The AI query engine looks up tools here when processing tool_use blocks.
/// Supports deferred tool loading — tools marked `should_defer()` are
/// excluded from the initial API request and loaded on-demand via ToolSearch.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// All tool schemas — used for internal purposes (agent filtering, etc.)
    pub fn schemas(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|t| {
                serde_json::json!({
                    "name": t.name(),
                    "description": t.description(),
                    "input_schema": t.input_schema(),
                })
            })
            .collect()
    }

    /// Schemas for always-loaded tools only (excludes deferred tools).
    /// These are sent in every API request. Deferred tools are loaded via ToolSearch.
    pub fn active_schemas(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .filter(|t| !t.should_defer())
            .map(|t| {
                serde_json::json!({
                    "name": t.name(),
                    "description": t.description(),
                    "input_schema": t.input_schema(),
                })
            })
            .collect()
    }

    /// Collect metadata snapshots of all deferred tools.
    /// Used to initialize ToolSearch before it's registered.
    pub fn collect_deferred_info(&self) -> Vec<DeferredToolInfo> {
        self.tools
            .values()
            .filter(|t| t.should_defer())
            .map(|t| DeferredToolInfo {
                name: t.name().to_string(),
                description: t.description().to_string(),
                search_hint: t.search_hint().map(String::from),
                schema: serde_json::json!({
                    "name": t.name(),
                    "description": t.description(),
                    "input_schema": t.input_schema(),
                }),
            })
            .collect()
    }

    /// Names of deferred tools — listed in system prompt so the model knows they exist.
    pub fn deferred_tool_names(&self) -> Vec<&str> {
        self.tools
            .values()
            .filter(|t| t.should_defer())
            .map(|t| t.name())
            .collect()
    }

    /// Search deferred tools by keyword query. Returns matching tool schemas.
    /// Supports:
    /// - `select:Name1,Name2` — exact name lookup (case-insensitive)
    /// - keyword search — scored by term frequency in name/description/search_hint
    pub fn search_tools(&self, query: &str, max_results: usize) -> Vec<serde_json::Value> {
        // Fast path: select:Name1,Name2
        if let Some(names) = query.strip_prefix("select:") {
            return names
                .split(',')
                .filter_map(|name| {
                    let name = name.trim();
                    // Case-insensitive lookup
                    self.tools
                        .values()
                        .find(|t| t.name().eq_ignore_ascii_case(name))
                })
                .map(|t| {
                    serde_json::json!({
                        "name": t.name(),
                        "description": t.description(),
                        "input_schema": t.input_schema(),
                    })
                })
                .collect();
        }

        // Parse query into required (+term) and optional terms
        let mut required_terms = Vec::new();
        let mut optional_terms = Vec::new();

        for token in query.split_whitespace() {
            if let Some(term) = token.strip_prefix('+') {
                required_terms.push(term.to_lowercase());
            } else {
                optional_terms.push(token.to_lowercase());
            }
        }

        let all_terms: Vec<&str> = required_terms
            .iter()
            .chain(optional_terms.iter())
            .map(|s| s.as_str())
            .collect();

        // Score each deferred tool
        let mut scored: Vec<(i32, &dyn Tool)> = self
            .tools
            .values()
            .filter(|t| t.should_defer())
            .filter(|t| {
                // Must contain all required terms somewhere
                required_terms.iter().all(|req| {
                    let name_lower = t.name().to_lowercase();
                    let desc_lower = t.description().to_lowercase();
                    let hint_lower = t.search_hint().unwrap_or("").to_lowercase();
                    name_lower.contains(req.as_str())
                        || desc_lower.contains(req.as_str())
                        || hint_lower.contains(req.as_str())
                })
            })
            .map(|t| {
                let name_lower = t.name().to_lowercase();
                let desc_lower = t.description().to_lowercase();
                let hint_lower = t.search_hint().unwrap_or("").to_lowercase();

                let mut score: i32 = 0;

                for term in &all_terms {
                    // Name part match (highest weight)
                    if name_lower.contains(term) {
                        // Exact name part (split by _) gets highest score
                        let name_parts: Vec<&str> = name_lower.split('_').collect();
                        if name_parts.iter().any(|p| p == term) {
                            score += 10;
                        } else {
                            score += 5;
                        }
                    }

                    // Search hint match
                    if hint_lower.contains(term) {
                        score += 4;
                    }

                    // Description match (lowest weight)
                    if desc_lower.contains(term) {
                        score += 2;
                    }
                }

                (score, t.as_ref())
            })
            .filter(|(score, _)| *score > 0)
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0));

        scored
            .into_iter()
            .take(max_results)
            .map(|(_, t)| {
                serde_json::json!({
                    "name": t.name(),
                    "description": t.description(),
                    "input_schema": t.input_schema(),
                })
            })
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

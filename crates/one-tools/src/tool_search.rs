//! ToolSearch tool — enables deferred tool loading.
//!
//! Tools marked with `should_defer()` are not sent in the initial API request.
//! Instead, the model uses ToolSearch to discover and load their schemas
//! on demand, reducing prompt token usage.
//!
//! Supports `select:Name` for exact lookup and keyword search with
//! scoring against name/description/search_hint.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

/// Snapshot of a deferred tool's metadata for search purposes.
#[derive(Debug, Clone)]
pub struct DeferredToolInfo {
    pub name: String,
    pub description: String,
    pub search_hint: Option<String>,
    pub schema: Value,
}

/// ToolSearch discovers deferred tools by keyword or exact name.
/// Holds a snapshot of deferred tool metadata taken at registry construction time.
pub struct ToolSearchTool {
    deferred_tools: Vec<DeferredToolInfo>,
}

impl ToolSearchTool {
    pub fn new(deferred_tools: Vec<DeferredToolInfo>) -> Self {
        Self { deferred_tools }
    }

    /// Search deferred tools by query. Returns matching tool schemas.
    fn search(&self, query: &str, max_results: usize) -> Vec<Value> {
        // Fast path: select:Name1,Name2
        if let Some(names) = query.strip_prefix("select:") {
            return names
                .split(',')
                .filter_map(|name| {
                    let name = name.trim();
                    self.deferred_tools
                        .iter()
                        .find(|t| t.name.eq_ignore_ascii_case(name))
                })
                .map(|t| t.schema.clone())
                .collect();
        }

        // Parse query: +term = required, others = optional
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
        let mut scored: Vec<(i32, &DeferredToolInfo)> = self
            .deferred_tools
            .iter()
            .filter(|t| {
                required_terms.iter().all(|req| {
                    let name_lower = t.name.to_lowercase();
                    let desc_lower = t.description.to_lowercase();
                    let hint_lower = t.search_hint.as_deref().unwrap_or("").to_lowercase();
                    name_lower.contains(req.as_str())
                        || desc_lower.contains(req.as_str())
                        || hint_lower.contains(req.as_str())
                })
            })
            .map(|t| {
                let name_lower = t.name.to_lowercase();
                let desc_lower = t.description.to_lowercase();
                let hint_lower = t.search_hint.as_deref().unwrap_or("").to_lowercase();

                let mut score: i32 = 0;

                for term in &all_terms {
                    // Name part match (highest weight)
                    if name_lower.contains(term) {
                        let name_parts: Vec<&str> = name_lower.split('_').collect();
                        if name_parts.iter().any(|p| p == term) {
                            score += 10; // exact name part
                        } else {
                            score += 5; // partial name match
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

                (score, t)
            })
            .filter(|(score, _)| *score > 0)
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0));

        scored
            .into_iter()
            .take(max_results)
            .map(|(_, t)| t.schema.clone())
            .collect()
    }
}

impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "tool_search"
    }

    fn description(&self) -> &str {
        "Search for and load deferred tool schemas. Use \"select:tool_name\" for exact \
         lookup or keywords to search by capability. Returns full tool schemas that \
         can then be invoked."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Query to find deferred tools. Use \"select:<tool_name>\" for direct \
                                    selection, or keywords to search. Use \"+term\" to require a term."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 5)"
                }
            },
            "required": ["query"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let query = input["query"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required parameter: query"))?;

            let max_results = input["max_results"].as_u64().unwrap_or(5) as usize;

            let matches = self.search(query, max_results);

            if matches.is_empty() {
                let available: Vec<&str> = self
                    .deferred_tools
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect();

                let msg = if available.is_empty() {
                    "No deferred tools available.".to_string()
                } else {
                    format!(
                        "No matches for \"{query}\". Available deferred tools: {}",
                        available.join(", ")
                    )
                };
                return Ok(ToolResult::success(msg));
            }

            let mut output = format!(
                "Found {} tool(s) matching \"{}\":\n\n",
                matches.len(),
                query
            );

            for schema in &matches {
                output.push_str(&serde_json::to_string_pretty(schema).unwrap_or_default());
                output.push_str("\n\n");
            }

            output.push_str(&format!(
                "Total deferred tools: {}. Use these tools by calling them with the schemas above.",
                self.deferred_tools.len()
            ));

            Ok(ToolResult::success(output))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn should_defer(&self) -> bool {
        false // ToolSearch must always be loaded
    }

    fn search_hint(&self) -> Option<&str> {
        Some("find discover load deferred tool schema")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context() -> ToolContext {
        ToolContext::new("/tmp", "test")
    }

    fn make_tool() -> ToolSearchTool {
        let deferred = vec![
            DeferredToolInfo {
                name: "web_fetch".to_string(),
                description: "Fetch a URL and return its content as text.".to_string(),
                search_hint: Some("fetch URL web page HTTP content".to_string()),
                schema: serde_json::json!({
                    "name": "web_fetch",
                    "description": "Fetch a URL",
                    "input_schema": {"type": "object", "properties": {"url": {"type": "string"}}}
                }),
            },
            DeferredToolInfo {
                name: "web_search".to_string(),
                description: "Search the web for information.".to_string(),
                search_hint: Some("search web DuckDuckGo query".to_string()),
                schema: serde_json::json!({
                    "name": "web_search",
                    "description": "Search the web",
                    "input_schema": {"type": "object", "properties": {"query": {"type": "string"}}}
                }),
            },
        ];
        ToolSearchTool::new(deferred)
    }

    #[tokio::test]
    async fn test_select_exact_name() {
        let tool = make_tool();
        let ctx = test_context();

        let result = tool
            .execute(serde_json::json!({"query": "select:web_fetch"}), &ctx)
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("web_fetch"));
        assert!(result.output.contains("input_schema"));
    }

    #[tokio::test]
    async fn test_select_multiple() {
        let tool = make_tool();
        let ctx = test_context();

        let result = tool
            .execute(
                serde_json::json!({"query": "select:web_fetch,web_search"}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("web_fetch"));
        assert!(result.output.contains("web_search"));
    }

    #[tokio::test]
    async fn test_keyword_search() {
        let tool = make_tool();
        let ctx = test_context();

        let result = tool
            .execute(serde_json::json!({"query": "fetch URL"}), &ctx)
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("web_fetch"));
    }

    #[tokio::test]
    async fn test_keyword_search_duck() {
        let tool = make_tool();
        let ctx = test_context();

        let result = tool
            .execute(serde_json::json!({"query": "DuckDuckGo"}), &ctx)
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("web_search"));
    }

    #[tokio::test]
    async fn test_required_term() {
        let tool = make_tool();
        let ctx = test_context();

        let result = tool
            .execute(serde_json::json!({"query": "+fetch HTTP"}), &ctx)
            .await
            .unwrap();

        assert!(!result.is_error);
        // Only web_fetch should match (required +fetch)
        assert!(result.output.contains("web_fetch"));
        assert!(!result.output.contains("\"name\": \"web_search\""));
    }

    #[tokio::test]
    async fn test_no_matches() {
        let tool = make_tool();
        let ctx = test_context();

        let result = tool
            .execute(serde_json::json!({"query": "nonexistent_xyz"}), &ctx)
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("No matches"));
        assert!(result.output.contains("web_fetch"));
        assert!(result.output.contains("web_search"));
    }

    #[tokio::test]
    async fn test_never_deferred() {
        let tool = make_tool();
        assert!(!tool.should_defer());
        assert!(tool.is_read_only());
    }

    #[tokio::test]
    async fn test_case_insensitive_select() {
        let tool = make_tool();
        let ctx = test_context();

        let result = tool
            .execute(serde_json::json!({"query": "select:Web_Fetch"}), &ctx)
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("web_fetch"));
    }
}

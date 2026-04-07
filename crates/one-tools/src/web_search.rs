//! WebSearch tool — search the web and return results.
//!
//! Uses DuckDuckGo's HTML lite interface (no API key required).
//! Returns a list of results with titles, URLs, and snippets.

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

pub struct WebSearchTool;

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for information. Returns a list of results with titles, URLs, and \
         snippets. Use this when you need up-to-date information, documentation, or answers \
         that may not be in the codebase."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10)"
                }
            },
            "required": ["query"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let query = input["query"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required parameter: query"))?;

            let max_results = input["max_results"].as_u64().unwrap_or(10) as usize;

            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::limited(5))
                .user_agent("one-cli/0.1.0")
                .build()?;

            // Use DuckDuckGo HTML lite (no API key needed)
            let url = format!("https://html.duckduckgo.com/html/?q={}", urlencoding(query));

            let response = match client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => {
                    return Ok(ToolResult::error(format!("Search failed: {e}")));
                }
            };

            if !response.status().is_success() {
                return Ok(ToolResult::error(format!(
                    "Search returned HTTP {}",
                    response.status()
                )));
            }

            let body = response.text().await?;
            let results = parse_ddg_results(&body, max_results);

            if results.is_empty() {
                return Ok(ToolResult::success(format!(
                    "No results found for: {query}"
                )));
            }

            let mut output = format!("Search results for: {query}\n\n");
            for (i, result) in results.iter().enumerate() {
                output.push_str(&format!(
                    "{}. {}\n   {}\n   {}\n\n",
                    i + 1,
                    result.title,
                    result.url,
                    result.snippet
                ));
            }

            Ok(ToolResult::success(output))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&str> {
        Some("search web DuckDuckGo query information")
    }
}

struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

/// URL-encode a search query.
fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}

/// Parse DuckDuckGo HTML lite search results.
fn parse_ddg_results(html: &str, max_results: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();

    // DDG HTML lite uses <a class="result__a" href="...">Title</a>
    // and <a class="result__snippet" ...>Snippet</a>
    // We parse these with simple string matching.

    let mut pos = 0;
    while results.len() < max_results {
        // Find next result link
        let link_marker = "class=\"result__a\"";
        let link_start = match html[pos..].find(link_marker) {
            Some(idx) => pos + idx,
            None => break,
        };

        // Extract href
        let href_start = match html[..link_start].rfind("href=\"") {
            Some(idx) => idx + 6,
            None => {
                pos = link_start + link_marker.len();
                continue;
            }
        };
        let href_end = match html[href_start..].find('"') {
            Some(idx) => href_start + idx,
            None => {
                pos = link_start + link_marker.len();
                continue;
            }
        };
        let raw_url = &html[href_start..href_end];

        // DDG wraps URLs in a redirect — extract the actual URL
        let url = if let Some(uddg) = raw_url.find("uddg=") {
            let start = uddg + 5;
            let end = raw_url[start..]
                .find('&')
                .map(|i| start + i)
                .unwrap_or(raw_url.len());
            percent_decode(&raw_url[start..end])
        } else {
            raw_url.to_string()
        };

        // Extract title (text between > and </a>)
        let title_start = match html[link_start..].find('>') {
            Some(idx) => link_start + idx + 1,
            None => {
                pos = link_start + link_marker.len();
                continue;
            }
        };
        let title_end = match html[title_start..].find("</a>") {
            Some(idx) => title_start + idx,
            None => {
                pos = link_start + link_marker.len();
                continue;
            }
        };
        let title = strip_tags(&html[title_start..title_end]);

        // Find snippet (appears after the link)
        let snippet_marker = "class=\"result__snippet\"";
        let snippet = if let Some(snippet_start) = html[title_end..].find(snippet_marker) {
            let abs_start = title_end + snippet_start;
            if let Some(tag_end) = html[abs_start..].find('>') {
                let text_start = abs_start + tag_end + 1;
                if let Some(text_end) = html[text_start..].find("</") {
                    strip_tags(&html[text_start..text_start + text_end])
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult {
                title: decode_entities(&title),
                url,
                snippet: decode_entities(&snippet),
            });
        }

        pos = title_end;
    }

    results
}

/// Strip HTML tags from a string.
fn strip_tags(s: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(c);
        }
    }
    result.trim().to_string()
}

/// Decode common HTML entities.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}

/// Percent-decode a URL string.
fn percent_decode(s: &str) -> String {
    let mut result = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
        {
            result.push(byte as char);
            i += 3;
            continue;
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

//! WebFetch tool — fetch a URL and return its content as text.
//!
//! Fetches HTML pages and extracts readable text content by stripping tags.
//! Also handles plain text, JSON, and other content types.

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

pub struct WebFetchTool;

impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch a URL and return its content as text. Supports HTML (stripped to readable text), \
         JSON, and plain text. Use this to read web pages, API responses, or documentation."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "max_length": {
                    "type": "integer",
                    "description": "Maximum response length in characters (default: 50000)"
                }
            },
            "required": ["url"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let url = input["url"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required parameter: url"))?;

            let max_length = input["max_length"].as_u64().unwrap_or(50_000) as usize;

            // Validate URL
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Ok(ToolResult::error("URL must start with http:// or https://"));
            }

            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::limited(5))
                .user_agent("one-cli/0.1.0")
                .build()?;

            let response = match client.get(url).send().await {
                Ok(r) => r,
                Err(e) => {
                    return Ok(ToolResult::error(format!("Failed to fetch URL: {e}")));
                }
            };

            let status = response.status();
            if !status.is_success() {
                return Ok(ToolResult::error(format!(
                    "HTTP {}: {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("Unknown")
                )));
            }

            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/plain")
                .to_string();

            let body = match response.text().await {
                Ok(t) => t,
                Err(e) => {
                    return Ok(ToolResult::error(format!(
                        "Failed to read response body: {e}"
                    )));
                }
            };

            let text = if content_type.contains("text/html") {
                strip_html_tags(&body)
            } else {
                body
            };

            // Truncate if needed
            let output = if text.len() > max_length {
                format!(
                    "{}\n\n[Truncated: showing {max_length} of {} characters]",
                    &text[..max_length],
                    text.len()
                )
            } else {
                text
            };

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
        Some("fetch URL web page HTTP content")
    }
}

/// Simple HTML tag stripper — extracts readable text from HTML.
/// Not a full parser, but handles common patterns well enough for
/// extracting documentation and article content.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut last_was_whitespace = false;

    let lower = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if !in_tag && chars[i] == '<' {
            // Check for script/style opening tags
            if i + 7 < len && &lower[i..i + 7] == "<script" {
                in_script = true;
            }
            if i + 6 < len && &lower[i..i + 6] == "<style" {
                in_style = true;
            }
            in_tag = true;
            i += 1;
            continue;
        }

        if in_tag && chars[i] == '>' {
            // Check for script/style closing tags
            // Look backward for the tag name
            let tag_content: String = chars[i.saturating_sub(20)..i]
                .iter()
                .collect::<String>()
                .to_lowercase();
            if tag_content.contains("/script") {
                in_script = false;
            }
            if tag_content.contains("/style") {
                in_style = false;
            }

            in_tag = false;

            // Block-level tags get a newline
            let _ = lower_chars; // suppress unused warning
            i += 1;
            continue;
        }

        if !in_tag && !in_script && !in_style {
            // Decode common HTML entities
            if chars[i] == '&' {
                let rest: String = chars[i..std::cmp::min(i + 10, len)].iter().collect();
                if rest.starts_with("&amp;") {
                    result.push('&');
                    i += 5;
                    last_was_whitespace = false;
                    continue;
                } else if rest.starts_with("&lt;") {
                    result.push('<');
                    i += 4;
                    last_was_whitespace = false;
                    continue;
                } else if rest.starts_with("&gt;") {
                    result.push('>');
                    i += 4;
                    last_was_whitespace = false;
                    continue;
                } else if rest.starts_with("&quot;") {
                    result.push('"');
                    i += 6;
                    last_was_whitespace = false;
                    continue;
                } else if rest.starts_with("&nbsp;") {
                    result.push(' ');
                    i += 6;
                    last_was_whitespace = true;
                    continue;
                } else if rest.starts_with("&#") {
                    // Numeric entity — skip to semicolon
                    if let Some(semi) = rest.find(';') {
                        let num_str = &rest[2..semi];
                        let code = if let Some(hex) = num_str.strip_prefix('x') {
                            u32::from_str_radix(hex, 16).ok()
                        } else {
                            num_str.parse::<u32>().ok()
                        };
                        if let Some(c) = code.and_then(char::from_u32) {
                            result.push(c);
                        }
                        i += semi + 1;
                        last_was_whitespace = false;
                        continue;
                    }
                }
            }

            let c = chars[i];
            if c.is_whitespace() {
                if !last_was_whitespace && !result.is_empty() {
                    result.push(' ');
                    last_was_whitespace = true;
                }
            } else {
                result.push(c);
                last_was_whitespace = false;
            }
        }

        i += 1;
    }

    // Clean up: collapse multiple blank lines
    let mut cleaned = String::new();
    let mut blank_count = 0;
    for line in result.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 2 {
                cleaned.push('\n');
            }
        } else {
            blank_count = 0;
            cleaned.push_str(trimmed);
            cleaned.push('\n');
        }
    }

    cleaned.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_html_basic() {
        let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
        let text = strip_html_tags(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains("<"));
    }

    #[test]
    fn test_strip_html_script_style() {
        let html = "<p>Before</p><script>var x = 1;</script><style>.foo{}</style><p>After</p>";
        let text = strip_html_tags(html);
        assert!(text.contains("Before"));
        assert!(text.contains("After"));
        assert!(!text.contains("var x"));
        assert!(!text.contains(".foo"));
    }

    #[test]
    fn test_strip_html_entities() {
        let html = "1 &lt; 2 &amp; 3 &gt; 0 &quot;hello&quot;";
        let text = strip_html_tags(html);
        assert!(text.contains("1 < 2 & 3 > 0 \"hello\""));
    }

    #[test]
    fn test_strip_html_whitespace() {
        let html = "<p>  too   many    spaces  </p>";
        let text = strip_html_tags(html);
        // Should collapse whitespace
        assert!(!text.contains("   "));
    }
}

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

/// Reads a file from the filesystem. Caps output at MAX_LINES to prevent
/// context window overflow. Supports offset/limit for reading chunks.
/// Blocks device files and detects binary files.
pub struct FileReadTool;

const MAX_LINES: usize = 2000;
const MAX_SIZE_BYTES: u64 = 256 * 1024; // 256KB pre-read gate

/// Image extensions that can be sent as multimodal content blocks.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// PDF extension — readable via pdftotext.
const PDF_EXTENSION: &str = "pdf";

/// Extensions that indicate truly binary files (not readable at all).
const BINARY_EXTENSIONS: &[&str] = &[
    "bmp", "ico", "svg", "tiff", "tif", "mp3", "mp4", "avi", "mov", "mkv", "flv", "wav", "ogg",
    "flac", "zip", "tar", "gz", "bz2", "xz", "7z", "rar", "exe", "dll", "so", "dylib", "bin",
    "obj", "o", "a", "wasm", "class", "pyc", "pyo", "ttf", "otf", "woff", "woff2", "eot", "sqlite",
    "db", "sqlite3",
];

/// Paths that should never be read (device files, infinite streams).
const BLOCKED_PATHS: &[&str] = &[
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/null",
    "/dev/stdin",
    "/dev/stdout",
    "/dev/stderr",
    "/dev/tty",
];

impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "Read"
    }

    fn description(&self) -> &str {
        "Read a file from the filesystem. Returns the file contents with line numbers. \
         Supports images (PNG/JPG/GIF/WebP as base64), PDFs (via pdftotext with pages param), \
         and Jupyter notebooks (.ipynb). Use offset/limit for large text files."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (0-based). Optional."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read. Defaults to 2000."
                },
                "pages": {
                    "type": "string",
                    "description": "Page range for PDF files (e.g. \"1-5\", \"3\"). Only for .pdf files."
                }
            },
            "required": ["file_path"]
        })
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        let input = input.clone();
        let working_dir = ctx.working_dir.clone();
        let read_files = ctx.read_files.clone();

        Box::pin(async move {
            let file_path = input["file_path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("file_path is required"))?;

            // Resolve relative paths against working directory
            let path = if Path::new(file_path).is_absolute() {
                std::path::PathBuf::from(file_path)
            } else {
                std::path::PathBuf::from(&working_dir).join(file_path)
            };

            // Track that this file has been read (for Edit safety checks)
            if let Ok(canonical) = std::fs::canonicalize(&path)
                && let Ok(mut set) = read_files.lock()
            {
                set.insert(canonical.to_string_lossy().to_string());
            }

            // Block device files
            let path_str = path.to_string_lossy();
            if BLOCKED_PATHS.iter().any(|p| path_str.starts_with(p))
                || path_str.starts_with("/proc/") && path_str.contains("/fd/")
            {
                return Ok(ToolResult::error(format!(
                    "Cannot read device file: {}",
                    path.display()
                )));
            }

            // Check file extension for special handling
            let ext_lower = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase())
                .unwrap_or_default();

            // Truly binary files — reject
            if BINARY_EXTENSIONS.contains(&ext_lower.as_str()) {
                return Ok(ToolResult::error(format!(
                    "Cannot read binary file: {} ({})",
                    path.display(),
                    ext_lower
                )));
            }

            // Image files — read as base64 for multimodal
            if IMAGE_EXTENSIONS.contains(&ext_lower.as_str()) {
                if !path.exists() {
                    return Ok(ToolResult::error(format!(
                        "File does not exist: {}",
                        path.display()
                    )));
                }
                return match tokio::fs::read(&path).await {
                    Ok(bytes) => {
                        use base64::Engine;
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        let media_type = match ext_lower.as_str() {
                            "png" => "image/png",
                            "jpg" | "jpeg" => "image/jpeg",
                            "gif" => "image/gif",
                            "webp" => "image/webp",
                            _ => "application/octet-stream",
                        };
                        // Return as content block JSON that providers can send as multimodal
                        let content = serde_json::json!([
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": media_type,
                                    "data": b64,
                                }
                            },
                            {
                                "type": "text",
                                "text": format!("Image file: {} ({} bytes)", path.display(), bytes.len())
                            }
                        ]);
                        Ok(ToolResult::success(content.to_string()))
                    }
                    Err(e) => Ok(ToolResult::error(format!("Error reading image: {e}"))),
                };
            }

            // PDF files — extract text via pdftotext
            if ext_lower == PDF_EXTENSION {
                if !path.exists() {
                    return Ok(ToolResult::error(format!(
                        "File does not exist: {}",
                        path.display()
                    )));
                }
                let pages = input["pages"].as_str().unwrap_or("");
                let mut cmd = tokio::process::Command::new("pdftotext");
                if !pages.is_empty() {
                    // Parse page range like "1-5" or "3"
                    if let Some((first, last)) = pages.split_once('-') {
                        cmd.arg("-f").arg(first.trim());
                        cmd.arg("-l").arg(last.trim());
                    } else {
                        cmd.arg("-f").arg(pages.trim());
                        cmd.arg("-l").arg(pages.trim());
                    }
                }
                cmd.arg(path.to_string_lossy().as_ref());
                cmd.arg("-"); // output to stdout

                return match cmd.output().await {
                    Ok(output) if output.status.success() => {
                        let text = String::from_utf8_lossy(&output.stdout);
                        if text.trim().is_empty() {
                            Ok(ToolResult::success(format!(
                                "(empty PDF: {})",
                                path.display()
                            )))
                        } else {
                            Ok(ToolResult::success(text.to_string()))
                        }
                    }
                    Ok(_) | Err(_) => Ok(ToolResult::error(format!(
                        "Cannot read PDF: {} (install pdftotext for PDF support)",
                        path.display()
                    ))),
                };
            }

            if !path.exists() {
                return Ok(ToolResult::error(format!(
                    "File does not exist: {}",
                    path.display()
                )));
            }

            // Check if it's a directory
            if path.is_dir() {
                return Ok(ToolResult::error(format!(
                    "{} is a directory, not a file. Use Glob to list files.",
                    path.display()
                )));
            }

            // Check file size before reading
            if let Ok(metadata) = tokio::fs::metadata(&path).await
                && metadata.len() > MAX_SIZE_BYTES
            {
                return Ok(ToolResult::error(format!(
                    "File too large: {} ({} bytes, max {}). Use offset/limit to read in chunks.",
                    path.display(),
                    metadata.len(),
                    MAX_SIZE_BYTES
                )));
            }

            let content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) => {
                    // Check if it's a binary content error
                    let err_str = e.to_string();
                    if err_str.contains("invalid utf-8")
                        || err_str.contains("stream did not contain valid UTF-8")
                    {
                        return Ok(ToolResult::error(format!(
                            "Cannot read binary file: {} (not valid UTF-8)",
                            path.display()
                        )));
                    }
                    return Ok(ToolResult::error(format!(
                        "Error reading {}: {e}",
                        path.display()
                    )));
                }
            };

            // Handle Jupyter notebooks (.ipynb) — parse JSON and extract cells
            if path.extension().and_then(|e| e.to_str()) == Some("ipynb") {
                return Ok(ToolResult::success(format_notebook(&content, &path)));
            }

            // Handle empty files
            if content.is_empty() {
                return Ok(ToolResult::success(format!(
                    "(empty file: {})",
                    path.display()
                )));
            }

            let offset = input["offset"].as_u64().unwrap_or(0) as usize;
            let limit = input["limit"].as_u64().unwrap_or(MAX_LINES as u64) as usize;

            let lines: Vec<&str> = content.lines().collect();
            let total = lines.len();

            // Validate offset
            if offset >= total {
                return Ok(ToolResult::error(format!(
                    "Offset {} exceeds file length ({} lines)",
                    offset, total
                )));
            }

            let selected: Vec<String> = lines
                .into_iter()
                .skip(offset)
                .take(limit)
                .enumerate()
                .map(|(i, line)| format!("{}\t{}", offset + i + 1, line))
                .collect();

            let mut output = selected.join("\n");

            if offset + limit < total {
                output.push_str(&format!(
                    "\n\n... ({} more lines not shown. Use offset/limit to read more.)",
                    total - offset - limit
                ));
            }

            Ok(ToolResult::success(output))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&str> {
        Some("read file contents text code source")
    }
}

/// Format a Jupyter notebook (.ipynb) for display.
/// Parses the JSON structure and extracts cells with their type,
/// source code, and outputs.
fn format_notebook(content: &str, path: &std::path::Path) -> String {
    let Ok(notebook) = serde_json::from_str::<serde_json::Value>(content) else {
        return format!("Failed to parse notebook: {}", path.display());
    };

    let cells = notebook["cells"].as_array();
    let Some(cells) = cells else {
        return format!("No cells found in notebook: {}", path.display());
    };

    let kernel = notebook["metadata"]["kernelspec"]["display_name"]
        .as_str()
        .unwrap_or("unknown");

    let mut lines = vec![format!(
        "Jupyter Notebook: {} ({} cells, kernel: {kernel})\n",
        path.display(),
        cells.len()
    )];

    for (i, cell) in cells.iter().enumerate() {
        let cell_type = cell["cell_type"].as_str().unwrap_or("unknown");

        // Extract source (array of strings or single string)
        let source = match &cell["source"] {
            serde_json::Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(""),
            serde_json::Value::String(s) => s.clone(),
            _ => String::new(),
        };

        if source.trim().is_empty() {
            continue;
        }

        match cell_type {
            "code" => {
                lines.push(format!("--- Cell {} [code] ---", i + 1));
                lines.push(format!("```\n{}\n```", source.trim()));

                // Extract outputs
                if let Some(outputs) = cell["outputs"].as_array() {
                    for output in outputs {
                        match output["output_type"].as_str() {
                            Some("stream") => {
                                let text = extract_text(&output["text"]);
                                if !text.is_empty() {
                                    lines.push(format!("Output:\n{text}"));
                                }
                            }
                            Some("execute_result") | Some("display_data") => {
                                let text = extract_text(&output["data"]["text/plain"]);
                                if !text.is_empty() {
                                    lines.push(format!("Result: {text}"));
                                }
                            }
                            Some("error") => {
                                let ename = output["ename"].as_str().unwrap_or("Error");
                                let evalue = output["evalue"].as_str().unwrap_or("");
                                lines.push(format!("Error: {ename}: {evalue}"));
                            }
                            _ => {}
                        }
                    }
                }
                lines.push(String::new());
            }
            "markdown" => {
                lines.push(format!("--- Cell {} [markdown] ---", i + 1));
                lines.push(source.trim().to_string());
                lines.push(String::new());
            }
            "raw" => {
                lines.push(format!("--- Cell {} [raw] ---", i + 1));
                lines.push(source.trim().to_string());
                lines.push(String::new());
            }
            _ => {}
        }
    }

    lines.join("\n")
}

/// Extract text from a notebook output field (array of strings or single string).
fn extract_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join("")
            .trim()
            .to_string(),
        serde_json::Value::String(s) => s.trim().to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext::new("/tmp", "test")
    }

    #[tokio::test]
    async fn test_nonexistent_file() {
        let tool = FileReadTool;
        let result = tool
            .execute(
                serde_json::json!({"file_path": "/tmp/definitely_does_not_exist_12345.txt"}),
                &ctx(),
            )
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("does not exist"));
    }

    #[tokio::test]
    async fn test_device_file_blocked() {
        let tool = FileReadTool;
        let result = tool
            .execute(serde_json::json!({"file_path": "/dev/zero"}), &ctx())
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("Cannot read device file"));
    }

    #[tokio::test]
    async fn test_binary_extension_blocked() {
        let tool = FileReadTool;
        let result = tool
            .execute(serde_json::json!({"file_path": "/tmp/test.exe"}), &ctx())
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("binary file"));
    }

    #[tokio::test]
    async fn test_image_returns_base64() {
        // PNG files should return base64 content blocks (not an error)
        let tool = FileReadTool;
        let result = tool
            .execute(
                serde_json::json!({"file_path": "/tmp/test_nonexistent.png"}),
                &ctx(),
            )
            .await
            .unwrap();

        // Non-existent image → error (but NOT "binary file" error)
        assert!(result.is_error);
        assert!(result.output.contains("does not exist"));
    }

    #[tokio::test]
    async fn test_directory_rejected() {
        let tool = FileReadTool;
        let result = tool
            .execute(serde_json::json!({"file_path": "/tmp"}), &ctx())
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("directory"));
    }

    #[test]
    fn test_tool_properties() {
        let tool = FileReadTool;
        assert_eq!(tool.name(), "Read");
        assert!(tool.is_read_only());
        assert!(!tool.should_defer());
    }

    #[test]
    fn test_notebook_formatting() {
        let notebook = serde_json::json!({
            "cells": [
                {
                    "cell_type": "markdown",
                    "source": ["# Hello\n", "This is a notebook"]
                },
                {
                    "cell_type": "code",
                    "source": ["x = 42\nprint(x)"],
                    "outputs": [
                        {
                            "output_type": "stream",
                            "text": ["42\n"]
                        }
                    ]
                }
            ],
            "metadata": {
                "kernelspec": {
                    "display_name": "Python 3"
                }
            }
        });

        let json_str = serde_json::to_string(&notebook).unwrap();
        let result = format_notebook(&json_str, std::path::Path::new("test.ipynb"));
        assert!(result.contains("2 cells"));
        assert!(result.contains("Python 3"));
        assert!(result.contains("# Hello"));
        assert!(result.contains("x = 42"));
        assert!(result.contains("42"));
        assert!(result.contains("[code]"));
        assert!(result.contains("[markdown]"));
    }

    #[test]
    fn test_notebook_error_output() {
        let notebook = serde_json::json!({
            "cells": [
                {
                    "cell_type": "code",
                    "source": ["1/0"],
                    "outputs": [
                        {
                            "output_type": "error",
                            "ename": "ZeroDivisionError",
                            "evalue": "division by zero"
                        }
                    ]
                }
            ],
            "metadata": {}
        });

        let json_str = serde_json::to_string(&notebook).unwrap();
        let result = format_notebook(&json_str, std::path::Path::new("test.ipynb"));
        assert!(result.contains("ZeroDivisionError"));
        assert!(result.contains("division by zero"));
    }
}

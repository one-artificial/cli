//! Provider implementations.
//!
//! - `openai_compat`: Shared base for OpenAI-compatible APIs (OpenAI, HuggingFace, LM Studio)
//! - `tool_shim`: Prompt-based tool use for providers without native support (Ollama)
//! - Anthropic has its own implementation in `../anthropic.rs` (unique format)

pub mod openai_compat;
pub mod tool_shim;

pub use openai_compat::{OpenAiCompatConfig, OpenAiCompatProvider};
pub use tool_shim::ToolShimProvider;

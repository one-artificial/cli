use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Application configuration, loaded from ~/.one/config.toml
/// with env var overrides and CLI flag overrides on top.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub provider: ProviderConfig,

    #[serde(default)]
    pub ui: UiConfig,

    #[serde(default)]
    pub integrations: IntegrationConfig,

    #[serde(default)]
    pub pet: PetConfig,

    /// Whether the user has completed first-run onboarding.
    #[serde(default)]
    pub has_completed_onboarding: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Default provider: "anthropic", "openai", "ollama", "google"
    pub default_provider: String,
    /// Default model for the default provider
    pub default_model: String,
    /// Max tokens for responses
    pub max_tokens: u32,
    /// Fast mode: same model with faster streaming output
    #[serde(default)]
    pub fast_mode: Option<bool>,

    #[serde(default)]
    pub anthropic: ProviderAuth,
    #[serde(default)]
    pub openai: ProviderAuth,
    #[serde(default)]
    pub google: ProviderAuth,
    #[serde(default)]
    pub ollama: OllamaConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderAuth {
    /// API key (prefer env var ANTHROPIC_API_KEY etc.)
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OllamaConfig {
    /// Base URL for Ollama (default: http://localhost:11434)
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    /// Show line numbers in file displays
    pub line_numbers: bool,
    /// Theme preset: "dark" (default), "light", "custom"
    pub theme: String,
    /// Custom theme colors (used when theme = "custom")
    #[serde(default)]
    pub colors: ThemeColors,
}

/// Customizable TUI colors. Each field accepts a color name
/// (black, red, green, yellow, blue, magenta, cyan, white, gray)
/// or a hex value (#RRGGBB).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeColors {
    /// User input text color
    pub user_text: String,
    /// AI response text color
    pub assistant_text: String,
    /// Tool call display color
    pub tool_call: String,
    /// Error message color
    pub error: String,
    /// Border/frame color
    pub border: String,
    /// Active tab/selection highlight
    pub highlight: String,
    /// Muted/secondary text
    pub muted: String,
    /// Diff addition color
    pub diff_add: String,
    /// Diff deletion color
    pub diff_remove: String,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self::dark()
    }
}

impl ThemeColors {
    /// Dark theme (default)
    pub fn dark() -> Self {
        Self {
            user_text: "white".into(),
            assistant_text: "white".into(),
            tool_call: "cyan".into(),
            error: "red".into(),
            border: "gray".into(),
            highlight: "cyan".into(),
            muted: "gray".into(),
            diff_add: "green".into(),
            diff_remove: "red".into(),
        }
    }

    /// Light theme
    pub fn light() -> Self {
        Self {
            user_text: "black".into(),
            assistant_text: "black".into(),
            tool_call: "blue".into(),
            error: "red".into(),
            border: "gray".into(),
            highlight: "blue".into(),
            muted: "gray".into(),
            diff_add: "green".into(),
            diff_remove: "red".into(),
        }
    }

    /// Get the appropriate colors for a theme preset.
    pub fn for_theme(name: &str) -> Self {
        match name {
            "light" => Self::light(),
            "custom" => Self::default(), // custom uses whatever's in the config
            _ => Self::dark(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IntegrationConfig {
    #[serde(default)]
    pub github: GitHubConfig,
    #[serde(default)]
    pub slack: SlackConfig,
    #[serde(default)]
    pub asana: AsanaConfig,
    #[serde(default)]
    pub notion: NotionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GitHubConfig {
    pub token: Option<String>,
    #[serde(default)]
    pub repos: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlackConfig {
    pub token: Option<String>,
    #[serde(default)]
    pub channels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AsanaConfig {
    pub token: Option<String>,
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotionConfig {
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PetConfig {
    /// Pet's name
    pub name: String,
    /// Pet species/type for ASCII art selection
    pub species: String,
    /// Whether the pet is enabled
    pub enabled: bool,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            // No default provider — onboarding asks users to connect one.
            // This prevents assuming Anthropic and treats all providers as integrations.
            default_provider: String::new(),
            default_model: String::new(),
            max_tokens: 8000,
            fast_mode: None,
            anthropic: ProviderAuth::default(),
            openai: ProviderAuth::default(),
            google: ProviderAuth::default(),
            ollama: OllamaConfig::default(),
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            line_numbers: true,
            theme: "dark".to_string(),
            colors: ThemeColors::default(),
        }
    }
}

impl Default for PetConfig {
    fn default() -> Self {
        Self {
            name: "Pixel".to_string(),
            species: "duck".to_string(),
            enabled: true,
        }
    }
}

impl AppConfig {
    /// Load config from ~/.one/config.toml, creating defaults if missing.
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path();

        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)?;
            let config: AppConfig = toml::from_str(&contents)?;
            Ok(config)
        } else {
            // Create default config
            let config = AppConfig::default();
            config.save()?;
            Ok(config)
        }
    }

    /// Save current config to disk.
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path();

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents = toml::to_string_pretty(self)?;
        std::fs::write(&config_path, contents)?;
        Ok(())
    }

    /// Get the API key for a provider, checking config then env vars.
    pub fn api_key_for(&self, provider: &str) -> String {
        match provider {
            "anthropic" => self
                .provider
                .anthropic
                .api_key
                .clone()
                .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
                .unwrap_or_default(),
            "openai" => self
                .provider
                .openai
                .api_key
                .clone()
                .or_else(|| std::env::var("OPENAI_API_KEY").ok())
                .unwrap_or_default(),
            "google" => self
                .provider
                .google
                .api_key
                .clone()
                .or_else(|| std::env::var("GOOGLE_API_KEY").ok())
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn config_path() -> PathBuf {
        dirs_next::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".one")
            .join("config.toml")
    }
}

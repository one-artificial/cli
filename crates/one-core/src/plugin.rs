use serde::{Deserialize, Serialize};

/// Plugin system architecture for One.
///
/// Plugins extend One with custom tools, commands, and integrations.
/// Three plugin types are supported:
///
/// 1. **Built-in** — compiled into the binary (Rust traits)
/// 2. **Script** — shell scripts in ~/.one/plugins/ that expose commands
/// 3. **WASM** — WebAssembly modules loaded via extism (future)
///
/// The plugin manifest (plugin.toml) describes what a plugin provides.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: Option<String>,
    pub plugin_type: PluginType,
    /// Commands this plugin adds (e.g., /deploy, /lint)
    #[serde(default)]
    pub commands: Vec<PluginCommand>,
    /// Tools this plugin adds to the AI's toolset
    #[serde(default)]
    pub tools: Vec<String>,
    /// Event hooks this plugin listens to
    #[serde(default)]
    pub hooks: Vec<PluginHook>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginType {
    /// Shell script plugin
    Script { entrypoint: String },
    /// WASM module plugin (future)
    Wasm { module_path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCommand {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PluginHook {
    /// Called when a session starts
    SessionStart,
    /// Called before a tool is used
    PreToolUse,
    /// Called after a tool is used
    PostToolUse,
    /// Called when a response completes
    PostResponse,
}

/// Registry of loaded plugins.
#[derive(Debug, Default)]
pub struct PluginRegistry {
    plugins: Vec<LoadedPlugin>,
}

#[derive(Debug)]
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub enabled: bool,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Discover plugins from ~/.one/plugins/
    pub fn discover() -> Self {
        let mut registry = Self::new();

        let plugin_dir = dirs_next::home_dir()
            .unwrap_or_default()
            .join(".one")
            .join("plugins");

        if !plugin_dir.exists() {
            return registry;
        }

        if let Ok(entries) = std::fs::read_dir(&plugin_dir) {
            for entry in entries.flatten() {
                let manifest_path = entry.path().join("plugin.toml");
                if manifest_path.exists()
                    && let Ok(contents) = std::fs::read_to_string(&manifest_path)
                    && let Ok(manifest) = toml::from_str::<PluginManifest>(&contents)
                {
                    tracing::info!("Discovered plugin: {}", manifest.name);
                    registry.plugins.push(LoadedPlugin {
                        manifest,
                        enabled: true,
                    });
                }
            }
        }

        registry
    }

    pub fn all(&self) -> &[LoadedPlugin] {
        &self.plugins
    }

    /// Get all commands provided by loaded plugins.
    pub fn commands(&self) -> Vec<(&str, &str)> {
        self.plugins
            .iter()
            .filter(|p| p.enabled)
            .flat_map(|p| {
                p.manifest
                    .commands
                    .iter()
                    .map(|c| (c.name.as_str(), c.description.as_str()))
            })
            .collect()
    }

    /// Get plugins that hook into a specific event.
    pub fn hooks_for(&self, hook: PluginHook) -> Vec<&LoadedPlugin> {
        self.plugins
            .iter()
            .filter(|p| p.enabled && p.manifest.hooks.contains(&hook))
            .collect()
    }
}

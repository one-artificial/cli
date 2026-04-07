use serde::{Deserialize, Serialize};

/// Agent routing reduces context window usage by giving each specialized
/// agent only the tools it needs. The master coordinator decides which
/// agent to invoke based on the user's intent.
///
/// Instead of stuffing all tool schemas into every API call,
/// we route to focused agents with reduced tool sets.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    pub role: AgentRole,
    pub allowed_tools: Vec<String>,
    pub system_prompt_suffix: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentRole {
    /// Reads and analyzes code without modifications
    Reader,
    /// Makes changes to code and files
    Writer,
    /// Runs commands and manages processes
    Executor,
    /// Searches across the codebase
    Explorer,
    /// The master that routes to other agents
    Coordinator,
}

impl AgentRole {
    pub fn default_tools(&self) -> Vec<&'static str> {
        match self {
            AgentRole::Reader => vec!["file_read", "grep", "glob"],
            AgentRole::Writer => vec!["file_write", "file_edit"],
            AgentRole::Executor => vec!["bash"],
            AgentRole::Explorer => vec!["grep", "glob", "file_read"],
            AgentRole::Coordinator => vec![], // Coordinator uses no tools, only routes
        }
    }

    pub fn system_prompt(&self) -> &'static str {
        match self {
            AgentRole::Reader => {
                "You are a code reading specialist. You can read files, search code, \
                 and find patterns. You CANNOT modify files or run commands. \
                 Report your findings clearly."
            }
            AgentRole::Writer => {
                "You are a code writing specialist. You can create and edit files. \
                 Make precise, minimal changes. Always explain what you changed and why."
            }
            AgentRole::Executor => {
                "You are a command execution specialist. You run shell commands \
                 and report their output. Be careful with destructive commands."
            }
            AgentRole::Explorer => {
                "You are a codebase exploration specialist. You find files, \
                 search patterns, and map project structure. Report your findings \
                 in a structured way."
            }
            AgentRole::Coordinator => {
                "You are the coordinator. Analyze the user's request and decide \
                 which specialist agent should handle it. You do not execute tools \
                 directly — you delegate to the right agent."
            }
        }
    }
}

/// Registry of available agents with their tool assignments.
#[derive(Debug, Clone)]
pub struct AgentRegistry {
    agents: Vec<AgentDefinition>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self { agents: Vec::new() }
    }

    /// Create the default set of agents.
    pub fn with_defaults() -> Self {
        let mut reg = Self::new();

        reg.register(AgentDefinition {
            name: "reader".to_string(),
            description: "Reads and analyzes code".to_string(),
            role: AgentRole::Reader,
            allowed_tools: AgentRole::Reader
                .default_tools()
                .into_iter()
                .map(String::from)
                .collect(),
            system_prompt_suffix: AgentRole::Reader.system_prompt().to_string(),
        });

        reg.register(AgentDefinition {
            name: "writer".to_string(),
            description: "Creates and edits files".to_string(),
            role: AgentRole::Writer,
            allowed_tools: AgentRole::Writer
                .default_tools()
                .into_iter()
                .map(String::from)
                .collect(),
            system_prompt_suffix: AgentRole::Writer.system_prompt().to_string(),
        });

        reg.register(AgentDefinition {
            name: "executor".to_string(),
            description: "Runs shell commands".to_string(),
            role: AgentRole::Executor,
            allowed_tools: AgentRole::Executor
                .default_tools()
                .into_iter()
                .map(String::from)
                .collect(),
            system_prompt_suffix: AgentRole::Executor.system_prompt().to_string(),
        });

        reg.register(AgentDefinition {
            name: "explorer".to_string(),
            description: "Searches and maps the codebase".to_string(),
            role: AgentRole::Explorer,
            allowed_tools: AgentRole::Explorer
                .default_tools()
                .into_iter()
                .map(String::from)
                .collect(),
            system_prompt_suffix: AgentRole::Explorer.system_prompt().to_string(),
        });

        reg
    }

    pub fn register(&mut self, agent: AgentDefinition) {
        self.agents.push(agent);
    }

    pub fn get(&self, name: &str) -> Option<&AgentDefinition> {
        self.agents.iter().find(|a| a.name == name)
    }

    pub fn all(&self) -> &[AgentDefinition] {
        &self.agents
    }

    /// Build a coordinator system prompt that describes the available agents
    /// so the AI knows how to route.
    pub fn coordinator_prompt(&self) -> String {
        let mut prompt = String::from(
            "You are the coordinator. Route the user's request to the most appropriate \
             specialist agent. Available agents:\n\n",
        );

        for agent in &self.agents {
            prompt.push_str(&format!(
                "- **{}**: {} (tools: {})\n",
                agent.name,
                agent.description,
                agent.allowed_tools.join(", ")
            ));
        }

        prompt.push_str(
            "\nRespond with the agent name to route to, or handle simple \
             questions yourself without routing.",
        );

        prompt
    }

    /// Filter tool schemas to only those allowed by a specific agent.
    pub fn filter_schemas(
        &self,
        agent_name: &str,
        all_schemas: &[serde_json::Value],
    ) -> Vec<serde_json::Value> {
        let Some(agent) = self.get(agent_name) else {
            return all_schemas.to_vec();
        };

        all_schemas
            .iter()
            .filter(|schema| {
                schema["name"]
                    .as_str()
                    .is_some_and(|name| agent.allowed_tools.contains(&name.to_string()))
            })
            .cloned()
            .collect()
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Wire protocol spoken by a provider endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Protocol {
    /// POST {base_url}/chat/completions (OpenAI, xAI, Groq, DeepSeek, Ollama, ...)
    #[default]
    #[serde(rename = "openai-chat")]
    OpenAiChat,
    /// POST {base_url}/responses (OpenAI Responses API)
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    /// POST {base_url}/v1/messages (Anthropic)
    #[serde(rename = "anthropic")]
    Anthropic,
}

impl Protocol {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "openai-chat" | "openai" | "chat" => Ok(Self::OpenAiChat),
            "openai-responses" | "responses" => Ok(Self::OpenAiResponses),
            "anthropic" | "messages" => Ok(Self::Anthropic),
            other => Err(Error::Config(format!(
                "unknown protocol '{other}' (expected openai-chat, openai-responses or anthropic)"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    /// Ask before every mutating/tool call.
    #[default]
    Ask,
    /// Run everything without asking.
    Allow,
    /// Block all tools.
    Deny,
}

/// A configured provider endpoint. `name` is the map key chosen by the user.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    /// Base URL, e.g. https://api.x.ai/v1
    #[serde(default)]
    pub base_url: String,
    /// API key: literal value or "$ENV_VAR_NAME".
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub protocol: Protocol,
    /// Extra headers appended to every request.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Extra query params appended to every request.
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    /// Optional path prefix inserted between base_url and endpoint
    /// (e.g. "/v1" when base_url has none). Empty = none.
    #[serde(default)]
    pub path_prefix: Option<String>,
}

/// A saved model with a user-chosen display name / alias.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Model id sent to the provider, e.g. "grok-4".
    pub id: String,
    /// Provider name (key into providers map).
    pub provider: String,
    /// Human display name; defaults to id.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Short alias for quick switching (`morrow -m fast`).
    #[serde(default)]
    pub alias: Option<String>,
    /// Context window in tokens (used by compaction).
    #[serde(default)]
    pub context_window: Option<u64>,
    /// Default max output tokens.
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub temperature: Option<f64>,
}

impl ModelConfig {
    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.id)
    }

    pub fn matches(&self, key: &str) -> bool {
        key == self.id
            || self.alias.as_deref() == Some(key)
            || self.label() == key
            || format!("{}/{}", self.provider, self.id) == key
            || self
                .alias
                .as_deref()
                .map(|a| format!("{}/{}", self.provider, a))
                == Some(key.to_string())
    }
}

/// A subagent. Built-ins exist; users define more in config.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentConfig {
    /// System prompt for the agent.
    #[serde(default)]
    pub prompt: String,
    /// Model key this agent uses ("alias", "provider/model" or raw id).
    /// None = inherit current session model.
    #[serde(default)]
    pub model: Option<String>,
    /// Tools allowed for this agent. None = all tools.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Description shown to the primary model when picking an agent.
    #[serde(default)]
    pub description: Option<String>,
    /// Hide from the task-tool listing (e.g. internal agents).
    #[serde(default)]
    pub hidden: bool,
}

/// One MCP (Model Context Protocol) stdio server. Its tools appear as
/// `mcp_<server>_<tool>` and go through the normal permission gate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Executable, e.g. "npx" or "/usr/local/bin/my-mcp".
    pub command: String,
    /// Args, e.g. ["-y", "@modelcontextprotocol/server-filesystem", "/proj"].
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment for the server process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Set false to keep a configured server disconnected.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Per-request timeout in seconds (default 60).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

fn default_true() -> bool {
    true
}

/// Root configuration file schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Default model key (alias / provider/model / id).
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub permission: PermissionMode,
    /// Per-tool overrides: tool name → ask | allow | deny.
    /// Beats the character mode; `deny` always wins, even in autonomous mode.
    #[serde(default)]
    pub permissions: BTreeMap<String, PermissionMode>,
    /// Bash guardrails: glob patterns always denied, e.g. "rm -rf /*".
    /// Matched against the `command` argument (`*` = any sequence).
    #[serde(default)]
    pub bash_deny: Vec<String>,
    /// MCP stdio servers, keyed by short name: `[mcp_servers.<name>]`.
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Saved models keyed by alias-or-id.
    #[serde(default)]
    pub models: BTreeMap<String, ModelConfig>,
    /// Subagents keyed by name.
    #[serde(default)]
    pub agents: BTreeMap<String, AgentConfig>,
    /// Bash tool timeout in seconds.
    #[serde(default = "default_bash_timeout")]
    pub bash_timeout_secs: u64,
    /// Compact the conversation when it exceeds this fraction of context window.
    #[serde(default = "default_compact_threshold")]
    pub compact_threshold: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_model: None,
            permission: PermissionMode::Ask,
            permissions: Default::default(),
            bash_deny: Default::default(),
            mcp_servers: Default::default(),
            providers: Default::default(),
            models: Default::default(),
            agents: Default::default(),
            bash_timeout_secs: default_bash_timeout(),
            compact_threshold: default_compact_threshold(),
        }
    }
}

fn default_bash_timeout() -> u64 {
    120
}
fn default_compact_threshold() -> f64 {
    0.85
}

impl Config {
    /// Built-in defaults: stock subagents, no providers/models (user adds those).
    pub fn builtin() -> Self {
        let mut cfg = Self::default();
        cfg.agents.insert(
            "general".into(),
            AgentConfig {
                prompt: "You are a capable general-purpose coding agent. Research complex questions, \
                         search code and execute multi-step tasks. When searching for text or files, \
                         prefer broad, fast queries in parallel."
                    .into(),
                description: Some("General-purpose agent for researching and multi-step tasks".into()),
                ..Default::default()
            },
        );
        cfg.agents.insert(
            "explore".into(),
            AgentConfig {
                prompt: "You are a read-only codebase exploration expert. Answer questions about the \
                         codebase quickly with specific file paths and line numbers. Do not modify files."
                    .into(),
                tools: Some(vec!["read".into(), "grep".into(), "glob".into(), "ls".into(), "bash".into()]),
                description: Some("Fast read-only agent for answering codebase questions".into()),
                ..Default::default()
            },
        );
        cfg.agents.insert(
            "plan".into(),
            AgentConfig {
                prompt:
                    "You are an expert software architect. Produce a concise implementation plan: \
                         steps to take, files to change, risks and tradeoffs. Do not write code."
                        .into(),
                tools: Some(vec![
                    "read".into(),
                    "grep".into(),
                    "glob".into(),
                    "ls".into(),
                    "bash".into(),
                ]),
                description: Some("Read-only agent that designs implementation plans".into()),
                ..Default::default()
            },
        );
        cfg
    }

    pub fn global_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("morrow")
            .join("config.toml")
    }

    /// Project-local config: `.morrow/config.toml` (one dot-dir per project,
    /// Codex/Claude style — sits next to sessions/, MEMORY.md, state.json).
    pub fn project_path() -> PathBuf {
        PathBuf::from(".morrow").join("config.toml")
    }

    /// Load global + project config merged (project wins per-key).
    pub fn load() -> Result<Self> {
        let mut cfg = Self::builtin();
        for path in [Self::global_path(), Self::project_path()] {
            if path.exists() {
                let raw = std::fs::read_to_string(&path)?;
                let parsed: Self = toml::from_str(&raw)?;
                cfg.merge(parsed);
            }
        }
        Ok(cfg)
    }

    /// Load from an explicit file (for tests / --config flag).
    pub fn load_from(path: &Path) -> Result<Self> {
        let mut cfg = Self::builtin();
        if path.exists() {
            let raw = std::fs::read_to_string(path)?;
            let parsed: Self = toml::from_str(&raw)?;
            cfg.merge(parsed);
        }
        Ok(cfg)
    }

    fn merge(&mut self, other: Self) {
        if other.default_model.is_some() {
            self.default_model = other.default_model;
        }
        self.permission = other.permission;
        self.permissions.extend(other.permissions);
        if !other.bash_deny.is_empty() {
            self.bash_deny = other.bash_deny;
        }
        self.mcp_servers.extend(other.mcp_servers);
        self.providers.extend(other.providers);
        self.models.extend(other.models);
        self.agents.extend(other.agents);
        if other.bash_timeout_secs != default_bash_timeout() {
            self.bash_timeout_secs = other.bash_timeout_secs;
        }
        self.compact_threshold = other.compact_threshold;
    }

    /// Resolve an api key that may be "$ENV_VAR".
    pub fn resolve_api_key(&self, provider: &ProviderConfig) -> Option<String> {
        let raw = provider.api_key.as_deref()?;
        Some(if let Some(var) = raw.strip_prefix('$') {
            std::env::var(var).unwrap_or_default()
        } else {
            raw.to_string()
        })
    }

    /// Find a saved model by alias/id/provider-model.
    pub fn find_model(&self, key: &str) -> Option<(String, ModelConfig)> {
        // exact key first
        if let Some(m) = self.models.get(key) {
            return Some((key.to_string(), m.clone()));
        }
        self.models
            .iter()
            .find(|(_, m)| m.matches(key))
            .map(|(k, m)| (k.clone(), m.clone()))
    }

    /// Write a starter config documenting every supported provider setup.
    /// Everything is commented out so `init-config` is safe to run anytime —
    /// the user uncomments the block(s) they need.
    pub fn write_sample(path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            path,
            r#"# morrow configuration
# Uncomment the block(s) you need, then `morrow` picks them up.
# Protocols: openai-chat | openai-responses | anthropic
# api_key: "$ENV_VAR" (recommended) or a literal key. Omit for keyless local endpoints.
#
# default_model = "codex-mini"

# ---------------------------------------------------------------- OpenAI ---
# Chat Completions API (gpt-4o, gpt-4.1, o-series, ...).
# [providers.openai]
# base_url = "https://api.openai.com/v1"
# api_key = "$OPENAI_API_KEY"
# protocol = "openai-chat"
#
# [models.gpt]
# id = "gpt-4.1-mini"
# provider = "openai"
# display_name = "GPT 4.1 Mini"
# context_window = 1048576

# ------------------------------------------------------------ Codex --------
# Codex / Responses API (codex-mini-latest, gpt-5-codex, ...).
# Same host, different wire protocol.
# [providers.codex]
# base_url = "https://api.openai.com/v1"
# api_key = "$OPENAI_API_KEY"
# protocol = "openai-responses"
#
# [models.codex-mini]
# id = "codex-mini-latest"
# provider = "codex"
# display_name = "Codex Mini"
# context_window = 200000

# ---------------------------------------------------------- Anthropic ------
# Claude via the Messages API (Sonnet, Opus, Haiku, ...).
# [providers.claude]
# base_url = "https://api.anthropic.com"
# api_key = "$ANTHROPIC_API_KEY"
# protocol = "anthropic"
#
# [models.sonnet]
# id = "claude-sonnet-4-5"
# provider = "claude"
# display_name = "Sonnet"
# context_window = 200000

# --------------------------------------------------------------- xAI -------
# Grok models, OpenAI-compatible chat endpoint.
# [providers.grok]
# base_url = "https://api.x.ai/v1"
# api_key = "$XAI_API_KEY"
# protocol = "openai-chat"
#
# [models.grok-fast]
# id = "grok-4-1-fast-non-reasoning"
# provider = "grok"
# display_name = "Grok Fast"
# context_window = 2000000

# -------------------------------------------------------------- Groq -------
# [providers.groq]
# base_url = "https://api.groq.com/openai/v1"
# api_key = "$GROQ_API_KEY"
# protocol = "openai-chat"
#
# [models.groq-llama]
# id = "llama-3.3-70b-versatile"
# provider = "groq"
# context_window = 131072

# ----------------------------------------------------------- DeepSeek ------
# [providers.deepseek]
# base_url = "https://api.deepseek.com/v1"
# api_key = "$DEEPSEEK_API_KEY"
# protocol = "openai-chat"
#
# [models.deepseek-chat]
# id = "deepseek-chat"
# provider = "deepseek"
# context_window = 64000

# --------------------------------------------------------- OpenRouter ------
# Any model on OpenRouter (Anthropic + OpenAI + others through one key).
# Use the openai-chat protocol with "provider/model" ids.
# [providers.openrouter]
# base_url = "https://openrouter.ai/api/v1"
# api_key = "$OPENROUTER_API_KEY"
# protocol = "openai-chat"
#
# [providers.openrouter.headers]
# HTTP-Referer = "https://morrow.local"
# X-Title = "morrow"
#
# [models.or-sonnet]
# id = "anthropic/claude-sonnet-4-5"
# provider = "openrouter"
# context_window = 200000

# ------------------------------------------------------------- Ollama ------
# Local models, no key needed. Any OpenAI-compatible host works here.
# [providers.ollama]
# base_url = "http://localhost:11434/v1"
# protocol = "openai-chat"
#
# [models.local]
# id = "qwen3:8b"
# provider = "ollama"
# context_window = 32768

# ------------------------------------------------------------- Custom ------
# Template for any other OpenAI-compatible endpoint (vLLM, LM Studio,
# Together, Fireworks, Mistral, ...). Optional path_prefix / headers / query.
# [providers.my-provider]
# base_url = "https://my-endpoint.example.com/v1"
# api_key = "$MY_API_KEY"
# protocol = "openai-chat"         # or "openai-responses" / "anthropic"
# path_prefix = "/v1"              # optional, appended before the endpoint path
# [providers.my-provider.headers]  # optional extra headers
# X-Custom = "value"
# [providers.my-provider.query]    # optional extra query params
# version = "2024-01-01"
#
# [models.fast]
# id = "llama-4-scout"
# provider = "my-provider"
# display_name = "Scout"
# context_window = 131072
# max_tokens = 4096
# temperature = 0.7

# ----------------------------------------------------------- Subagents -----
# [mcp_servers]                  # external tools via MCP (stdio)
# [mcp_servers.files]            # tools appear as mcp_files_<tool>
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/sam/projects"]
# enabled = true
# timeout_secs = 60
#
# [permissions]                    # per-tool overrides (beat character mode)
# edit = "ask"                     # ask | allow | deny — deny wins everywhere
# bash = "ask"
# read = "allow"
# bash_deny = ["rm -rf /*", "git push --force*", "shutdown *"]  # always blocked
#
# [agents.reviewer]                # custom subagent
# prompt = "You are a strict code reviewer."
# model = "fast"                   # optional different model
# tools = ["read", "grep", "glob"] # optional restricted toolset
"#,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let raw = r#"
default_model = "fast"
permission = "allow"

[providers.grok]
base_url = "https://api.x.ai/v1"
api_key = "$XAI_API_KEY"
protocol = "openai-chat"

[models.fast]
id = "grok-3-mini"
provider = "grok"
alias = "fast"
display_name = "Grok Mini"
context_window = 131072

[agents.explore]
prompt = "explore things"
tools = ["read", "grep"]
"#;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert_eq!(cfg.default_model.as_deref(), Some("fast"));
        assert_eq!(cfg.permission, PermissionMode::Allow);
        assert_eq!(cfg.models["fast"].id, "grok-3-mini");
        assert_eq!(cfg.agents["explore"].prompt, "explore things");
    }

    #[test]
    fn finds_model_by_alias_and_provider_id() {
        let raw = r#"
[providers.p]
base_url = "http://x"

[models.m1]
id = "model-a"
provider = "p"
alias = "a1"

[models.m2]
id = "model-b"
provider = "p"
"#;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert!(cfg.find_model("a1").is_some());
        assert!(cfg.find_model("p/model-a").is_some());
        assert!(cfg.find_model("model-b").is_some());
        assert!(cfg.find_model("nope").is_none());
    }

    #[test]
    fn protocol_parse() {
        assert_eq!(Protocol::parse("anthropic").unwrap(), Protocol::Anthropic);
        assert_eq!(Protocol::parse("openai").unwrap(), Protocol::OpenAiChat);
        assert!(Protocol::parse("bogus").is_err());
    }
}

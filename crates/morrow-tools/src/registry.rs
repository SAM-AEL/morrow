use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use morrow_core::Result;

/// Whether a tool can modify the world (used by the permission gate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    ReadOnly,
    Mutating,
}

/// Shared state handed to every tool invocation.
#[derive(Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub timeout_secs: u64,
    /// In-memory todo list shared across the session.
    pub todos: Arc<Mutex<Vec<crate::todo::TodoItem>>>,
    /// Shared handle to the active character (memory/integrations/chat live here).
    pub character: Option<Arc<Mutex<morrow_core::Character>>>,
}

impl ToolContext {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            timeout_secs: 120,
            todos: Arc::new(Mutex::new(Vec::new())),
            character: None,
        }
    }

    pub fn with_character(mut self, character: Arc<Mutex<morrow_core::Character>>) -> Self {
        self.character = Some(character);
        self
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// JSON schema for parameters.
    fn parameters(&self) -> serde_json::Value;
    fn kind(&self) -> ToolKind;
    async fn run(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<String>;

    /// JSON-schema tool spec for the LLM.
    fn spec(&self) -> morrow_provider::ToolSpec {
        morrow_provider::ToolSpec {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters(),
        }
    }
}

// ---- arg helpers -----------------------------------------------------------

pub(crate) fn str_arg<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| morrow_core::Error::Other(format!("missing string argument '{key}'")))
}

pub(crate) fn opt_str_arg<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

pub(crate) fn opt_u64_arg(args: &serde_json::Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}

pub(crate) fn resolve(ctx: &ToolContext, p: &str) -> PathBuf {
    let path = PathBuf::from(p);
    if path.is_absolute() {
        path
    } else {
        ctx.cwd.join(path)
    }
}

/// Collection of tools addressable by name.
pub struct Registry {
    tools: Vec<Box<dyn Tool>>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn add(&mut self, tool: impl Tool + 'static) -> &mut Self {
        self.tools.push(Box::new(tool));
        self
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|t| t.name()).collect()
    }

    /// Specs restricted to the given tool names (None = all).
    pub fn specs(&self, allowed: Option<&[String]>) -> Vec<morrow_provider::ToolSpec> {
        self.tools
            .iter()
            .filter(|t| match allowed {
                Some(names) => names.iter().any(|n| n == t.name()),
                None => true,
            })
            .map(|t| t.spec())
            .collect()
    }
}

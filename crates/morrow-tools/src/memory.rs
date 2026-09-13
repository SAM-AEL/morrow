use async_trait::async_trait;
use serde_json::json;

use crate::registry::{str_arg, Tool, ToolContext, ToolKind};

fn with_char<T>(
    ctx: &ToolContext,
    f: impl FnOnce(&mut morrow_core::Character) -> T,
) -> morrow_core::Result<T> {
    let cell = ctx
        .character
        .as_ref()
        .ok_or_else(|| morrow_core::Error::Other("no character attached to this session".into()))?;
    let mut guard = cell.lock().unwrap();
    guard.touch();
    Ok(f(&mut guard))
}

/// Character memory: append a durable learning.
pub struct Remember;

#[async_trait]
impl Tool for Remember {
    fn name(&self) -> &'static str {
        "remember"
    }

    fn description(&self) -> &'static str {
        "Save a durable fact/learning. scope=\"character\" (default) writes to YOUR OWN \
         persistent memory file; scope=\"project\" writes to the repo's shared \
         .morrow/MEMORY.md (conventions, decisions, lessons — safe to commit). Write compact, \
         self-contained facts."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "The fact to remember, one per call"},
                "scope": {"type": "string", "description": "\"character\" (default) or \"project\""}
            },
            "required": ["text"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Mutating
    }

    async fn run(
        &self,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> morrow_core::Result<String> {
        let text = str_arg(args, "text")?.to_string();
        let scope = args
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("character");
        if scope.eq_ignore_ascii_case("project") {
            let root = ctx.cwd.clone();
            let path = morrow_core::append_project_memory(&root, &text)?;
            return Ok(format!("saved to project memory ({path})"));
        }
        with_char(ctx, |c| {
            c.remember(text);
            format!(
                "remembered ({}): {} memories total",
                c.name,
                c.memories.len()
            )
        })
    }
}

/// Character memory recall.
pub struct Recall;

#[async_trait]
impl Tool for Recall {
    fn name(&self) -> &'static str {
        "recall"
    }

    fn description(&self) -> &'static str {
        "Search YOUR OWN persistent memory plus the repo's .morrow/MEMORY.md. \
         Empty query returns your most recent memories."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Case-insensitive substring; empty = latest"}
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::ReadOnly
    }

    async fn run(
        &self,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> morrow_core::Result<String> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let hits: Vec<String> = with_char(ctx, |c| {
            if query.trim().is_empty() {
                c.memories
                    .iter()
                    .rev()
                    .take(20)
                    .map(|m| format!("[{}] {}", m.at.format("%Y-%m-%d"), m.text))
                    .collect()
            } else {
                c.recall(&query)
                    .into_iter()
                    .map(|m| format!("[{}] {}", m.at.format("%Y-%m-%d"), m.text))
                    .collect()
            }
        })?;
        if hits.is_empty() {
            return Ok(format!("no memories matching '{query}'"));
        }
        let mut out = hits.join("\n");
        // Project memory grep (cheap substring scan, newest entries last).
        if !query.trim().is_empty() {
            let path = morrow_core::memory_path(&ctx.cwd);
            if let Ok(raw) = std::fs::read_to_string(&path) {
                let q = query.to_lowercase();
                let pm: Vec<String> = raw
                    .lines()
                    .filter(|l| l.starts_with("- ") && l.to_lowercase().contains(&q))
                    .map(|l| format!("[project] {l}"))
                    .collect();
                if !pm.is_empty() {
                    out.push_str("\n--- project memory ---\n");
                    out.push_str(&pm.join("\n"));
                }
            }
        }
        Ok(out)
    }
}

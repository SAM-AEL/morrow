use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use crate::registry::{resolve, str_arg, Tool, ToolContext, ToolKind};

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "glob"
    }

    fn description(&self) -> &'static str {
        "Find files matching a glob pattern (e.g. \"src/**/*.rs\", \"**/*.toml\"). \
         Returns up to 200 paths sorted by modification time (newest first)."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Glob pattern"},
                "path": {"type": "string", "description": "Base directory (default: project root)"}
            },
            "required": ["pattern"]
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
        let pattern = str_arg(args, "pattern")?.to_string();
        let base = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => resolve(ctx, p),
            None => ctx.cwd.clone(),
        };
        // glob crate wants the full pattern; join carefully for absolute/relative bases.
        let full: PathBuf = if PathBuf::from(&pattern).is_absolute() || base.as_os_str() == "." {
            PathBuf::from(&pattern)
        } else {
            base.join(&pattern)
        };

        let result = tokio::task::spawn_blocking(move || -> Result<Vec<PathBuf>, String> {
            let mut entries = Vec::new();
            let walker = glob::glob(&full.to_string_lossy())
                .map_err(|e| format!("bad pattern '{pattern}': {e}"))?;
            for entry in walker.flatten() {
                entries.push(entry);
                if entries.len() >= 500 {
                    break;
                }
            }
            Ok(entries)
        })
        .await
        .map_err(|e| morrow_core::Error::Other(e.to_string()))?
        .map_err(morrow_core::Error::Other)?;

        let mut scored: Vec<(std::time::SystemTime, PathBuf)> = result
            .into_iter()
            .filter_map(|p| {
                let m = std::fs::metadata(&p).ok()?;
                Some((m.modified().ok()?, p))
            })
            .collect();
        scored.sort_by_key(|e| std::cmp::Reverse(e.0));

        let shown: Vec<String> = scored
            .iter()
            .take(200)
            .map(|(_, p)| p.display().to_string())
            .collect();
        if shown.is_empty() {
            return Ok("no files matched".into());
        }
        let total = scored.len();
        let mut out = shown.join("\n");
        if total > 200 {
            out.push_str(&format!("\n... and {} more", total - 200));
        }
        Ok(out)
    }
}

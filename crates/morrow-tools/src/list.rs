use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use crate::registry::{resolve, Tool, ToolContext, ToolKind};

pub struct List;

#[async_trait]
impl Tool for List {
    fn name(&self) -> &'static str {
        "ls"
    }

    fn description(&self) -> &'static str {
        "List the contents of a directory with entry types."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Directory to list (default: project root)"}
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
        let dir: PathBuf = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => resolve(ctx, p),
            None => ctx.cwd.clone(),
        };
        let mut entries = vec![];
        let mut rd = tokio::fs::read_dir(&dir).await.map_err(|e| {
            morrow_core::Error::Other(format!("cannot list {}: {e}", dir.display()))
        })?;
        while let Some(entry) = rd.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            let ft = entry.file_type().await?;
            let suffix = if ft.is_dir() {
                "/"
            } else if ft.is_symlink() {
                "@"
            } else {
                ""
            };
            entries.push(format!("{name}{suffix}"));
        }
        entries.sort();
        if entries.is_empty() {
            Ok("(empty directory)".into())
        } else {
            Ok(entries.join("\n"))
        }
    }
}

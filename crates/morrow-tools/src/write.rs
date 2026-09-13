use async_trait::async_trait;
use serde_json::json;

use crate::registry::{resolve, str_arg, Tool, ToolContext, ToolKind};

pub struct Write;

#[async_trait]
impl Tool for Write {
    fn name(&self) -> &'static str {
        "write"
    }

    fn description(&self) -> &'static str {
        "Write (create or fully overwrite) a file. Parent directories are created automatically. \
         Prefer `edit` for modifying existing files."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path"},
                "content": {"type": "string", "description": "Full file content"}
            },
            "required": ["path", "content"]
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
        let path = resolve(ctx, str_arg(args, "path")?);
        let content = str_arg(args, "content")?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, content).await?;
        Ok(format!(
            "wrote {} ({} bytes)",
            path.display(),
            content.len()
        ))
    }
}

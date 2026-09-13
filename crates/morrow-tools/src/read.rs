use async_trait::async_trait;
use serde_json::json;

use crate::registry::{opt_u64_arg, resolve, str_arg, Tool, ToolContext, ToolKind};

const MAX_BYTES: u64 = 256 * 1024;

pub struct Read;

#[async_trait]
impl Tool for Read {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> &'static str {
        "Read a file with line numbers. Supports `offset` (1-based start line) and `limit`."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path (absolute or relative to project root)"},
                "offset": {"type": "integer", "description": "Start line (1-based)"},
                "limit": {"type": "integer", "description": "Max lines to return"}
            },
            "required": ["path"]
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
        let path = resolve(ctx, str_arg(args, "path")?);
        let meta = tokio::fs::metadata(&path).await.map_err(|e| {
            morrow_core::Error::Other(format!("cannot read {}: {e}", path.display()))
        })?;
        if meta.is_dir() {
            return Err(morrow_core::Error::Other(format!(
                "{} is a directory",
                path.display()
            )));
        }
        if meta.len() > MAX_BYTES {
            return Err(morrow_core::Error::Other(format!(
                "{} too large ({}, limit {} bytes)",
                path.display(),
                meta.len(),
                MAX_BYTES
            )));
        }
        let raw = tokio::fs::read(&path).await?;
        let content = String::from_utf8_lossy(&raw);
        let offset = opt_u64_arg(args, "offset").unwrap_or(1).max(1) as usize;
        let limit = opt_u64_arg(args, "limit").unwrap_or(2000) as usize;

        let mut out = String::new();
        for (i, line) in content.lines().enumerate().skip(offset - 1).take(limit) {
            out.push_str(&format!("{:>6}: {}\n", i + 1, line));
            if out.len() > MAX_BYTES as usize {
                out.push_str("... (truncated)\n");
                break;
            }
        }
        if out.is_empty() {
            out = "(empty file or past end of file)".into();
        }
        Ok(out)
    }
}

use async_trait::async_trait;
use serde_json::json;

use crate::registry::{resolve, str_arg, Tool, ToolContext, ToolKind};

const CONTEXT_LINES: usize = 3;

pub struct Edit;

#[async_trait]
impl Tool for Edit {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        "Exact-match text replacement in a file. `old` must appear exactly once unless \
         `replace_all` is true. Fails with context if not found or ambiguous."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File to edit"},
                "old": {"type": "string", "description": "Exact text to replace"},
                "new": {"type": "string", "description": "Replacement text"},
                "replace_all": {"type": "boolean", "description": "Replace every occurrence (default false)"}
            },
            "required": ["path", "old", "new"]
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
        let old = str_arg(args, "old")?;
        let new = str_arg(args, "new")?;
        let replace_all = args
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let raw = tokio::fs::read_to_string(&path).await.map_err(|e| {
            morrow_core::Error::Other(format!("cannot read {}: {e}", path.display()))
        })?;

        let count = raw.matches(old).count();
        if count == 0 {
            return Err(morrow_core::Error::Other(format!(
                "old text not found in {}. Nearby lines for context:\n{}",
                path.display(),
                nearby_context(&raw, old)
            )));
        }
        if count > 1 && !replace_all {
            return Err(morrow_core::Error::Other(format!(
                "'old' matches {count} locations; provide more surrounding text or set replace_all=true"
            )));
        }
        if old == new {
            return Err(morrow_core::Error::Other(
                "old and new are identical".into(),
            ));
        }

        let updated = if replace_all {
            raw.replace(old, new)
        } else {
            raw.replacen(old, new, 1)
        };
        tokio::fs::write(&path, &updated).await?;
        Ok(format!(
            "edited {} ({} replacement{})",
            path.display(),
            if replace_all { count } else { 1 },
            if replace_all && count > 1 { "s" } else { "" }
        ))
    }
}

fn nearby_content(haystack: &str, needle: &str) -> String {
    // find best-effort partial match location
    let first_word = needle.split_whitespace().next().unwrap_or("");
    if let Some(pos) = haystack.find(first_word) {
        let start = haystack[..pos]
            .lines()
            .count()
            .saturating_sub(CONTEXT_LINES);
        haystack
            .lines()
            .skip(start)
            .take(CONTEXT_LINES * 2 + needle.lines().count())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    }
}

fn nearby_context(haystack: &str, needle: &str) -> String {
    let c = nearby_content(haystack, needle);
    if c.is_empty() {
        "(file has no similar content)".into()
    } else {
        c
    }
}

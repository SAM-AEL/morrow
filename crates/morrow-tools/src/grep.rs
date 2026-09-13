use async_trait::async_trait;
use ignore::{WalkBuilder, WalkState};
use serde_json::json;
use std::path::PathBuf;
use std::sync::mpsc;

use crate::registry::{opt_u64_arg, resolve, str_arg, Tool, ToolContext, ToolKind};

const MAX_MATCHES: usize = 200;

pub struct Grep;

#[async_trait]
impl Tool for Grep {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> &'static str {
        "Regex search over files (respects .gitignore). Returns `path:line: text` matches. \
         Use `include` to filter by glob like \"*.rs\"."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Rust-flavored regex"},
                "path": {"type": "string", "description": "Directory or file to search (default: project root)"},
                "include": {"type": "string", "description": "Glob filter, e.g. \"*.ts\""}
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
        let target = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => resolve(ctx, p),
            None => ctx.cwd.clone(),
        };
        let include = args
            .get("include")
            .and_then(|v| v.as_str())
            .map(String::from);
        let max = opt_u64_arg(args, "limit").unwrap_or(MAX_MATCHES as u64) as usize;

        tokio::task::spawn_blocking(move || search(pattern, target, include, max))
            .await
            .map_err(|e| morrow_core::Error::Other(e.to_string()))?
    }
}

fn search(
    pattern: String,
    target: PathBuf,
    include: Option<String>,
    max: usize,
) -> morrow_core::Result<String> {
    let re = regex::Regex::new(&pattern)
        .map_err(|e| morrow_core::Error::Other(format!("bad regex '{pattern}': {e}")))?;
    let glob_filter = include.and_then(|g| glob::Pattern::new(&g).ok());

    let (tx, rx) = mpsc::channel::<String>();
    let mut count = 0usize;

    let walker = WalkBuilder::new(&target)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .build_parallel();

    walker.run(|| {
        let tx = tx.clone();
        let re = re.clone();
        let gf = glob_filter.clone();
        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return WalkState::Continue;
            };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                return WalkState::Continue;
            }
            let path = entry.path().to_path_buf();
            if let Some(gp) = &gf {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if !gp.matches(&name) && !gp.matches_path(&path) {
                    return WalkState::Continue;
                }
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                return WalkState::Continue;
            };
            for (i, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    let rendered = format!("{}:{}: {}", path.display(), i + 1, line.trim());
                    if tx.send(rendered).is_err() {
                        return WalkState::Quit;
                    }
                }
            }
            WalkState::Continue
        })
    });
    drop(tx);

    let mut out = String::new();
    for msg in rx.iter() {
        if count >= max {
            out.push_str("... (result truncated)\n");
            break;
        }
        out.push_str(&msg);
        out.push('\n');
        count += 1;
    }
    if out.is_empty() {
        out = "no matches".into();
    }
    Ok(out)
}

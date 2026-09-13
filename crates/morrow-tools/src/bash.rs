use async_trait::async_trait;
use serde_json::json;
use std::process::Stdio;
use tokio::process::Command;

use crate::registry::{str_arg, Tool, ToolContext, ToolKind};

pub struct Bash;

#[async_trait]
impl Tool for Bash {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command in the project directory and return combined stdout+stderr. \
         Use for builds, tests, git and other CLI work. Commands time out after `timeout` seconds."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "The shell command to run"},
                "timeout": {"type": "integer", "description": "Timeout in seconds (default from config)"}
            },
            "required": ["command"]
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
        let command = str_arg(args, "command")?;
        let timeout = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(ctx.timeout_secs)
            .min(3600);

        let child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&ctx.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            // detach from our process group so Ctrl-C on morrow doesn't kill children
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| morrow_core::Error::Other(format!("failed to spawn: {e}")))?;

        // On timeout the future (and child) are dropped; kill_on_drop cleans up.
        let output = match tokio::time::timeout(
            std::time::Duration::from_secs(timeout),
            child.wait_with_output(),
        )
        .await
        {
            Ok(res) => res.map_err(|e| morrow_core::Error::Other(e.to_string()))?,
            Err(_) => {
                return Err(morrow_core::Error::Other(format!(
                    "command timed out after {timeout}s"
                )));
            }
        };

        let mut out = String::new();
        if !output.stdout.is_empty() {
            out.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        const MAX: usize = 100_000;
        if out.len() > MAX {
            let cut = out
                .char_indices()
                .nth(MAX / 2)
                .map(|(i, _)| i)
                .unwrap_or(MAX);
            out = format!(
                "{}\n...[truncated]...\n{}",
                &out[..cut],
                &out[out.len() - MAX / 4..]
            );
        }
        out.push_str(&format!(
            "\n(exit code: {})",
            output.status.code().unwrap_or(-1)
        ));
        Ok(out)
    }
}

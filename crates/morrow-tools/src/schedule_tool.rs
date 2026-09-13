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

/// Characters schedule their own future work — "run this at 12am",
/// "every day at 6pm", "in 2 hours" — parsed against device-local time.
pub struct ScheduleTool;

#[async_trait]
impl Tool for ScheduleTool {
    fn name(&self) -> &'static str {
        "schedule"
    }

    fn description(&self) -> &'static str {
        "Schedule tasks for yourself to run automatically in the future. Times are parsed \
         from natural language using the DEVICE's local time zone. Examples for `when`: \
         \"at 12am\", \"tomorrow at 9am\", \"every day at 6pm\", \"weekdays at 08:30\", \
         \"every monday at 9am\", \"in 2 hours\", \"hourly\". Recurring tasks repeat \
         automatically; one-shots run once."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["add", "list", "remove", "cancel"]},
                "when": {"type": "string", "description": "(add) human time expression"},
                "task": {"type": "string", "description": "(add) full self-contained prompt to run"},
                "id": {"type": "string", "description": "(remove/cancel) schedule id"}
            },
            "required": ["action"]
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
        let action = str_arg(args, "action")?;
        match action {
            "add" => {
                let when = args
                    .get("when")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| morrow_core::Error::Other("missing 'when'".into()))?;
                let task = args
                    .get("task")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| morrow_core::Error::Other("missing 'task'".into()))?;
                let sched = morrow_core::Schedule::new(when, task)?;
                with_char(ctx, |c| {
                    // keep bounded
                    if c.schedules.len() >= 100 {
                        c.schedules.remove(0);
                    }
                    let desc = sched.describe();
                    c.schedules.push(sched);
                    format!("scheduled ✓ {desc}\ntask: {task}")
                })
            }
            "list" => {
                let out = with_char(ctx, |c| {
                    if c.schedules.is_empty() {
                        return "(no scheduled tasks)".to_string();
                    }
                    c.schedules
                        .iter()
                        .map(|s| s.describe())
                        .collect::<Vec<_>>()
                        .join("\n")
                })?;
                Ok(out)
            }
            "remove" | "cancel" => {
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| morrow_core::Error::Other("missing 'id'".into()))?;
                with_char(ctx, |c| match c.schedules.iter().position(|s| s.id == id) {
                    Some(pos) => {
                        c.schedules.remove(pos);
                        Ok(format!("schedule '{id}' removed"))
                    }
                    None => Err(morrow_core::Error::Other(format!("no schedule '{id}'"))),
                })?
            }
            other => Err(morrow_core::Error::Other(format!(
                "unknown schedule action '{other}'"
            ))),
        }
    }
}

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::registry::{Tool, ToolContext, ToolKind};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TodoItem {
    pub content: String,
    pub status: String, // pending | in_progress | completed
}

pub struct Todo;

#[async_trait]
impl Tool for Todo {
    fn name(&self) -> &'static str {
        "todo"
    }

    fn description(&self) -> &'static str {
        "Maintain a task list for the current session. Pass the FULL updated list every time; \
         keep exactly one task `in_progress` while working."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {"type": "string"},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}
                        },
                        "required": ["content", "status"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::ReadOnly // bookkeeping only
    }

    async fn run(
        &self,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> morrow_core::Result<String> {
        let todos: Vec<TodoItem> = serde_json::from_value(args["todos"].clone())
            .map_err(|e| morrow_core::Error::Other(format!("bad todos payload: {e}")))?;
        *ctx.todos.lock().unwrap() = todos.clone();
        let mut out = String::from("todo list updated:\n");
        for t in &todos {
            out.push_str(&format!("  [{}] {}\n", t.status, t.content));
        }
        Ok(out)
    }
}

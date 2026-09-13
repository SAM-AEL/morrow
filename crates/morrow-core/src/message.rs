use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON string of arguments.
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Content {
    Text(String),
    ToolUse(Vec<ToolCall>),
    ToolResult {
        call_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Content>,
}

impl Message {
    pub fn text(role: Role, s: impl Into<String>) -> Self {
        Self {
            role,
            parts: vec![Content::Text(s.into())],
        }
    }

    pub fn assistant_tool_use(calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            parts: vec![Content::ToolUse(calls)],
        }
    }

    pub fn tool_result(
        call_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: Role::Tool,
            parts: vec![Content::ToolResult {
                call_id: call_id.into(),
                content: content.into(),
                is_error,
            }],
        }
    }

    pub fn plain_text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Content::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn has_tool_calls(&self) -> bool {
        self.parts.iter().any(|p| matches!(p, Content::ToolUse(_)))
    }
}

/// Usage stats for a single completion.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

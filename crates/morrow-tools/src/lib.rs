mod bash;
mod edit;
mod glob_tool;
mod grep;
mod integration;
mod list;
pub mod mcp;
pub mod memory;
mod read;
pub mod registry;
pub mod schedule_tool;
mod todo;
pub use todo::TodoItem;
mod webfetch;
mod write;

pub use mcp::{McpClient, McpTool};
pub use registry::{Registry, Tool, ToolContext, ToolKind};

/// Build a registry preloaded with every built-in tool.
pub fn builtin_registry() -> Registry {
    let mut r = Registry::new();
    r.add(bash::Bash);
    r.add(read::Read);
    r.add(write::Write);
    r.add(edit::Edit);
    r.add(glob_tool::GlobTool);
    r.add(grep::Grep);
    r.add(list::List);
    r.add(webfetch::WebFetch::new());
    r.add(todo::Todo);
    r.add(memory::Remember);
    r.add(memory::Recall);
    r.add(integration::IntegrationTool::new());
    r.add(schedule_tool::ScheduleTool);
    r
}

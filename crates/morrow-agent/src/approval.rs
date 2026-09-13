use async_trait::async_trait;

/// Decides whether a tool invocation may run.
#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    /// Return true to run the tool. Called for mutating tools in ask-mode.
    async fn approve(&self, tool: &str, arguments: &str) -> bool;
}

/// Approves everything (headless / trusted mode).
pub struct AutoApprove;

#[async_trait]
impl ApprovalHandler for AutoApprove {
    async fn approve(&self, _tool: &str, _arguments: &str) -> bool {
        true
    }
}

/// Denies everything (`permission = "deny"`).
pub struct AskAlways;

#[async_trait]
impl ApprovalHandler for AskAlways {
    async fn approve(&self, tool: &str, _arguments: &str) -> bool {
        tracing_stub(&format!("denied tool {tool}"));
        false
    }
}

fn tracing_stub(_s: &str) {}

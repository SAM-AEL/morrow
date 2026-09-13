use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

/// An approval the agent is waiting on; UI shows a modal and answers it.
pub struct PendingApproval {
    pub tool: String,
    pub arguments: String,
    reply: oneshot::Sender<bool>,
}

impl PendingApproval {
    pub fn answer(self, allow: bool) {
        let _ = self.reply.send(allow);
    }
}

/// morrow_agent::ApprovalHandler that routes requests into the TUI loop.
pub struct TuiApproval {
    tx: mpsc::UnboundedSender<PendingApproval>,
    rx: Mutex<Option<mpsc::UnboundedReceiver<PendingApproval>>>,
}

impl Default for TuiApproval {
    fn default() -> Self {
        Self::new()
    }
}

impl TuiApproval {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
        }
    }

    /// Take the receiving end (called once by the UI loop).
    pub fn take_receiver(&self) -> Option<mpsc::UnboundedReceiver<PendingApproval>> {
        self.rx.lock().unwrap().take()
    }
}

#[async_trait]
impl morrow_agent::ApprovalHandler for TuiApproval {
    async fn approve(&self, tool: &str, arguments: &str) -> bool {
        let (reply_tx, reply_rx) = oneshot::channel();
        let req = PendingApproval {
            tool: tool.to_string(),
            arguments: arguments.to_string(),
            reply: reply_tx,
        };
        if self.tx.send(req).is_err() {
            return false;
        }
        matches!(reply_rx.await, Ok(true))
    }
}

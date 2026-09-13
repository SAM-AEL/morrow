use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex as AsyncMutex};

use crate::registry::{Tool, ToolContext, ToolKind};

/// One tool advertised by an MCP server.
#[derive(Debug, Clone)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Line-delimited JSON-RPC client over a spawned MCP (stdio) server.
#[derive(Clone)]
pub struct McpClient {
    inner: Arc<McpInner>,
}

struct McpInner {
    server: String,
    stdin: AsyncMutex<Option<ChildStdin>>,
    _child: Mutex<Option<Child>>,
    next_id: AtomicU64,
    waiters: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
    write_lock: AsyncMutex<()>,
    timeout_secs: u64,
}

impl McpClient {
    /// Spawn the server, run `initialize`, and return the client + tool list.
    pub async fn spawn(
        server: &str,
        cfg: &morrow_core::McpServerConfig,
        cwd: &Path,
    ) -> morrow_core::Result<(Self, Vec<McpToolDef>)> {
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args).envs(&cfg.env).current_dir(cwd);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| morrow_core::Error::Other(format!("mcp '{server}' spawn failed: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| morrow_core::Error::Other(format!("mcp '{server}' has no stdin")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| morrow_core::Error::Other(format!("mcp '{server}' has no stdout")))?;

        let client = Self {
            inner: Arc::new(McpInner {
                server: server.to_string(),
                stdin: AsyncMutex::new(Some(stdin)),
                _child: Mutex::new(Some(child)),
                next_id: AtomicU64::new(1),
                waiters: Mutex::new(HashMap::new()),
                write_lock: AsyncMutex::new(()),
                timeout_secs: cfg.timeout_secs.unwrap_or(60).max(5),
            }),
        };

        // Pump stdout lines → waiting requests.
        let pump = client.clone();
        let mut lines = BufReader::new(stdout).lines();
        tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                pump.route_line(&line);
            }
            pump.fail_all("mcp server closed stdout");
        });

        // initialize + initialized notification
        let init: Value = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "morrow", "version": env!("CARGO_PKG_VERSION")},
                }),
            )
            .await?;
        let _ = init.get("result");
        client
            .notify("notifications/initialized", json!({}))
            .await?;

        let listed = client.request("tools/list", json!({})).await?;
        let tools = listed
            .get("result")
            .and_then(|r| r.get("tools"))
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|t| {
                Some(McpToolDef {
                    name: t.get("name")?.as_str()?.to_string(),
                    description: t
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string(),
                    input_schema: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or(json!({"type": "object"})),
                })
            })
            .collect();
        Ok((client, tools))
    }

    fn route_line(&self, line: &str) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let Some(id) = v.get("id").and_then(|i| i.as_u64()) else {
            return; // notification / server request — not ours
        };
        if let Some(tx) = self.inner.waiters.lock().unwrap().remove(&id) {
            let _ = tx.send(v);
        }
    }

    fn fail_all(&self, reason: &str) {
        let mut waiters = self.inner.waiters.lock().unwrap();
        for (_, tx) in waiters.drain() {
            let _ = tx.send(json!({"error": {"message": reason}}));
        }
    }

    async fn request(&self, method: &str, params: Value) -> morrow_core::Result<Value> {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.inner.waiters.lock().unwrap().insert(id, tx);
        let write_ok = {
            let _w = self.inner.write_lock.lock().await;
            let mut stdin = self.inner.stdin.lock().await;
            let Some(handle) = stdin.as_mut() else {
                self.inner.waiters.lock().unwrap().remove(&id);
                return Err(morrow_core::Error::Other(format!(
                    "mcp '{}' stdin closed",
                    self.inner.server
                )));
            };
            let payload = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
            let mut line = serde_json::to_string(&payload).unwrap_or_default();
            line.push('\n');
            let ok =
                handle.write_all(line.as_bytes()).await.is_ok() && handle.flush().await.is_ok();
            ok
        };
        if !write_ok {
            self.inner.waiters.lock().unwrap().remove(&id);
            return Err(morrow_core::Error::Other(format!(
                "mcp '{}' write failed",
                self.inner.server
            )));
        }
        let timeout = std::time::Duration::from_secs(self.inner.timeout_secs);
        let resp = tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| {
                morrow_core::Error::Other(format!(
                    "mcp '{}' request '{method}' timed out",
                    self.inner.server
                ))
            })?
            .map_err(|_| {
                morrow_core::Error::Other(format!(
                    "mcp '{}' request '{method}' dropped",
                    self.inner.server
                ))
            })?;
        if let Some(err) = resp.get("error") {
            return Err(morrow_core::Error::Other(format!(
                "mcp '{}' {method} error: {err}",
                self.inner.server
            )));
        }
        Ok(resp)
    }

    async fn notify(&self, method: &str, params: Value) -> morrow_core::Result<()> {
        let _w = self.inner.write_lock.lock().await;
        let mut stdin = self.inner.stdin.lock().await;
        let payload = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let mut line = serde_json::to_string(&payload).unwrap_or_default();
        line.push('\n');
        if let Some(handle) = stdin.as_mut() {
            let _ = handle.write_all(line.as_bytes()).await;
            let _ = handle.flush().await;
        }
        Ok(())
    }

    /// Call a server tool; returns rendered text (errors as text, not Err,
    /// so the model sees them as tool results).
    pub async fn call_tool(&self, name: &str, arguments: &Value) -> String {
        match self
            .request("tools/call", json!({"name": name, "arguments": arguments}))
            .await
        {
            Err(e) => format!("mcp error: {e}"),
            Ok(resp) => {
                let result = resp.get("result").cloned().unwrap_or(Value::Null);
                let is_error = result
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let mut parts = Vec::new();
                if let Some(items) = result.get("content").and_then(|c| c.as_array()) {
                    for item in items {
                        match item.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                                    parts.push(t.to_string());
                                }
                            }
                            Some(other) => parts.push(format!("[mcp {other} content omitted]")),
                            None => {}
                        }
                    }
                }
                if parts.is_empty() {
                    parts.push("(empty result)".to_string());
                }
                let text = parts.join("\n");
                if is_error {
                    format!("mcp tool error: {text}")
                } else {
                    text
                }
            }
        }
    }

    pub fn server_name(&self) -> &str {
        &self.inner.server
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_SERVER: &str = r#"
import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    mid, method = msg.get("id"), msg.get("method")
    if method == "initialize":
        resp = {"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {"name": "fake", "version": "0"}}}
    elif method == "tools/list":
        resp = {"jsonrpc": "2.0", "id": mid, "result": {"tools": [{"name": "echo", "description": "Echoes text", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}}]}}
    elif method == "tools/call":
        text = msg.get("params", {}).get("arguments", {}).get("text", "")
        resp = {"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": "echo:" + text}]}}
    else:
        continue
    sys.stdout.write(json.dumps(resp) + "\n")
    sys.stdout.flush()
"#;

    fn fake_config() -> morrow_core::McpServerConfig {
        morrow_core::McpServerConfig {
            command: "python3".into(),
            args: vec!["-c".into(), FAKE_SERVER.into()],
            env: Default::default(),
            enabled: true,
            timeout_secs: Some(10),
        }
    }

    #[tokio::test]
    async fn stdio_initialize_list_call_roundtrip() {
        let dir = std::env::temp_dir();
        let (client, tools) = McpClient::spawn("fake", &fake_config(), &dir)
            .await
            .expect("fake server should spawn");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        let out = client.call_tool("echo", &json!({"text": "hi"})).await;
        assert_eq!(out, "echo:hi");

        // Registry adapter naming + spec shape.
        let adapter = McpTool::new(client, tools.into_iter().next().unwrap());
        assert_eq!(adapter.name(), "mcp_fake_echo");
        assert!(adapter.description().contains("fake"));
    }
}

/// Registry adapter: exposes one MCP server tool as `mcp_<server>_<tool>`.
pub struct McpTool {
    client: McpClient,
    leaked_name: &'static str,
    leaked_description: &'static str,
    orig_name: String,
    schema: Value,
}

impl McpTool {
    pub fn adapter_name(server: &str, tool: &str) -> String {
        let clean = |s: &str| {
            s.chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect::<String>()
        };
        format!("mcp_{}_{}", clean(server), clean(tool))
    }

    pub fn new(client: McpClient, def: McpToolDef) -> Self {
        let adapter = Self::adapter_name(client.server_name(), &def.name);
        // Two small intentional leaks per server tool so registry accessors
        // can be &'static (one adapter set per Runner, bounded by config).
        let leaked_name: &'static str = Box::leak(adapter.into_boxed_str());
        let description = if def.description.is_empty() {
            format!("MCP tool {} from {}", def.name, client.server_name())
        } else {
            format!(
                "{} [mcp:{}]",
                def.description.chars().take(400).collect::<String>(),
                client.server_name()
            )
        };
        let leaked_description: &'static str = Box::leak(description.into_boxed_str());
        Self {
            client,
            leaked_name,
            leaked_description,
            orig_name: def.name,
            schema: def.input_schema,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &'static str {
        self.leaked_name
    }

    fn description(&self) -> &'static str {
        self.leaked_description
    }

    fn parameters(&self) -> Value {
        self.schema.clone()
    }

    fn kind(&self) -> ToolKind {
        // External side effects we can't classify: always approval-gated
        // in approval mode, allowed only in autonomous mode or via config.
        ToolKind::Mutating
    }

    async fn run(&self, args: &Value, _ctx: &ToolContext) -> morrow_core::Result<String> {
        Ok(self.client.call_tool(&self.orig_name, args).await)
    }
}

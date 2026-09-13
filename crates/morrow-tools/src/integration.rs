use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::json;

use crate::registry::{opt_str_arg, str_arg, Tool, ToolContext, ToolKind};

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

fn resolve_key(i: &morrow_core::Integration) -> Option<String> {
    i.api_key.as_ref().map(|raw| match raw.strip_prefix('$') {
        Some(var) => std::env::var(var).unwrap_or_default(),
        None => raw.clone(),
    })
}

/// The character's self-managed third-party API integrations:
/// add, list, remove and actually CALL them (autonomous API automation).
pub struct IntegrationTool {
    client: reqwest::Client,
}

impl Default for IntegrationTool {
    fn default() -> Self {
        Self::new()
    }
}

impl IntegrationTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent(concat!("morrow/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest"),
        }
    }
}

#[async_trait]
impl Tool for IntegrationTool {
    fn name(&self) -> &'static str {
        "integration"
    }

    fn description(&self) -> &'static str {
        "Manage YOUR third-party API integrations (stored in your own memory file): \
         `add` a new API endpoint with its key, `list` what you have, `remove` one, or \
         `call` one — performing real authenticated HTTP requests autonomously."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["add", "list", "remove", "call"]},
                "name": {"type": "string", "description": "Integration name (add/remove/call)"},
                "base_url": {"type": "string", "description": "(add) e.g. https://api.github.com"},
                "description": {"type": "string", "description": "(add) what this API is for"},
                "api_key": {"type": "string", "description": "(add) literal key or $ENV_VAR (recommended)"},
                "method": {"type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"], "description": "(call)"},
                "path": {"type": "string", "description": "(call) path appended to base_url"},
                "body": {"type": "string", "description": "(call) optional JSON request body"}
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
        let action = str_arg(args, "action")?.to_string();

        match action.as_str() {
            "add" => {
                let name = str_arg(args, "name")?.to_string();
                let base_url = str_arg(args, "base_url")?.to_string();
                if !base_url.starts_with("http") {
                    return Err(morrow_core::Error::Other("base_url must be http(s)".into()));
                }
                let integ = morrow_core::Integration {
                    description: opt_str_arg(args, "description").unwrap_or("").to_string(),
                    base_url,
                    api_key: opt_str_arg(args, "api_key").map(String::from),
                    headers: BTreeMap::new(),
                };
                with_char(ctx, |c| {
                    let masked = integ.masked_key();
                    c.integrations.insert(name.clone(), integ);
                    format!("integration '{name}' saved (key: {masked})")
                })
            }
            "remove" => {
                let name = str_arg(args, "name")?.to_string();
                with_char(ctx, |c| match c.integrations.remove(&name) {
                    Some(_) => Ok(format!("integration '{name}' removed")),
                    None => Err(morrow_core::Error::Other(format!(
                        "no integration named '{name}'"
                    ))),
                })?
            }
            "list" => {
                let lines: Vec<String> = with_char(ctx, |c| {
                    c.integrations
                        .iter()
                        .map(|(n, i)| {
                            format!(
                                "{n}: {} @ {} (key: {})",
                                i.description,
                                i.base_url,
                                i.masked_key()
                            )
                        })
                        .collect()
                })?;
                if lines.is_empty() {
                    return Ok("no integrations yet; add one with action=add".into());
                }
                Ok(lines.join("\n"))
            }
            "call" => {
                // snapshot integration config from shared state
                let (integ, name) = {
                    let name = str_arg(args, "name")?.to_string();
                    let integ = with_char(ctx, |c| c.integrations.get(&name).cloned())?
                        .ok_or_else(|| {
                            morrow_core::Error::Other(format!("no integration named '{name}'"))
                        })?;
                    (integ, name)
                };

                let method = opt_str_arg(args, "method").unwrap_or("GET").to_uppercase();
                let path = opt_str_arg(args, "path").unwrap_or("/").to_string();
                let body = opt_str_arg(args, "body");

                let url = format!("{}{}", integ.base_url.trim_end_matches('/'), path);
                let method_req = reqwest::Method::from_bytes(method.as_bytes())
                    .map_err(|e| morrow_core::Error::Other(format!("bad method: {e}")))?;
                let mut req = self.client.request(method_req, &url);
                if let Some(key) = resolve_key(&integ).filter(|k| !k.is_empty()) {
                    req = req.bearer_auth(key);
                }
                for (k, v) in &integ.headers {
                    req = req.header(k, v);
                }
                req = req.header("Accept", "application/json");
                if let Some(b) = body {
                    req = req
                        .header("Content-Type", "application/json")
                        .body(b.to_string());
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|e| morrow_core::Error::Other(format!("request failed: {e}")))?;
                let status = resp.status().as_u16();
                let text = resp.text().await.unwrap_or_default();
                const MAX: usize = 8000;
                let text = if text.len() > MAX {
                    format!("{}…[truncated]", &text[..MAX])
                } else {
                    text
                };
                let _ = name;
                Ok(format!("[{status}] {url}\n{text}"))
            }
            other => Err(morrow_core::Error::Other(format!(
                "unknown integration action '{other}'"
            ))),
        }
    }
}

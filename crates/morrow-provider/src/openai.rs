use async_trait::async_trait;
use morrow_core::config::ProviderConfig;
use morrow_core::message::{Content, Message, Role, Usage};
use morrow_core::{Error, Result};
use serde_json::{json, Value};

use super::{CompletionRequest, LlmProvider, SseParser, StreamEvent};

fn endpoint(cfg: &ProviderConfig, path: &str) -> String {
    let base = cfg.base_url.trim_end_matches('/');
    match &cfg.path_prefix {
        Some(p) if !p.is_empty() => format!("{}{}/{}", base, p.trim_end_matches('/'), path),
        _ => format!("{}/{}", base, path),
    }
}

/// Convert internal messages to OpenAI wire format (chat-completions style).
pub fn to_openai_messages(system: &str, messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    if !system.is_empty() {
        out.push(json!({"role": "system", "content": system}));
    }
    for m in messages {
        match &m.role {
            Role::System => {
                out.push(json!({"role": "system", "content": m.plain_text()}));
            }
            Role::User => {
                out.push(json!({"role": "user", "content": m.plain_text()}));
            }
            Role::Assistant => {
                let mut obj = json!({"role": "assistant"});
                let text = m.plain_text();
                obj["content"] = if text.is_empty() && m.has_tool_calls() {
                    Value::Null
                } else {
                    json!(text)
                };
                for part in &m.parts {
                    if let Content::ToolUse(calls) = part {
                        obj["tool_calls"] = Value::Array(
                            calls
                                .iter()
                                .map(|c| {
                                    json!({
                                        "id": c.id,
                                        "type": "function",
                                        "function": {"name": c.name, "arguments": c.arguments},
                                    })
                                })
                                .collect(),
                        );
                    }
                }
                out.push(obj);
            }
            Role::Tool => {
                for part in &m.parts {
                    if let Content::ToolResult {
                        call_id, content, ..
                    } = part
                    {
                        out.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": content,
                        }));
                    }
                }
            }
        }
    }
    out
}

pub(crate) async fn post_stream(
    _client: &reqwest::Client,
    _cfg: &ProviderConfig,
    _url: String,
    rb: reqwest::RequestBuilder,
    body: Value,
) -> Result<reqwest::Response> {
    let resp = rb
        .json(&body)
        .send()
        .await
        .map_err(|e| Error::Provider(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        let code = status.as_u16();
        let text = resp.text().await.unwrap_or_default();
        return Err(Error::Api {
            status: code,
            message: summarize_error(&text),
        });
    }
    Ok(resp)
}

pub(crate) fn summarize_error(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        if let Some(msg) = v["error"]["message"].as_str() {
            return msg.to_string();
        }
        if let Some(msg) = v["message"].as_str() {
            return msg.to_string();
        }
    }
    body.chars().take(400).collect()
}

async fn pump_sse(resp: reqwest::Response, mut handle_data: impl FnMut(&str)) -> Result<()> {
    use futures::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut parser = SseParser::default();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| Error::Provider(e.to_string()))?;
        let chunk_str = String::from_utf8_lossy(&bytes).into_owned();
        let mut stop = false;
        parser.push(&chunk_str, |data| {
            if data == "[DONE]" {
                stop = true;
                return;
            }
            handle_data(data);
        });
        if stop {
            break;
        }
    }
    Ok(())
}

fn apply_auth(
    cfg: &ProviderConfig,
    api_key: Option<&str>,
    mut rb: reqwest::RequestBuilder,
    anthropic_style: bool,
) -> reqwest::RequestBuilder {
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        rb = if anthropic_style {
            rb.header("x-api-key", key)
        } else {
            rb.bearer_auth(key)
        };
    }
    for (k, v) in &cfg.headers {
        rb = rb.header(k, v);
    }
    if !cfg.query.is_empty() {
        rb = rb.query(&Vec::from_iter(
            cfg.query.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        ));
    }
    rb
}

// ---------------------------------------------------------------------------
// POST {base}/chat/completions  (OpenAI-compatible)
// ---------------------------------------------------------------------------

pub struct ChatCompletions {
    client: reqwest::Client,
    cfg: ProviderConfig,
    api_key: Option<String>,
}

impl ChatCompletions {
    pub fn new(client: reqwest::Client, cfg: ProviderConfig, api_key: Option<String>) -> Self {
        Self {
            client,
            cfg,
            api_key,
        }
    }

    fn request_body(req: &CompletionRequest) -> Value {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    },
                })
            })
            .collect();
        let mut body = json!({
            "model": req.model,
            "stream": true,
            "stream_options": {"include_usage": true},
            "messages": to_openai_messages(&req.system, &req.messages),
        });
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        if let Some(t) = req.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(m) = req.max_tokens {
            body["max_tokens"] = json!(m);
        }
        body
    }
}

#[async_trait]
impl LlmProvider for ChatCompletions {
    async fn stream(
        &self,
        req: CompletionRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<()> {
        let url = endpoint(&self.cfg, "chat/completions");
        let rb = apply_auth(
            &self.cfg,
            self.api_key.as_deref(),
            self.client.post(&url),
            false,
        );
        let resp = post_stream(&self.client, &self.cfg, url, rb, Self::request_body(&req)).await?;

        let mut names: Vec<(String, String)> = Vec::new(); // per index: id, name
        pump_sse(resp, |data| {
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                return;
            };
            if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                on_event(StreamEvent::Finish {
                    usage: Usage {
                        input_tokens: u["prompt_tokens"].as_u64().unwrap_or(0),
                        output_tokens: u["completion_tokens"].as_u64().unwrap_or(0),
                    },
                });
            }
            let Some(choice) = v["choices"].get(0) else {
                return;
            };
            let delta = &choice["delta"];
            if let Some(text) = delta["content"].as_str() {
                if !text.is_empty() {
                    on_event(StreamEvent::TextDelta(text.to_string()));
                }
            }
            if let Some(tcs) = delta["tool_calls"].as_array() {
                for tc in tcs {
                    let idx = tc["index"].as_u64().map(|v| v as usize).unwrap_or(0);
                    while names.len() <= idx {
                        names.push((String::new(), String::new()));
                    }
                    if let (Some(id), Some(name)) =
                        (tc["id"].as_str(), tc["function"]["name"].as_str())
                    {
                        names[idx] = (id.to_string(), name.to_string());
                        on_event(StreamEvent::ToolCallBegin {
                            index: idx,
                            id: id.to_string(),
                            name: name.to_string(),
                        });
                    }
                    if let Some(args) = tc["function"]["arguments"].as_str() {
                        if !args.is_empty() {
                            on_event(StreamEvent::ToolCallDelta {
                                index: idx,
                                args: args.to_string(),
                            });
                        }
                    }
                }
            }
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// POST {base}/responses  (OpenAI Responses API)
// ---------------------------------------------------------------------------

pub struct Responses {
    client: reqwest::Client,
    cfg: ProviderConfig,
    api_key: Option<String>,
}

impl Responses {
    pub fn new(client: reqwest::Client, cfg: ProviderConfig, api_key: Option<String>) -> Self {
        Self {
            client,
            cfg,
            api_key,
        }
    }

    fn input_items(messages: &[Message]) -> Vec<Value> {
        let mut out = Vec::new();
        for m in messages {
            match m.role {
                Role::User => out.push(json!({
                    "role": "user",
                    "content": [{"type": "input_text", "text": m.plain_text()}],
                })),
                Role::Assistant => {
                    let text = m.plain_text();
                    if !text.is_empty() {
                        out.push(json!({
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": text}],
                        }));
                    }
                    for part in &m.parts {
                        if let Content::ToolUse(calls) = part {
                            for c in calls {
                                out.push(json!({
                                    "type": "function_call",
                                    "call_id": c.id,
                                    "name": c.name,
                                    "arguments": c.arguments,
                                }));
                            }
                        }
                    }
                }
                Role::Tool => {
                    for part in &m.parts {
                        if let Content::ToolResult {
                            call_id, content, ..
                        } = part
                        {
                            out.push(json!({
                                "type": "function_call_output",
                                "call_id": call_id,
                                "output": content,
                            }));
                        }
                    }
                }
                Role::System => {}
            }
        }
        out
    }

    fn request_body(req: &CompletionRequest) -> Value {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect();
        let mut body = json!({
            "model": req.model,
            "stream": true,
            "instructions": req.system,
            "input": Self::input_items(&req.messages),
        });
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        if let Some(t) = req.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(m) = req.max_tokens {
            body["max_output_tokens"] = json!(m);
        }
        body
    }
}

#[async_trait]
impl LlmProvider for Responses {
    async fn stream(
        &self,
        req: CompletionRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<()> {
        let url = endpoint(&self.cfg, "responses");
        let rb = apply_auth(
            &self.cfg,
            self.api_key.as_deref(),
            self.client.post(&url),
            false,
        );
        let resp = post_stream(&self.client, &self.cfg, url, rb, Self::request_body(&req)).await?;

        let mut item_index: std::collections::HashMap<String, usize> = Default::default();
        pump_sse(resp, |data| {
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                return;
            };
            match v["type"].as_str().unwrap_or("") {
                "response.output_text.delta" => {
                    if let Some(d) = v["delta"].as_str() {
                        on_event(StreamEvent::TextDelta(d.to_string()));
                    }
                }
                "response.output_item.added" => {
                    if v["item"]["type"].as_str() == Some("function_call") {
                        let item_id = v["item"]["id"].as_str().unwrap_or("").to_string();
                        let idx = item_index.len();
                        item_index.insert(item_id, idx);
                        on_event(StreamEvent::ToolCallBegin {
                            index: idx,
                            id: v["item"]["call_id"].as_str().unwrap_or("").to_string(),
                            name: v["item"]["name"].as_str().unwrap_or("").to_string(),
                        });
                    }
                }
                "response.function_call_arguments.delta" => {
                    let idx = item_index
                        .get(v["item_id"].as_str().unwrap_or(""))
                        .copied()
                        .unwrap_or(0);
                    if let Some(d) = v["delta"].as_str() {
                        on_event(StreamEvent::ToolCallDelta {
                            index: idx,
                            args: d.to_string(),
                        });
                    }
                }
                "response.completed" => {
                    if let Some(u) = v["response"]["usage"].as_object() {
                        on_event(StreamEvent::Finish {
                            usage: Usage {
                                input_tokens: u
                                    .get("input_tokens")
                                    .and_then(|x| x.as_u64())
                                    .unwrap_or(0),
                                output_tokens: u
                                    .get("output_tokens")
                                    .and_then(|x| x.as_u64())
                                    .unwrap_or(0),
                            },
                        });
                    }
                }
                _ => {}
            }
        })
        .await
    }
}

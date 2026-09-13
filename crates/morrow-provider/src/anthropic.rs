use async_trait::async_trait;
use morrow_core::config::ProviderConfig;
use morrow_core::message::{Content, Message, Role, Usage};
use morrow_core::{Error, Result};
use serde_json::{json, Value};

use super::{CompletionRequest, LlmProvider, SseParser, StreamEvent};
use crate::openai::summarize_error;

pub struct AnthropicMessages {
    client: reqwest::Client,
    cfg: ProviderConfig,
    api_key: Option<String>,
}

impl AnthropicMessages {
    pub fn new(client: reqwest::Client, cfg: ProviderConfig, api_key: Option<String>) -> Self {
        Self {
            client,
            cfg,
            api_key,
        }
    }

    fn endpoint(&self) -> String {
        let base = self.cfg.base_url.trim_end_matches('/');
        let path = if base.ends_with("/v1") {
            "messages"
        } else {
            "v1/messages"
        };
        format!("{base}/{path}")
    }

    fn messages_body(messages: &[Message]) -> Vec<Value> {
        let mut out = Vec::new();
        for m in messages {
            match m.role {
                Role::System => {
                    out.push(json!({"role": "user", "content": [{"type": "text", "text": m.plain_text()}]}));
                }
                Role::User => {
                    out.push(json!({"role": "user", "content": [{"type": "text", "text": m.plain_text()}]}));
                }
                Role::Assistant => {
                    let mut blocks = Vec::new();
                    let text = m.plain_text();
                    if !text.is_empty() {
                        blocks.push(json!({"type": "text", "text": text}));
                    }
                    for part in &m.parts {
                        if let Content::ToolUse(calls) = part {
                            for c in calls {
                                let args: Value =
                                    serde_json::from_str(&c.arguments).unwrap_or(json!({}));
                                blocks.push(json!({
                                    "type": "tool_use",
                                    "id": c.id,
                                    "name": c.name,
                                    "input": args,
                                }));
                            }
                        }
                    }
                    if blocks.is_empty() {
                        blocks.push(json!({"type": "text", "text": "(no content)"}));
                    }
                    out.push(json!({"role": "assistant", "content": blocks}));
                }
                Role::Tool => {
                    for part in &m.parts {
                        if let Content::ToolResult {
                            call_id,
                            content,
                            is_error,
                        } = part
                        {
                            out.push(json!({
                                "role": "user",
                                "content": [{
                                    "type": "tool_result",
                                    "tool_use_id": call_id,
                                    "content": content,
                                    "is_error": is_error,
                                }],
                            }));
                        }
                    }
                }
            }
        }
        out
    }

    fn request_body(&self, req: &CompletionRequest) -> Value {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                })
            })
            .collect();
        let max_tokens = req.max_tokens.unwrap_or(8192);
        let mut body = json!({
            "model": req.model,
            "max_tokens": max_tokens,
            "stream": true,
            "system": req.system,
            "messages": Self::messages_body(&req.messages),
        });
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        if let Some(t) = req.temperature {
            body["temperature"] = json!(t);
        }
        body
    }
}

#[async_trait]
impl LlmProvider for AnthropicMessages {
    async fn stream(
        &self,
        req: CompletionRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<()> {
        use futures::StreamExt;
        let url = self.endpoint();

        let mut rb = self.client.post(&url);
        if let Some(key) = self.api_key.as_deref().filter(|k| !k.is_empty()) {
            rb = rb.header("x-api-key", key);
        }
        rb = rb.header("anthropic-version", "2023-06-01");
        for (k, v) in &self.cfg.headers {
            rb = rb.header(k, v);
        }

        let resp = match rb.json(&self.request_body(&req)).send().await {
            Ok(r) => r,
            Err(e) => return Err(Error::Provider(e.to_string())),
        };
        if !resp.status().is_success() {
            let code = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Api {
                status: code,
                message: summarize_error(&text),
            });
        }

        // tool_use block ids by content-block index
        let mut block_ids: std::collections::HashMap<u64, (String, String)> = Default::default();
        let mut stream = resp.bytes_stream();
        let mut parser = SseParser::default();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| Error::Provider(e.to_string()))?;
            let chunk_str = String::from_utf8_lossy(&bytes).into_owned();
            parser.push(&chunk_str, |data| {
                let Ok(v) = serde_json::from_str::<Value>(data) else {
                    return;
                };
                match v["type"].as_str().unwrap_or("") {
                    "message_start" => {
                        let u = &v["message"]["usage"];
                        on_event(StreamEvent::Finish {
                            usage: Usage {
                                input_tokens: u["input_tokens"].as_u64().unwrap_or(0),
                                output_tokens: 0,
                            },
                        });
                    }
                    "content_block_start" => {
                        let idx = v["index"].as_u64().unwrap_or(0);
                        if v["content_block"]["type"].as_str() == Some("tool_use") {
                            let id = v["content_block"]["id"].as_str().unwrap_or("").to_string();
                            let name = v["content_block"]["name"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();
                            block_ids.insert(idx, (id.clone(), name.clone()));
                            on_event(StreamEvent::ToolCallBegin {
                                index: idx as usize,
                                id,
                                name,
                            });
                        }
                    }
                    "content_block_delta" => {
                        let delta = &v["delta"];
                        match delta["type"].as_str().unwrap_or("") {
                            "text_delta" => {
                                if let Some(t) = delta["text"].as_str() {
                                    on_event(StreamEvent::TextDelta(t.to_string()));
                                }
                            }
                            "input_json_delta" => {
                                let idx = v["index"].as_u64().unwrap_or(0) as usize;
                                if let Some(p) = delta["partial_json"].as_str() {
                                    on_event(StreamEvent::ToolCallDelta {
                                        index: idx,
                                        args: p.to_string(),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            });
        }
        Ok(())
    }
}

use morrow_core::message::{Message, Usage};
use serde::{Deserialize, Serialize};

/// A tool exposed to the model (JSON-schema parameters).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON schema object for the parameters.
    pub parameters: serde_json::Value,
}

/// One streaming event from a completion.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    TextDelta(String),
    ToolCallBegin {
        index: usize,
        id: String,
        name: String,
    },
    ToolCallDelta {
        index: usize,
        args: String,
    },
    Finish {
        usage: Usage,
    },
}

/// Request for one completion turn.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
}

/// Streaming LLM client abstraction implemented per wire protocol.
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Run one completion, invoking `on_event` for every streamed chunk.
    async fn stream(
        &self,
        req: CompletionRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> morrow_core::Result<()>;
}

pub mod anthropic;
pub mod openai;

use morrow_core::config::{Protocol, ProviderConfig};

/// Build a provider client for the given endpoint configuration.
pub fn create(
    cfg: &ProviderConfig,
    api_key: Option<String>,
) -> morrow_core::Result<Box<dyn LlmProvider>> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("morrow/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| morrow_core::Error::Provider(e.to_string()))?;
    Ok(match cfg.protocol {
        Protocol::OpenAiChat => {
            Box::new(openai::ChatCompletions::new(client, cfg.clone(), api_key))
        }
        Protocol::OpenAiResponses => Box::new(openai::Responses::new(client, cfg.clone(), api_key)),
        Protocol::Anthropic => Box::new(anthropic::AnthropicMessages::new(
            client,
            cfg.clone(),
            api_key,
        )),
    })
}

/// Minimal incremental SSE line parser shared by all protocols.
#[derive(Default)]
pub struct SseParser {
    buf: String,
}

impl SseParser {
    pub fn push(&mut self, chunk: &str, mut on_data: impl FnMut(&str)) {
        self.buf.push_str(chunk);
        while let Some(pos) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=pos).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            if let Some(data) = line.strip_prefix("data:") {
                on_data(data.trim_start());
            }
            // ignore comments / event:/id: lines
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_parses_incremental_chunks() {
        let mut p = SseParser::default();
        let mut out = Vec::new();
        p.push("data: {\"a\":1}\n\nda", |d| out.push(d.to_string()));
        p.push("ta: {\"b\":2}\n\n", |d| out.push(d.to_string()));
        assert_eq!(out, vec!["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn sse_ignores_comments_and_done_marker_passes_through() {
        let mut p = SseParser::default();
        let mut out = Vec::new();
        p.push(": keepalive\ndata: [DONE]\n", |d| out.push(d.to_string()));
        assert_eq!(out, vec!["[DONE]"]);
    }
}

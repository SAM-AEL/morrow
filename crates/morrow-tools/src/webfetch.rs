use async_trait::async_trait;
use serde_json::json;

use crate::registry::{opt_u64_arg, str_arg, Tool, ToolContext, ToolKind};

pub struct WebFetch {
    client: reqwest::Client,
}

impl Default for WebFetch {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFetch {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent(concat!("morrow/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
        }
    }
}

/// Very small HTML-to-text conversion for readability.
fn strip_html(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // find tag end
            if let Some(end) = lower[i..].find('>') {
                let tag = lower[i + 1..i + end].trim();
                if tag.starts_with("script") || tag.starts_with("style") {
                    // skip until matching close tag
                    let close = format!("</{}", &tag[..5]);
                    match lower[i + end..].find(&close) {
                        Some(off) => i += end + off,
                        None => return out.split_whitespace().collect::<Vec<_>>().join(" "),
                    }
                }
                i += end + 1;
                out.push(' ');
                continue;
            }
            break;
        }
        let ch = html[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &'static str {
        "webfetch"
    }

    fn description(&self) -> &'static str {
        "Fetch a URL over HTTP(S) and return its body as text (HTML is stripped). \
         Useful for docs and APIs."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "Absolute http(s) URL"},
                "max_length": {"type": "integer", "description": "Max returned characters (default 20000)"}
            },
            "required": ["url"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::ReadOnly
    }

    async fn run(
        &self,
        args: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> morrow_core::Result<String> {
        let url = str_arg(args, "url")?;
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(morrow_core::Error::Other(
                "url must start with http:// or https://".into(),
            ));
        }
        let max_len = opt_u64_arg(args, "max_length").unwrap_or(20_000) as usize;
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| morrow_core::Error::Other(format!("fetch failed: {e}")))?;
        let status = resp.status();
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = resp.text().await.unwrap_or_default();

        let text = if ctype.contains("html") || body.trim_start().starts_with('<') {
            strip_html(&body)
        } else {
            body
        };

        // Truncate on a char boundary: a byte slice would panic on any multi-byte
        // character near the cut, which any real page can contain.
        let truncated = crate::truncate_utf8(&text, max_len);
        Ok(format!(
            "[status {}] {}\n{}",
            status.as_u16(),
            url,
            truncated
        ))
    }
}

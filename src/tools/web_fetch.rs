use crate::agent::llama_client::ToolDef;
use serde_json::{Value, json};
use std::time::Duration;

const MAX_BYTES_RAW: usize = 500 * 1024;
const MAX_BYTES_OUTPUT: usize = 32 * 1024;
const TIMEOUT_SECS: u64 = 20;

pub fn spec() -> ToolDef {
    super::tool_def(
        "WebFetch",
        "Fetch the contents of a URL via HTTP GET. HTML responses are stripped to plain text (scripts/styles removed, entities decoded, tags dropped). Use to read web pages, JSON APIs, or documentation. Returns at most 32KB of text. Times out after 20s. Read-only.",
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Absolute URL (http or https) to fetch."
                }
            },
            "required": ["url"],
            "additionalProperties": false,
        }),
    )
}

pub async fn execute(args: &Value) -> anyhow::Result<String> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("web_fetch: missing `url`"))?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(anyhow::anyhow!("web_fetch: url must start with http:// or https://"));
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .user_agent("mochi/0.1 (+terminal AI pet)")
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| anyhow::anyhow!("web_fetch: client build: {e}"))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("web_fetch: request failed: {e}"))?;
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let bytes =
        resp.bytes().await.map_err(|e| anyhow::anyhow!("web_fetch: read body failed: {e}"))?;

    let raw_truncated = bytes.len() > MAX_BYTES_RAW;
    let slice = if raw_truncated { &bytes[..MAX_BYTES_RAW] } else { &bytes[..] };
    let raw = String::from_utf8_lossy(slice);

    let is_html = content_type.to_lowercase().contains("html") || raw.trim_start().starts_with('<');
    let mut text = if is_html { html_to_text(&raw) } else { raw.into_owned() };

    let output_truncated = text.len() > MAX_BYTES_OUTPUT;
    if output_truncated {
        text.truncate(MAX_BYTES_OUTPUT);
    }

    let mut out = format!(
        "# GET {url}\nstatus: {status}\ncontent-type: {content_type}\nbytes-raw: {}, bytes-text: {}{}\n\n",
        bytes.len(),
        text.len(),
        if is_html { " (HTML stripped)" } else { "" }
    );
    out.push_str(&text);
    if output_truncated {
        out.push_str("\n\n[truncated: text exceeds 32KB output cap]");
    } else if raw_truncated {
        out.push_str("\n\n[truncated: response exceeds 500KB read limit before stripping]");
    }
    Ok(out)
}

/// Crude HTML → text stripper. Removes script/style blocks and all tags,
/// decodes a small set of common entities, and collapses whitespace.
/// Good enough for letting an LLM read article-like pages; not a real parser.
fn html_to_text(html: &str) -> String {
    let stripped = strip_block(html, "<script", "</script>");
    let stripped = strip_block(&stripped, "<style", "</style>");
    let stripped = strip_block(&stripped, "<!--", "-->");

    let mut out = String::with_capacity(stripped.len() / 2);
    let mut chars = stripped.chars().peekable();
    let mut in_tag = false;
    while let Some(c) = chars.next() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if in_tag => {}
            _ => out.push(c),
        }
    }
    let decoded = decode_entities(&out);
    collapse_whitespace(&decoded)
}

fn strip_block(s: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        match rest.find(open) {
            None => {
                out.push_str(rest);
                break;
            }
            Some(start) => {
                out.push_str(&rest[..start]);
                let after_open = &rest[start..];
                match after_open.find(close) {
                    None => break,
                    Some(end) => rest = &after_open[end + close.len()..],
                }
            }
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_blank = false;
    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !last_blank && !out.is_empty() {
                out.push('\n');
                last_blank = true;
            }
            continue;
        }
        let mut prev_space = false;
        for c in trimmed.chars() {
            if c.is_whitespace() {
                if !prev_space {
                    out.push(' ');
                    prev_space = true;
                }
            } else {
                out.push(c);
                prev_space = false;
            }
        }
        out.push('\n');
        last_blank = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::execute;
    use serde_json::json;

    #[tokio::test]
    async fn rejects_relative_url() {
        let result = execute(&json!({"url": "/some/path"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_file_scheme() {
        let result = execute(&json!({"url": "file:///etc/passwd"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn missing_url_returns_err() {
        let result = execute(&json!({})).await;
        assert!(result.is_err());
    }

    #[test]
    fn html_stripper_drops_scripts_styles_and_tags() {
        let html = "<html><head><style>.x{color:red}</style><script>alert(1)</script></head><body><h1>Hello</h1><p>World &amp; mochi</p></body></html>";
        let out = super::html_to_text(html);
        assert!(out.contains("Hello"));
        assert!(out.contains("World & mochi"));
        assert!(!out.contains("alert"));
        assert!(!out.contains("color:red"));
        assert!(!out.contains("<"));
    }

    #[test]
    fn html_stripper_collapses_whitespace() {
        let html = "<p>line1</p>\n\n\n\n<p>line2</p>";
        let out = super::html_to_text(html);
        let blanks = out.matches("\n\n\n").count();
        assert_eq!(blanks, 0);
    }

    #[test]
    fn html_stripper_decodes_common_entities() {
        let out = super::html_to_text("<p>&lt;tag&gt; &amp; &nbsp;done</p>");
        assert!(out.contains("<tag>"));
        assert!(out.contains("&"));
    }
}

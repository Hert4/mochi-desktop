//! `WebSearch` tool. Mirrors Anthropic SDK `WebSearch` schema with `query` arg.
//! Implementation hits DuckDuckGo's server-rendered HTML endpoint (no API key
//! needed) and strips the result list to a compact `title | url | snippet`
//! line-per-result format that fits comfortably in a llama context.

use crate::agent::llama_client::ToolDef;
use serde_json::{Value, json};
use std::time::Duration;

const TIMEOUT_SECS: u64 = 15;
const DEFAULT_COUNT: usize = 8;
const MAX_COUNT: usize = 15;

pub fn spec() -> ToolDef {
    super::tool_def(
        "WebSearch",
        "Search the web via DuckDuckGo and return a ranked list of result titles, URLs, and snippets. Use BEFORE WebFetch when the user asks a factual question and you don't know the URL. Returns 8 results by default, up to 15. Do NOT call WebFetch on google.com/search or bing.com directly — use WebSearch.",
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query in plain English. Be specific."
                },
                "count": {
                    "type": "integer",
                    "description": "How many results to return (default 8, max 15)."
                }
            },
            "required": ["query"],
            "additionalProperties": false,
        }),
    )
}

pub async fn execute(args: &Value) -> anyhow::Result<String> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("WebSearch: missing `query`"))?
        .trim();
    if query.is_empty() {
        return Err(anyhow::anyhow!("WebSearch: empty query"));
    }
    let count = args
        .get("count")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_COUNT)
        .clamp(1, MAX_COUNT);

    let encoded = urlencode(query);
    let url = format!("https://html.duckduckgo.com/html/?q={encoded}");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .user_agent("Mozilla/5.0 (mochi/0.1 terminal-pet)")
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| anyhow::anyhow!("WebSearch: client build: {e}"))?;

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("WebSearch: request failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!("WebSearch: {url} returned {status}"));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| anyhow::anyhow!("WebSearch: read body: {e}"))?;

    let results = parse_ddg_html(&body, count);
    if results.is_empty() {
        return Ok(format!(
            "WebSearch `{query}`: 0 results (DuckDuckGo HTML response was empty or unparseable)"
        ));
    }

    let mut out = format!("WebSearch `{query}` — {} results:\n\n", results.len());
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!("{}. {}\n   {}\n", i + 1, r.title, r.url));
        if !r.snippet.is_empty() {
            out.push_str(&format!("   {}\n", r.snippet));
        }
        out.push('\n');
    }
    Ok(out)
}

#[derive(Debug, PartialEq, Eq)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

/// Parse DuckDuckGo HTML results. The page uses `<a class="result__a" href="...">` for
/// titles/URLs and `<a class="result__snippet">` for snippets. Brittle but
/// stable enough; falls back gracefully when patterns change.
fn parse_ddg_html(html: &str, max: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let mut cursor = html;

    while results.len() < max {
        let Some(anchor_idx) = cursor.find("class=\"result__a\"") else { break };
        let after_class = &cursor[anchor_idx..];
        let Some(href_idx) = after_class.find("href=\"") else { break };
        let url_start = href_idx + "href=\"".len();
        let Some(url_end) = after_class[url_start..].find('"') else { break };
        let raw_href = &after_class[url_start..url_start + url_end];
        let url = decode_ddg_redirect(raw_href);

        let Some(title_open) = after_class.find('>') else { break };
        let title_inner = &after_class[title_open + 1..];
        let Some(title_close) = title_inner.find("</a>") else { break };
        let title_html = &title_inner[..title_close];
        let title = strip_tags(title_html).trim().to_owned();

        let after_title = &title_inner[title_close + "</a>".len()..];
        let snippet = if let Some(s_idx) = after_title.find("class=\"result__snippet\"") {
            let after_s = &after_title[s_idx..];
            if let Some(s_open) = after_s.find('>') {
                let snip_inner = &after_s[s_open + 1..];
                if let Some(s_close) = snip_inner.find("</a>") {
                    strip_tags(&snip_inner[..s_close]).trim().to_owned()
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult { title, url, snippet });
        }
        cursor = after_title;
    }

    results
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if in_tag => {}
            _ => out.push(c),
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// DuckDuckGo wraps result URLs as `//duckduckgo.com/l/?uddg=<encoded>&...`.
/// Decode the inner `uddg` parameter when present; pass through otherwise.
fn decode_ddg_redirect(href: &str) -> String {
    let trimmed = href.strip_prefix("//").unwrap_or(href);
    let trimmed = trimmed.strip_prefix("https://").unwrap_or(trimmed);
    let trimmed = trimmed.strip_prefix("http://").unwrap_or(trimmed);
    if let Some(q_idx) = trimmed.find("uddg=") {
        let after = &trimmed[q_idx + "uddg=".len()..];
        let end = after.find('&').unwrap_or(after.len());
        return urldecode(&after[..end]);
    }
    if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href.to_owned()
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                let n = u8::from_str_radix(hex, 16).unwrap_or(0);
                out.push(n);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{decode_ddg_redirect, parse_ddg_html, strip_tags, urldecode, urlencode};

    #[test]
    fn urlencode_basic() {
        assert_eq!(urlencode("hello world"), "hello+world");
        assert_eq!(urlencode("a&b"), "a%26b");
        assert_eq!(urlencode("café"), "caf%C3%A9");
    }

    #[test]
    fn urldecode_roundtrips_known_inputs() {
        assert_eq!(urldecode("hello+world"), "hello world");
        assert_eq!(urldecode("a%26b"), "a&b");
        assert_eq!(urldecode("caf%C3%A9"), "café");
    }

    #[test]
    fn strip_tags_decodes_entities() {
        assert_eq!(strip_tags("<b>hi &amp; bye</b>"), "hi & bye");
    }

    #[test]
    fn decodes_ddg_uddg_wrapper() {
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fcat&rut=abc";
        assert_eq!(decode_ddg_redirect(href), "https://example.com/cat");
    }

    #[test]
    fn passes_through_direct_https_url() {
        assert_eq!(decode_ddg_redirect("https://example.com"), "https://example.com");
    }

    #[test]
    fn parses_minimal_ddg_result_block() {
        let html = r##"
        <div>
          <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa">Example A</a>
          <a class="result__snippet" href="x">First snippet here.</a>
        </div>
        <div>
          <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fb">Example B</a>
        </div>
        "##;
        let results = parse_ddg_html(html, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Example A");
        assert_eq!(results[0].url, "https://example.com/a");
        assert_eq!(results[0].snippet, "First snippet here.");
        assert_eq!(results[1].title, "Example B");
        assert!(results[1].snippet.is_empty());
    }
}

use eventsource_stream::Eventsource as _;
use futures::stream::{Stream, StreamExt as _};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct LlamaConfig {
    pub url: String,
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

impl Default for LlamaConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:8765".to_owned(),
            model: None,
            temperature: None,
            max_tokens: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEvent {
    Delta(String),
    Done,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct ChatChunk {
    #[serde(default)]
    choices: Vec<ChatChunkChoice>,
}

#[derive(Deserialize)]
struct ChatChunkChoice {
    #[serde(default)]
    delta: ChatChunkDelta,
}

#[derive(Deserialize, Default)]
struct ChatChunkDelta {
    #[serde(default)]
    content: Option<String>,
}

pub async fn stream_chat(
    config: &LlamaConfig,
    messages: &[Message],
) -> anyhow::Result<impl Stream<Item = anyhow::Result<ChatEvent>> + use<>> {
    let url = format!("{}/v1/chat/completions", config.url.trim_end_matches('/'));
    let body = ChatRequest {
        model: config.model.as_deref().unwrap_or("local"),
        messages,
        stream: true,
        temperature: config.temperature,
        max_tokens: config.max_tokens,
    };
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("llama request failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("llama returned {status}: {text}"));
    }
    let stream = resp.bytes_stream().eventsource().map(parse_sse_item);
    Ok(stream)
}

fn parse_sse_item<E: std::fmt::Display>(
    item: Result<eventsource_stream::Event, E>,
) -> anyhow::Result<ChatEvent> {
    let evt = item.map_err(|e| anyhow::anyhow!("sse parse: {e}"))?;
    parse_sse_data(&evt.data)
}

fn parse_sse_data(data: &str) -> anyhow::Result<ChatEvent> {
    if data == "[DONE]" {
        return Ok(ChatEvent::Done);
    }
    let chunk: ChatChunk =
        serde_json::from_str(data).map_err(|e| anyhow::anyhow!("decode chunk: {e}"))?;
    let content =
        chunk.choices.into_iter().next().and_then(|c| c.delta.content).unwrap_or_default();
    Ok(ChatEvent::Delta(content))
}

#[cfg(test)]
mod tests {
    use super::{ChatEvent, parse_sse_data};

    #[test]
    fn parses_delta_chunk() {
        let data = r#"{"choices":[{"delta":{"content":"Hello"},"index":0}]}"#;
        let evt = parse_sse_data(data).unwrap();
        assert_eq!(evt, ChatEvent::Delta("Hello".to_owned()));
    }

    #[test]
    fn parses_done_marker() {
        let evt = parse_sse_data("[DONE]").unwrap();
        assert_eq!(evt, ChatEvent::Done);
    }

    #[test]
    fn parses_empty_delta_as_empty_string() {
        let data = r#"{"choices":[{"delta":{},"index":0,"finish_reason":"stop"}]}"#;
        let evt = parse_sse_data(data).unwrap();
        assert_eq!(evt, ChatEvent::Delta(String::new()));
    }

    #[test]
    fn parses_chunk_with_no_choices() {
        let data = r#"{"choices":[]}"#;
        let evt = parse_sse_data(data).unwrap();
        assert_eq!(evt, ChatEvent::Delta(String::new()));
    }

    #[test]
    fn rejects_malformed_json() {
        let result = parse_sse_data("not-json");
        assert!(result.is_err());
    }
}

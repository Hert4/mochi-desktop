use async_stream::try_stream;
use eventsource_stream::Eventsource as _;
use futures::stream::{Stream, StreamExt as _};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<RoleToolCall>>,
}

impl Message {
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: Role::System, content: content.into(), tool_call_id: None, tool_calls: None }
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: Role::User, content: content.into(), tool_call_id: None, tool_calls: None }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    #[must_use]
    pub fn tool_response(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: None,
        }
    }
}

/// Re-emitted tool calls on an assistant turn (for history when feeding back results).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoleToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: RoleToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoleToolCallFunction {
    pub name: String,
    pub arguments: String,
}

/// Tool definition advertised to the model.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: ToolDefFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEvent {
    Delta(String),
    /// A fully accumulated tool call. The runner should execute it and feed
    /// the result back in a new turn as a `Role::Tool` message.
    ToolCall(LlmToolCall),
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [ToolDef]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
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
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct ChatChunkDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<DeltaToolCall>>,
}

#[derive(Deserialize)]
struct DeltaToolCall {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<DeltaToolCallFunction>,
}

#[derive(Deserialize, Default)]
struct DeltaToolCallFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Default)]
struct ToolCallBuffer {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ToolCallBuffer {
    fn finalize(self) -> Option<LlmToolCall> {
        Some(LlmToolCall {
            id: self.id.unwrap_or_else(|| format!("call_{}", uuid::Uuid::new_v4().simple())),
            name: self.name?,
            arguments: self.arguments,
        })
    }
}

pub async fn stream_chat(
    config: &LlamaConfig,
    messages: &[Message],
) -> anyhow::Result<impl Stream<Item = anyhow::Result<ChatEvent>> + use<>> {
    stream_chat_inner(config, messages, None).await
}

pub async fn stream_chat_with_tools(
    config: &LlamaConfig,
    messages: &[Message],
    tools: &[ToolDef],
) -> anyhow::Result<impl Stream<Item = anyhow::Result<ChatEvent>> + use<>> {
    stream_chat_inner(config, messages, Some(tools)).await
}

async fn stream_chat_inner(
    config: &LlamaConfig,
    messages: &[Message],
    tools: Option<&[ToolDef]>,
) -> anyhow::Result<impl Stream<Item = anyhow::Result<ChatEvent>> + use<>> {
    let url = format!("{}/v1/chat/completions", config.url.trim_end_matches('/'));
    let body = ChatRequest {
        model: config.model.as_deref().unwrap_or("local"),
        messages,
        stream: true,
        temperature: config.temperature,
        max_tokens: config.max_tokens,
        tools,
        tool_choice: if tools.is_some() { Some("auto") } else { None },
    };
    let body_json =
        serde_json::to_string(&body).map_err(|e| anyhow::anyhow!("encode body: {e}"))?;
    let has_tools_field = body_json.contains("\"tools\":");
    tracing::info!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "llama_request_send",
        body_bytes = body_json.len(),
        has_tools_field,
        tool_count = tools.map(<[_]>::len).unwrap_or(0),
        "POST llama /v1/chat/completions"
    );
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("content-type", "application/json")
        .body(body_json)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("llama request failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("llama returned {status}: {text}"));
    }

    let bytes = resp.bytes_stream();
    Ok(try_stream! {
        let mut sse = bytes.eventsource();
        let mut tool_buffers: HashMap<u32, ToolCallBuffer> = HashMap::new();
        while let Some(item) = sse.next().await {
            let evt = item.map_err(|e| anyhow::anyhow!("sse parse: {e}"))?;
            if evt.data == "[DONE]" {
                for (_, buf) in std::mem::take(&mut tool_buffers) {
                    if let Some(tc) = buf.finalize() {
                        yield ChatEvent::ToolCall(tc);
                    }
                }
                yield ChatEvent::Done;
                break;
            }
            let chunk: ChatChunk = serde_json::from_str(&evt.data)
                .map_err(|e| anyhow::anyhow!("decode chunk: {e}"))?;
            for choice in chunk.choices {
                if let Some(content) = choice.delta.content {
                    if !content.is_empty() {
                        yield ChatEvent::Delta(content);
                    }
                }
                if let Some(tcs) = choice.delta.tool_calls {
                    for tc in tcs {
                        let buf = tool_buffers.entry(tc.index).or_default();
                        if let Some(id) = tc.id { buf.id = Some(id); }
                        if let Some(func) = tc.function {
                            if let Some(name) = func.name { buf.name = Some(name); }
                            if let Some(args) = func.arguments { buf.arguments.push_str(&args); }
                        }
                    }
                }
                if choice.finish_reason.as_deref() == Some("tool_calls") {
                    for (_, buf) in std::mem::take(&mut tool_buffers) {
                        if let Some(tc) = buf.finalize() {
                            yield ChatEvent::ToolCall(tc);
                        }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{ChatEvent, LlmToolCall, ToolCallBuffer};
    use crate::agent::llama_client as me;
    use serde_json::json;

    fn parse_chunk(data: &str) -> me::ChatChunk {
        serde_json::from_str(data).unwrap()
    }

    #[test]
    fn parses_content_delta() {
        let chunk = parse_chunk(r#"{"choices":[{"delta":{"content":"Hello"}}]}"#);
        let content = chunk.choices.into_iter().next().and_then(|c| c.delta.content);
        assert_eq!(content.as_deref(), Some("Hello"));
    }

    #[test]
    fn parses_tool_call_start() {
        let chunk = parse_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_x","type":"function","function":{"name":"read_file","arguments":""}}]}}]}"#,
        );
        let tcs = chunk.choices.into_iter().next().unwrap().delta.tool_calls.unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].id.as_deref(), Some("call_x"));
        assert_eq!(tcs[0].function.as_ref().unwrap().name.as_deref(), Some("read_file"));
    }

    #[test]
    fn accumulator_assembles_split_arguments() {
        let mut buf = ToolCallBuffer::default();
        buf.id = Some("call_1".into());
        buf.name = Some("read_file".into());
        buf.arguments.push_str("{\"path\":");
        buf.arguments.push_str("\"/tmp/foo\"}");
        let finalized = buf.finalize().unwrap();
        assert_eq!(finalized.id, "call_1");
        assert_eq!(finalized.name, "read_file");
        assert_eq!(finalized.arguments, "{\"path\":\"/tmp/foo\"}");
    }

    #[test]
    fn finalize_synthesizes_id_when_missing() {
        let mut buf = ToolCallBuffer::default();
        buf.name = Some("read_file".into());
        let finalized = buf.finalize().unwrap();
        assert!(finalized.id.starts_with("call_"));
    }

    #[test]
    fn finalize_returns_none_without_name() {
        let buf = ToolCallBuffer::default();
        assert!(buf.finalize().is_none());
    }

    #[test]
    fn chat_event_equality() {
        let a = ChatEvent::ToolCall(LlmToolCall {
            id: "x".into(),
            name: "y".into(),
            arguments: "{}".into(),
        });
        let b = ChatEvent::ToolCall(LlmToolCall {
            id: "x".into(),
            name: "y".into(),
            arguments: "{}".into(),
        });
        assert_eq!(a, b);
    }

    #[test]
    fn message_helpers_produce_expected_role() {
        let m = me::Message::tool_response("call_1", "hello");
        assert_eq!(m.role, me::Role::Tool);
        assert_eq!(m.tool_call_id.as_deref(), Some("call_1"));
    }

    #[test]
    fn tool_def_serializes_function_envelope() {
        let def = me::ToolDef {
            kind: "function",
            function: me::ToolDefFunction {
                name: "read_file".into(),
                description: "Read a file".into(),
                parameters: json!({"type":"object","properties":{}}),
            },
        };
        let v = serde_json::to_value(&def).unwrap();
        assert_eq!(v["type"], "function");
        assert_eq!(v["function"]["name"], "read_file");
    }
}

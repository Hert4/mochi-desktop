//! Llama provider lifecycle: replaces the Node bridge with a direct llama.cpp
//! HTTP+SSE client and synthesizes the BridgeEvents the existing TUI expects.

use crate::agent::client::AgentConnection;
use crate::agent::llama_client::{
    ChatEvent, LlamaConfig, LlmToolCall, Message, RoleToolCall, RoleToolCallFunction,
    stream_chat_with_tools,
};
use crate::tools;
use crate::agent::types;
use crate::agent::wire::{BridgeCommand, BridgeEvent, CommandEnvelope, EventEnvelope};
use crate::memory::{self, MemoryStore, render_memory_section};
use crate::memory_capture;
use crate::skills::{self, Skill};
use futures::StreamExt as _;
use std::collections::BTreeMap;
use std::rc::Rc;
use tokio::sync::mpsc;

/// Out-of-band control messages from slash handlers to the running llama task.
/// Used to trigger system-prompt rebuilds when memory or skill state changes mid-session.
#[derive(Debug, Clone)]
pub enum LlamaRuntimeCommand {
    RebuildSystem,
    SetActiveSkill(Option<String>),
}

use super::ConnectionSlot;
use super::StartConnectionParams;
use super::event_dispatch::handle_bridge_event;

pub(super) async fn run_llama_task(
    params: StartConnectionParams,
    conn_slot_writer: Rc<std::cell::RefCell<Option<ConnectionSlot>>>,
    rt_tx: mpsc::UnboundedSender<LlamaRuntimeCommand>,
    mut rt_rx: mpsc::UnboundedReceiver<LlamaRuntimeCommand>,
) {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<CommandEnvelope>();
    *conn_slot_writer.borrow_mut() =
        Some(ConnectionSlot { conn: Rc::new(AgentConnection::new(cmd_tx.clone())) });

    let mut connected_once = false;
    let session_id = uuid::Uuid::new_v4().to_string();

    handle_bridge_event(
        &params.event_tx,
        &cmd_tx,
        &mut connected_once,
        false,
        EventEnvelope {
            request_id: None,
            event: BridgeEvent::Initialized { result: synth_initialize_result() },
        },
    );

    handle_bridge_event(
        &params.event_tx,
        &cmd_tx,
        &mut connected_once,
        false,
        EventEnvelope {
            request_id: None,
            event: BridgeEvent::Connected {
                session_id: session_id.clone(),
                cwd: params.cwd_raw.clone(),
                current_model: synth_current_model(&params.llama_url),
                available_models: vec![synth_available_model(&params.llama_url)],
                mode: None,
                history_updates: None,
            },
        },
    );

    let config = LlamaConfig {
        url: params.llama_url.clone(),
        model: None,
        temperature: Some(params.llama_temperature),
        max_tokens: Some(1024),
    };

    let base_system =
        "You are Mochi, a cute terminal pet companion. Reply concisely (1-3 sentences) and stay in character.".to_owned();
    let mut active_skill: Option<String> = None;
    let mut skills_cache: BTreeMap<String, Skill> = load_skills_cache();

    let mut history: Vec<Message> = vec![Message::system(build_system_prompt(
        &base_system,
        active_skill.as_deref(),
        &skills_cache,
    ))];

    loop {
        tokio::select! {
            Some(envelope) = cmd_rx.recv() => {
                match envelope.command {
                    BridgeCommand::Prompt { session_id: prompt_session, chunks } => {
                        let user_text = extract_user_text(&chunks);
                        if !user_text.is_empty() {
                            spawn_background_capture(
                                config.clone(),
                                user_text,
                                active_skill.clone(),
                                rt_tx.clone(),
                            );
                        }
                        handle_prompt(
                            &params.event_tx,
                            &cmd_tx,
                            &mut connected_once,
                            &mut history,
                            &config,
                            prompt_session,
                            chunks,
                        )
                        .await;
                    }
                    BridgeCommand::Shutdown => break,
                    BridgeCommand::CancelTurn { .. } => {
                        // v0.1: in-flight requests are not cancellable. Skip silently.
                    }
                    BridgeCommand::Initialize { .. }
                    | BridgeCommand::NewSession { .. }
                    | BridgeCommand::CreateSession { .. }
                    | BridgeCommand::ResumeSession { .. } => {
                        // sessions are implicit in llama mode; ignore.
                    }
                    _ => {
                        // SetMode, MCP/plugins, elicitation, etc. — silently drop.
                    }
                }
            }
            Some(rt_cmd) = rt_rx.recv() => {
                match rt_cmd {
                    LlamaRuntimeCommand::RebuildSystem => {
                        skills_cache = load_skills_cache();
                        if let Some(first) = history.first_mut() {
                            first.content = build_system_prompt(
                                &base_system,
                                active_skill.as_deref(),
                                &skills_cache,
                            );
                        }
                    }
                    LlamaRuntimeCommand::SetActiveSkill(name) => {
                        active_skill = name;
                        skills_cache = load_skills_cache();
                        if let Some(first) = history.first_mut() {
                            first.content = build_system_prompt(
                                &base_system,
                                active_skill.as_deref(),
                                &skills_cache,
                            );
                        }
                    }
                }
            }
            else => break,
        }
    }
}

fn load_skills_cache() -> BTreeMap<String, Skill> {
    skills::default_skills_dir()
        .and_then(|p| skills::load_all(&p).ok())
        .unwrap_or_default()
}

fn extract_user_text(chunks: &[types::PromptChunk]) -> String {
    chunks
        .iter()
        .filter(|c| c.kind == "text")
        .filter_map(|c| c.value.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn spawn_background_capture(
    config: LlamaConfig,
    user_text: String,
    active_skill: Option<String>,
    rt_tx: mpsc::UnboundedSender<LlamaRuntimeCommand>,
) {
    tokio::task::spawn_local(async move {
        let facts = memory_capture::capture_facts(&config, &user_text).await;
        if facts.is_empty() {
            return;
        }
        let Some(path) = memory::default_db_path() else { return };
        let Ok(store) = MemoryStore::open(&path) else { return };
        let behavioral_scope = active_skill.as_deref().unwrap_or("default");
        for fact in &facts {
            let scope = if fact.kind == memory::FactKind::Behavioral {
                Some(behavioral_scope)
            } else {
                None
            };
            let _ = store.upsert(fact.kind, &fact.slug, &fact.content, scope);
        }
        tracing::info!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "memory_auto_captured",
            count = facts.len(),
            "auto-captured {} facts from user message", facts.len(),
        );
        let _ = rt_tx.send(LlamaRuntimeCommand::RebuildSystem);
    });
}

const MAX_TOOL_ITERATIONS: usize = 6;

#[allow(clippy::too_many_arguments)]
async fn handle_prompt(
    event_tx: &mpsc::UnboundedSender<crate::agent::events::ClientEvent>,
    cmd_tx: &mpsc::UnboundedSender<CommandEnvelope>,
    connected_once: &mut bool,
    history: &mut Vec<Message>,
    config: &LlamaConfig,
    session_id: String,
    chunks: Vec<types::PromptChunk>,
) {
    let user_text = extract_user_text(&chunks);
    if user_text.is_empty() {
        emit_turn_complete(event_tx, cmd_tx, connected_once, session_id);
        return;
    }

    history.push(Message::user(user_text));

    let tool_specs = tools::available_tools();

    for iteration in 0..MAX_TOOL_ITERATIONS {
        tracing::info!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "llama_prompt_iteration",
            iteration,
            tools_advertised = tool_specs.len(),
            history_len = history.len(),
            "llama prompt iteration"
        );
        let stream_result = stream_chat_with_tools(config, history, &tool_specs).await;
        let mut stream = match stream_result {
            Ok(s) => Box::pin(s),
            Err(err) => {
                if iteration == 0 {
                    history.pop();
                }
                emit_turn_error(event_tx, cmd_tx, connected_once, session_id, err.to_string());
                return;
            }
        };

        let mut assistant_text = String::new();
        let mut tool_calls: Vec<LlmToolCall> = Vec::new();
        let mut error: Option<anyhow::Error> = None;

        while let Some(item) = stream.next().await {
            match item {
                Ok(ChatEvent::Delta(text)) if text.is_empty() => {}
                Ok(ChatEvent::Delta(text)) => {
                    assistant_text.push_str(&text);
                    emit_assistant_chunk(event_tx, cmd_tx, connected_once, &session_id, text);
                }
                Ok(ChatEvent::ToolCall(tc)) => tool_calls.push(tc),
                Ok(ChatEvent::Done) => break,
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
        }
        drop(stream);

        if let Some(err) = error {
            history.push(Message::assistant(assistant_text));
            emit_turn_error(event_tx, cmd_tx, connected_once, session_id, err.to_string());
            return;
        }

        history.push(assistant_message(assistant_text, &tool_calls));

        if tool_calls.is_empty() {
            emit_turn_complete(event_tx, cmd_tx, connected_once, session_id);
            return;
        }

        for tc in &tool_calls {
            emit_tool_call_started(event_tx, cmd_tx, connected_once, &session_id, tc);
            let result = tools::execute(&tc.name, &tc.arguments).await;
            let (status, output) = match result {
                Ok(o) => ("completed", o),
                Err(e) => ("failed", format!("tool error: {e}")),
            };
            emit_tool_call_completed(
                event_tx,
                cmd_tx,
                connected_once,
                &session_id,
                tc,
                status,
                &output,
            );
            history.push(Message::tool_response(tc.id.clone(), output));
        }
    }

    emit_turn_error(
        event_tx,
        cmd_tx,
        connected_once,
        session_id,
        format!("max tool iterations ({MAX_TOOL_ITERATIONS}) reached"),
    );
}

fn assistant_message(text: String, tool_calls: &[LlmToolCall]) -> Message {
    let mut msg = Message::assistant(text);
    if !tool_calls.is_empty() {
        msg.tool_calls = Some(
            tool_calls
                .iter()
                .map(|tc| RoleToolCall {
                    id: tc.id.clone(),
                    kind: "function".to_owned(),
                    function: RoleToolCallFunction {
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    },
                })
                .collect(),
        );
    }
    msg
}

fn emit_tool_call_started(
    event_tx: &mpsc::UnboundedSender<crate::agent::events::ClientEvent>,
    cmd_tx: &mpsc::UnboundedSender<CommandEnvelope>,
    connected_once: &mut bool,
    session_id: &str,
    tc: &LlmToolCall,
) {
    let raw_input = serde_json::from_str::<serde_json::Value>(&tc.arguments).ok();
    let sdk_kind = sdk_kind_for(&tc.name);
    let tool_call = types::ToolCall {
        tool_call_id: tc.id.clone(),
        title: format!("{} {}", sdk_kind, summarize_args(&tc.arguments)),
        kind: sdk_kind.to_owned(),
        status: "in_progress".to_owned(),
        content: Vec::new(),
        raw_input,
        raw_output: None,
        output_metadata: None,
        task_metadata: None,
        locations: Vec::new(),
        meta: None,
    };
    handle_bridge_event(
        event_tx,
        cmd_tx,
        connected_once,
        false,
        EventEnvelope {
            request_id: None,
            event: BridgeEvent::SessionUpdate {
                session_id: session_id.to_owned(),
                update: types::SessionUpdate::ToolCall { tool_call },
            },
        },
    );
}

fn emit_tool_call_completed(
    event_tx: &mpsc::UnboundedSender<crate::agent::events::ClientEvent>,
    cmd_tx: &mpsc::UnboundedSender<CommandEnvelope>,
    connected_once: &mut bool,
    session_id: &str,
    tc: &LlmToolCall,
    status: &str,
    output: &str,
) {
    let content = vec![types::ToolCallContent::Content {
        content: types::ContentBlock::Text { text: output.to_owned() },
    }];
    let update = types::ToolCallUpdate {
        tool_call_id: tc.id.clone(),
        fields: types::ToolCallUpdateFields {
            status: Some(status.to_owned()),
            content: Some(content),
            raw_output: Some(output.to_owned()),
            ..Default::default()
        },
    };
    handle_bridge_event(
        event_tx,
        cmd_tx,
        connected_once,
        false,
        EventEnvelope {
            request_id: None,
            event: BridgeEvent::SessionUpdate {
                session_id: session_id.to_owned(),
                update: types::SessionUpdate::ToolCallUpdate { tool_call_update: update },
            },
        },
    );
}

fn summarize_args(args: &str) -> String {
    let trimmed = args.trim();
    if trimmed.len() <= 80 {
        trimmed.to_owned()
    } else {
        format!("{}…", &trimmed[..77])
    }
}

/// Map snake_case OpenAI tool names to PascalCase SDK names so CCR's
/// `tool_name_label` (src/ui/theme.rs) dispatches to the right icon and label
/// instead of falling through to the generic "Tool" placeholder.
fn sdk_kind_for(name: &str) -> &'static str {
    match name {
        "read_file" => "Read",
        "write_file" => "Write",
        "edit_file" => "Edit",
        "bash" | "run_command" => "Bash",
        "grep" | "search" => "Grep",
        "glob" | "find_file" => "Glob",
        "list_dir" | "ls" => "LS",
        "web_fetch" | "fetch_url" => "WebFetch",
        "web_search" => "WebSearch",
        _ => "Tool",
    }
}

#[cfg(test)]
mod tests {
    use super::sdk_kind_for;

    #[test]
    fn maps_known_tool_names_to_pascal_case() {
        assert_eq!(sdk_kind_for("read_file"), "Read");
        assert_eq!(sdk_kind_for("write_file"), "Write");
        assert_eq!(sdk_kind_for("bash"), "Bash");
        assert_eq!(sdk_kind_for("run_command"), "Bash");
        assert_eq!(sdk_kind_for("web_fetch"), "WebFetch");
    }

    #[test]
    fn falls_back_to_tool_for_unknown_names() {
        assert_eq!(sdk_kind_for("something_weird"), "Tool");
        assert_eq!(sdk_kind_for(""), "Tool");
    }
}

fn emit_assistant_chunk(
    event_tx: &mpsc::UnboundedSender<crate::agent::events::ClientEvent>,
    cmd_tx: &mpsc::UnboundedSender<CommandEnvelope>,
    connected_once: &mut bool,
    session_id: &str,
    text: String,
) {
    handle_bridge_event(
        event_tx,
        cmd_tx,
        connected_once,
        false,
        EventEnvelope {
            request_id: None,
            event: BridgeEvent::SessionUpdate {
                session_id: session_id.to_owned(),
                update: types::SessionUpdate::AgentMessageChunk {
                    content: types::ContentBlock::Text { text },
                },
            },
        },
    );
}

fn emit_turn_complete(
    event_tx: &mpsc::UnboundedSender<crate::agent::events::ClientEvent>,
    cmd_tx: &mpsc::UnboundedSender<CommandEnvelope>,
    connected_once: &mut bool,
    session_id: String,
) {
    handle_bridge_event(
        event_tx,
        cmd_tx,
        connected_once,
        false,
        EventEnvelope {
            request_id: None,
            event: BridgeEvent::TurnComplete {
                session_id,
                terminal_reason: Some(types::TerminalReason::Completed),
            },
        },
    );
}

fn emit_turn_error(
    event_tx: &mpsc::UnboundedSender<crate::agent::events::ClientEvent>,
    cmd_tx: &mpsc::UnboundedSender<CommandEnvelope>,
    connected_once: &mut bool,
    session_id: String,
    message: String,
) {
    handle_bridge_event(
        event_tx,
        cmd_tx,
        connected_once,
        false,
        EventEnvelope {
            request_id: None,
            event: BridgeEvent::TurnError {
                session_id,
                message,
                error_kind: None,
                sdk_result_subtype: None,
                assistant_error: None,
                terminal_reason: Some(types::TerminalReason::ModelError),
            },
        },
    );
}

fn build_system_prompt(
    base: &str,
    active_skill: Option<&str>,
    skills_map: &BTreeMap<String, Skill>,
) -> String {
    let mut out = String::new();
    if let Some(path) = memory::default_db_path() {
        if let Ok(store) = MemoryStore::open(&path) {
            let facts = store.list(None).unwrap_or_default();
            let section = render_memory_section(&facts, active_skill);
            if !section.is_empty() {
                out.push_str(&section);
            }
        }
    }
    out.push_str(base);
    if let Some(name) = active_skill {
        if let Some(skill) = skills_map.get(name) {
            out.push_str(&format!("\n\n[Active skill: {name}]\n{}\n", skill.body));
        }
    }
    out
}

fn synth_initialize_result() -> types::InitializeResult {
    types::InitializeResult {
        agent_name: "mochi-llama".to_owned(),
        agent_version: env!("CARGO_PKG_VERSION").to_owned(),
        auth_methods: Vec::new(),
        capabilities: types::AgentCapabilities {
            prompt_image: false,
            prompt_embedded_context: false,
            supports_session_listing: false,
            supports_resume_session: false,
        },
    }
}

fn synth_current_model(url: &str) -> types::CurrentModel {
    types::CurrentModel {
        requested_id: None,
        resolved_id: format!("llama@{}", url.trim_start_matches("http://").trim_start_matches("https://")),
        display_name_short: "llama".to_owned(),
        display_name_long: format!("local llama.cpp ({url})"),
        catalog_id: None,
        supports_effort: false,
        supported_effort_levels: Vec::new(),
        supports_fast_mode: None,
        supports_auto_mode: None,
        supports_adaptive_thinking: None,
        is_authoritative: true,
    }
}

fn synth_available_model(url: &str) -> types::AvailableModel {
    types::AvailableModel {
        id: format!("llama@{}", url.trim_start_matches("http://").trim_start_matches("https://")),
        display_name: "local llama.cpp".to_owned(),
        description: Some(url.to_owned()),
        supports_effort: false,
        supported_effort_levels: Vec::new(),
        supports_adaptive_thinking: None,
        supports_fast_mode: None,
        supports_auto_mode: None,
    }
}

//! Llama provider lifecycle: replaces the Node bridge with a direct llama.cpp
//! HTTP+SSE client and synthesizes the BridgeEvents the existing TUI expects.

use crate::agent::client::AgentConnection;
use crate::agent::llama_client::{ChatEvent, LlamaConfig, Message, Role, stream_chat};
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

    let mut history: Vec<Message> = vec![Message {
        role: Role::System,
        content: build_system_prompt(&base_system, active_skill.as_deref(), &skills_cache),
    }];

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
    let user_text = chunks
        .iter()
        .filter(|c| c.kind == "text")
        .filter_map(|c| c.value.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if user_text.is_empty() {
        emit_turn_complete(event_tx, cmd_tx, connected_once, session_id);
        return;
    }

    history.push(Message { role: Role::User, content: user_text });

    let mut stream = match stream_chat(config, history).await {
        Ok(s) => Box::pin(s),
        Err(err) => {
            history.pop();
            emit_turn_error(event_tx, cmd_tx, connected_once, session_id, err.to_string());
            return;
        }
    };

    let mut assistant = String::new();
    let mut error: Option<anyhow::Error> = None;

    while let Some(item) = stream.next().await {
        match item {
            Ok(ChatEvent::Delta(text)) if text.is_empty() => {}
            Ok(ChatEvent::Delta(text)) => {
                assistant.push_str(&text);
                emit_assistant_chunk(event_tx, cmd_tx, connected_once, &session_id, text);
            }
            Ok(ChatEvent::Done) => break,
            Err(err) => {
                error = Some(err);
                break;
            }
        }
    }

    drop(stream);

    if let Some(err) = error {
        if assistant.is_empty() {
            history.pop();
        } else {
            history.push(Message { role: Role::Assistant, content: assistant });
        }
        emit_turn_error(event_tx, cmd_tx, connected_once, session_id, err.to_string());
        return;
    }

    history.push(Message { role: Role::Assistant, content: assistant });
    emit_turn_complete(event_tx, cmd_tx, connected_once, session_id);
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

//! Llama provider lifecycle: replaces the Node bridge with a direct llama.cpp
//! HTTP+SSE client and synthesizes the BridgeEvents the existing TUI expects.

use crate::agent::client::AgentConnection;
use crate::agent::llama_client::{
    ChatEvent, LlamaConfig, LlmToolCall, Message, RoleToolCall, RoleToolCallFunction,
    stream_chat_with_tools,
};
use crate::agent::types;
use crate::agent::wire::{BridgeCommand, BridgeEvent, CommandEnvelope, EventEnvelope};
use crate::memory::{self, MemoryStore, render_memory_section};
use crate::memory_capture;
use crate::memory_judge::{self, JudgeOutcome};
use crate::skills::{self, Skill};
use crate::tools;
use futures::StreamExt as _;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use tokio::sync::mpsc;

/// Out-of-band control messages from slash handlers to the running llama task.
/// Used to trigger system-prompt rebuilds when memory or skill state changes mid-session.
#[derive(Debug, Clone)]
pub enum LlamaRuntimeCommand {
    RebuildSystem,
    SetActiveSkill(Option<String>),
    /// Switch the underlying llama-server URL (used when `/provider` swaps the
    /// managed model). Future requests go to the new endpoint.
    SetLlamaUrl(String),
    /// Switch memory injection strategy. `All` = dump every fact into the
    /// system prompt at session start (current behavior). `Active` = run a
    /// per-turn query proposal + match, inject only relevant facts.
    SetMemoryMode(MemoryMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryMode {
    All,
    Active,
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

    let mut config = LlamaConfig {
        url: params.llama_url.clone(),
        model: None,
        temperature: Some(params.llama_temperature),
        max_tokens: Some(1024),
    };

    let base_system =
        "You are Mochi, a cute terminal pet companion. Reply concisely (1-3 sentences) and stay in character.".to_owned();
    let mut active_skill: Option<String> = None;
    let mut skills_cache: BTreeMap<String, Skill> = load_skills_cache();
    let mut memory_mode: MemoryMode = MemoryMode::All;

    let mut history: Vec<Message> = vec![Message::system(build_system_prompt(
        &base_system,
        active_skill.as_deref(),
        &skills_cache,
    ))];
    let mut allow_set: HashSet<String> = HashSet::new();

    loop {
        tokio::select! {
            Some(envelope) = cmd_rx.recv() => {
                match envelope.command {
                    BridgeCommand::Prompt { session_id: prompt_session, chunks } => {
                        let user_text = extract_user_text(&chunks);
                        if !user_text.is_empty() {
                            // When memory_mode is Active, rebuild history[0] with only
                            // the facts the LLM thinks are relevant to this turn.
                            // Falls back to the cached full-dump system prompt on any
                            // failure so we never block a prompt waiting on memory.
                            if memory_mode == MemoryMode::Active {
                                let new_system = build_active_system_prompt(
                                    &config,
                                    &base_system,
                                    active_skill.as_deref(),
                                    &skills_cache,
                                    &user_text,
                                )
                                .await;
                                if let Some(first) = history.first_mut() {
                                    first.content = new_system;
                                }
                            }
                            spawn_background_capture(
                                config.clone(),
                                user_text.clone(),
                                active_skill.clone(),
                                rt_tx.clone(),
                            );
                        }
                        handle_prompt(
                            &params.event_tx,
                            &cmd_tx,
                            &mut cmd_rx,
                            &mut connected_once,
                            &mut allow_set,
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
                    LlamaRuntimeCommand::SetLlamaUrl(new_url) => {
                        tracing::info!(
                            target: crate::logging::targets::APP_LIFECYCLE,
                            event_name = "llama_url_switched",
                            old_url = %config.url,
                            new_url = %new_url,
                        );
                        config.url = new_url;
                    }
                    LlamaRuntimeCommand::SetMemoryMode(new_mode) => {
                        tracing::info!(
                            target: crate::logging::targets::APP_LIFECYCLE,
                            event_name = "memory_mode_switched",
                            mode = ?new_mode,
                        );
                        memory_mode = new_mode;
                    }
                }
            }
            else => break,
        }
    }
}

fn fallback_slug_scope(
    fact: &crate::memory_capture::CapturedFact,
    behavioral_scope: &str,
) -> (String, Option<String>) {
    let scope = if fact.kind == memory::FactKind::Behavioral {
        Some(behavioral_scope.to_owned())
    } else {
        None
    };
    (fact.slug.clone(), scope)
}

fn load_skills_cache() -> BTreeMap<String, Skill> {
    skills::default_skills_dir().and_then(|p| skills::load_all(&p).ok()).unwrap_or_default()
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
            // BOOKMARKS-style judge: against existing same-kind facts, decide
            // reuse/derive/new. On reuse, overwrite the matched fact's content
            // by upserting under its existing slug. Otherwise insert fresh.
            let existing = store.list(Some(fact.kind)).unwrap_or_default();
            let existing_refs: Vec<&memory::Fact> = existing.iter().collect();
            let outcome = memory_judge::judge_capture(&config, fact, &existing_refs).await;

            let (target_slug, target_scope) = match outcome {
                JudgeOutcome::Reuse { existing_id } => {
                    let matched = existing.iter().find(|f| f.id == existing_id);
                    match matched {
                        Some(m) => (m.slug.clone(), m.skill_scope.clone()),
                        None => fallback_slug_scope(fact, behavioral_scope),
                    }
                }
                JudgeOutcome::Derive { .. } | JudgeOutcome::New => {
                    fallback_slug_scope(fact, behavioral_scope)
                }
            };
            let _ = store.upsert(fact.kind, &target_slug, &fact.content, target_scope.as_deref());
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

#[derive(Debug, Clone, Copy)]
enum PermissionDecision {
    AllowOnce,
    AllowSession,
    Reject,
}

#[allow(clippy::too_many_arguments)]
async fn handle_prompt(
    event_tx: &mpsc::UnboundedSender<crate::agent::events::ClientEvent>,
    cmd_tx: &mpsc::UnboundedSender<CommandEnvelope>,
    cmd_rx: &mut mpsc::UnboundedReceiver<CommandEnvelope>,
    connected_once: &mut bool,
    allow_set: &mut HashSet<String>,
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
            let allowed = if tools::needs_permission(&tc.name) && !allow_set.contains(&tc.name) {
                match request_permission(event_tx, cmd_tx, cmd_rx, connected_once, &session_id, tc)
                    .await
                {
                    PermissionDecision::AllowOnce => true,
                    PermissionDecision::AllowSession => {
                        allow_set.insert(tc.name.clone());
                        true
                    }
                    PermissionDecision::Reject => false,
                }
            } else {
                true
            };

            emit_tool_call_started(event_tx, cmd_tx, connected_once, &session_id, tc);

            if !allowed {
                let denial = "User denied permission for this tool call.";
                let denial_block = vec![types::ToolCallContent::Content {
                    content: types::ContentBlock::Text { text: denial.to_owned() },
                }];
                emit_tool_call_completed(
                    event_tx,
                    cmd_tx,
                    connected_once,
                    &session_id,
                    tc,
                    "failed",
                    denial_block,
                    denial,
                );
                history.push(Message::tool_response(tc.id.clone(), denial.to_owned()));
                continue;
            }

            let result = tools::execute(&tc.name, &tc.arguments).await;
            let (status, ui_content, model_text) = match result {
                Ok(r) => ("completed", r.ui_content, r.model_text),
                Err(e) => {
                    let msg = format!("tool error: {e}");
                    let block = vec![types::ToolCallContent::Content {
                        content: types::ContentBlock::Text { text: msg.clone() },
                    }];
                    ("failed", block, msg)
                }
            };
            emit_tool_call_completed(
                event_tx,
                cmd_tx,
                connected_once,
                &session_id,
                tc,
                status,
                ui_content,
                &model_text,
            );
            history.push(Message::tool_response(tc.id.clone(), model_text));
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
    let tool_call = types::ToolCall {
        tool_call_id: tc.id.clone(),
        title: format!("{} {}", tc.name, summarize_args(&tc.arguments)),
        kind: tc.name.clone(),
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

#[allow(clippy::too_many_arguments)]
fn emit_tool_call_completed(
    event_tx: &mpsc::UnboundedSender<crate::agent::events::ClientEvent>,
    cmd_tx: &mpsc::UnboundedSender<CommandEnvelope>,
    connected_once: &mut bool,
    session_id: &str,
    tc: &LlmToolCall,
    status: &str,
    ui_content: Vec<types::ToolCallContent>,
    raw_output: &str,
) {
    let update = types::ToolCallUpdate {
        tool_call_id: tc.id.clone(),
        fields: types::ToolCallUpdateFields {
            status: Some(status.to_owned()),
            content: Some(ui_content),
            raw_output: Some(raw_output.to_owned()),
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
    if trimmed.len() <= 80 { trimmed.to_owned() } else { format!("{}…", &trimmed[..77]) }
}

async fn request_permission(
    event_tx: &mpsc::UnboundedSender<crate::agent::events::ClientEvent>,
    cmd_tx: &mpsc::UnboundedSender<CommandEnvelope>,
    cmd_rx: &mut mpsc::UnboundedReceiver<CommandEnvelope>,
    connected_once: &mut bool,
    session_id: &str,
    tc: &LlmToolCall,
) -> PermissionDecision {
    let raw_input = serde_json::from_str::<serde_json::Value>(&tc.arguments).ok();
    let permission_tool_call = types::ToolCall {
        tool_call_id: tc.id.clone(),
        title: format!("{} {}", tc.name, summarize_args(&tc.arguments)),
        kind: tc.name.clone(),
        status: "pending".to_owned(),
        content: Vec::new(),
        raw_input,
        raw_output: None,
        output_metadata: None,
        task_metadata: None,
        locations: Vec::new(),
        meta: None,
    };
    let options = vec![
        types::PermissionOption {
            option_id: "allow_once".to_owned(),
            name: "Allow once".to_owned(),
            description: None,
            kind: "allow_once".to_owned(),
        },
        types::PermissionOption {
            option_id: "allow_session".to_owned(),
            name: format!("Allow {} for this session", tc.name),
            description: None,
            kind: "allow_session".to_owned(),
        },
        types::PermissionOption {
            option_id: "reject_once".to_owned(),
            name: "Reject".to_owned(),
            description: None,
            kind: "reject_once".to_owned(),
        },
    ];
    let request =
        types::PermissionRequest { tool_call: permission_tool_call, options, display: None };

    handle_bridge_event(
        event_tx,
        cmd_tx,
        connected_once,
        false,
        EventEnvelope {
            request_id: None,
            event: BridgeEvent::PermissionRequest { session_id: session_id.to_owned(), request },
        },
    );

    while let Some(env) = cmd_rx.recv().await {
        match env.command {
            BridgeCommand::PermissionResponse { tool_call_id, outcome, .. }
                if tool_call_id == tc.id =>
            {
                return match outcome {
                    types::PermissionOutcome::Selected { option_id } => match option_id.as_str() {
                        "allow_once" => PermissionDecision::AllowOnce,
                        "allow_session" => PermissionDecision::AllowSession,
                        _ => PermissionDecision::Reject,
                    },
                    types::PermissionOutcome::Cancelled => PermissionDecision::Reject,
                };
            }
            BridgeCommand::CancelTurn { .. } => return PermissionDecision::Reject,
            _ => {
                // Drop unrelated commands during permission wait (v0.1 simplification).
            }
        }
    }
    PermissionDecision::Reject
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

/// Active-mode system prompt: call the BOOKMARKS-style query proposer,
/// match the proposed queries against the store, and render ONLY the
/// matched facts (plus profile, which is always relevant). Falls back to
/// the all-facts builder if the memory store or LLM call fails.
async fn build_active_system_prompt(
    config: &LlamaConfig,
    base: &str,
    active_skill: Option<&str>,
    skills_map: &BTreeMap<String, Skill>,
    user_text: &str,
) -> String {
    let Some(path) = memory::default_db_path() else {
        return build_system_prompt(base, active_skill, skills_map);
    };
    let Ok(store) = MemoryStore::open(&path) else {
        return build_system_prompt(base, active_skill, skills_map);
    };

    let (queries, mut facts) =
        crate::memory_query::fetch_relevant_facts(config, user_text, &store, active_skill).await;

    // Profile is small and almost always useful; always include it even if
    // the proposer didn't ask for it explicitly.
    if let Ok(profile_facts) = store.list(Some(memory::FactKind::Profile)) {
        for p in profile_facts {
            if !facts.iter().any(|f| f.id == p.id) {
                facts.push(p);
            }
        }
    }

    tracing::info!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "memory_active_injected",
        queries = queries.len(),
        facts = facts.len(),
    );

    let mut out = String::new();
    if !facts.is_empty() {
        let section = render_memory_section(&facts, active_skill);
        if !section.is_empty() {
            out.push_str(&section);
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
        resolved_id: format!(
            "llama@{}",
            url.trim_start_matches("http://").trim_start_matches("https://")
        ),
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

// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Slash command executors: dispatching parsed commands to their handler functions.

use super::{
    parse, push_system_message, push_user_message, require_active_session, require_connection,
    set_command_pending,
};
use crate::agent::events::ClientEvent;
use crate::app::config::{self, SettingFile, store};
use crate::app::connect::{SessionStartReason, begin_resume_session, start_new_session};
use crate::app::events::push_system_message_with_severity;
use crate::app::{App, AppStatus, CancelOrigin, SystemSeverity};
use std::fmt::Write as _;

const ONE_M_CONTEXT_USAGE: &str = "Usage: /1m-context <enable|disable|status>";
const OPUS_VERSION_USAGE: &str = "Usage: /opus-version <4.5|4.6|4.7|default|status>";
const OPUS_4_5_MODEL_ID: &str = "claude-opus-4-5-20251101";
const OPUS_4_6_MODEL_ID: &str = "claude-opus-4-6";
const OPUS_4_7_MODEL_ID: &str = "claude-opus-4-7";

/// Handle slash command submission.
///
/// Returns `true` if the slash input was fully handled and should not be sent as a prompt.
/// Returns `false` when the input should continue through the normal prompt path.
pub fn try_handle_submit(app: &mut App, text: &str) -> bool {
    let Some(parsed) = parse(text) else {
        return false;
    };

    match parsed.name {
        "/1m-context" => handle_1m_context_submit(app, &parsed.args),
        "/cancel" => handle_cancel_submit(app),
        "/compact" => handle_compact_submit(app, &parsed.args),
        "/config" => handle_config_submit(app, &parsed.args),
        "/docs" => handle_docs_submit(app, &parsed.args),
        "/mcp" => handle_mcp_submit(app, &parsed.args),
        "/plugins" => handle_plugins_submit(app, &parsed.args),
        "/opus-version" => handle_opus_version_submit(app, &parsed.args),
        "/status" => handle_status_submit(app, &parsed.args),
        "/usage" => handle_usage_submit(app, &parsed.args),
        "/login" => handle_login_submit(app, &parsed.args),
        "/logout" => handle_logout_submit(app, &parsed.args),
        "/mode" => handle_mode_submit(app, &parsed.args),
        "/model" => handle_model_submit(app, &parsed.args),
        "/new-session" => handle_new_session_submit(app, &parsed.args),
        "/resume" => handle_resume_submit(app, &parsed.args),
        "/memory" => handle_memory_submit(app, text, &parsed.args),
        "/skill" => handle_skill_submit(app, &parsed.args),
        "/pet" => handle_pet_submit(app, &parsed.args),
        "/clear" => handle_clear_submit(app),
        "/provider" => handle_provider_submit(app, &parsed.args),
        _ => handle_unknown_submit(app, parsed.name),
    }
}

fn info(app: &mut App, msg: impl Into<String>) {
    let s = msg.into();
    push_system_message_with_severity(app, Some(SystemSeverity::Info), &s);
}

fn warn(app: &mut App, msg: impl Into<String>) {
    let s = msg.into();
    push_system_message_with_severity(app, Some(SystemSeverity::Warning), &s);
}

fn signal_runtime(app: &App, cmd: crate::app::connect::llama_lifecycle::LlamaRuntimeCommand) {
    if let Some(tx) = app.llama_runtime_tx.as_ref() {
        let _ = tx.send(cmd);
    }
}

fn handle_memory_submit(app: &mut App, raw: &str, args: &[&str]) -> bool {
    use crate::memory::{FactKind, MemoryStore, default_db_path};
    let Some(path) = default_db_path() else {
        warn(app, "memory: no home dir; cannot open ~/.mochi/memory.");
        return true;
    };
    let store = match MemoryStore::open(&path) {
        Ok(s) => s,
        Err(e) => {
            warn(app, format!("memory: open failed: {e}"));
            return true;
        }
    };

    let sub = args.first().copied().unwrap_or("list");
    match sub {
        "list" => {
            let kind = args.get(1).and_then(|s| FactKind::parse(s));
            match store.list(kind) {
                Ok(facts) if facts.is_empty() => {
                    info(app, "memory: no facts stored.");
                }
                Ok(facts) => {
                    let mut out = String::from("memory:\n");
                    for f in facts {
                        out.push_str(&format!(
                            "  [{}] {} (scope={})  {}\n",
                            f.kind.as_str(),
                            f.slug,
                            f.skill_scope.as_deref().unwrap_or("-"),
                            f.content
                        ));
                    }
                    info(app, out);
                }
                Err(e) => {
                    info(app, format!("memory: list failed: {e}"));
                }
            }
        }
        "remember" => {
            let Some(kind) = args.get(1).and_then(|s| FactKind::parse(s)) else {
                info(
                    app,
                    "usage: /memory remember KIND SLUG CONTENT  (KIND: profile|concept|state|behavioral)",
                );
                return true;
            };
            let Some(&slug) = args.get(2) else {
                info(app, "usage: /memory remember KIND SLUG CONTENT");
                return true;
            };
            let content = raw.splitn(5, char::is_whitespace).nth(4).unwrap_or("").trim();
            if content.is_empty() {
                info(app, "usage: /memory remember KIND SLUG CONTENT");
                return true;
            }
            let mut wrote = false;
            match store.upsert(kind, slug, content, None) {
                Ok(_) => {
                    wrote = true;
                    info(app, format!("remembered [{}] {slug}", kind.as_str()));
                }
                Err(e) => warn(app, format!("memory: write failed: {e}")),
            }
            if wrote {
                signal_runtime(
                    app,
                    crate::app::connect::llama_lifecycle::LlamaRuntimeCommand::RebuildSystem,
                );
            }
        }
        "forget" => {
            let Some(&slug) = args.get(1) else {
                info(app, "usage: /memory forget SLUG");
                return true;
            };
            let mut wrote = false;
            match store.forget(slug) {
                Ok(0) => info(app, format!("no fact named `{slug}`")),
                Ok(n) => {
                    wrote = true;
                    info(app, format!("forgot {n} fact(s) `{slug}`"));
                }
                Err(e) => warn(app, format!("memory: forget failed: {e}")),
            }
            if wrote {
                signal_runtime(
                    app,
                    crate::app::connect::llama_lifecycle::LlamaRuntimeCommand::RebuildSystem,
                );
            }
        }
        "profile" => {
            let content = raw.splitn(3, char::is_whitespace).nth(2).unwrap_or("").trim();
            if content.is_empty() {
                match store.profile() {
                    Ok(Some(p)) => info(app, format!("profile:\n{p}")),
                    Ok(None) => info(app, "no profile set. usage: /memory profile <text>"),
                    Err(e) => warn(app, format!("memory: read failed: {e}")),
                }
            } else {
                match store.upsert(FactKind::Profile, "user", content, None) {
                    Ok(_) => {
                        info(app, "profile updated.");
                        signal_runtime(
                            app,
                            crate::app::connect::llama_lifecycle::LlamaRuntimeCommand::RebuildSystem,
                        );
                    }
                    Err(e) => warn(app, format!("memory: write failed: {e}")),
                }
            }
        }
        "consolidate" => {
            if !matches!(app.provider, crate::Provider::Llamacpp) {
                warn(app, "memory consolidate requires llamacpp provider");
                return true;
            }
            spawn_memory_consolidate(app);
        }
        "query" => {
            let scene = raw.splitn(3, char::is_whitespace).nth(2).unwrap_or("").trim();
            if scene.is_empty() {
                info(
                    app,
                    "usage: /memory query <text>\n(LLM proposes search queries against memory and returns matched facts)",
                );
                return true;
            }
            if !matches!(app.provider, crate::Provider::Llamacpp) {
                warn(app, "memory query requires llamacpp provider");
                return true;
            }
            spawn_memory_query(app, scene.to_owned());
        }
        "mode" => {
            let mode_arg = args.get(2).copied().unwrap_or("").to_lowercase();
            let mode = match mode_arg.as_str() {
                "active" => crate::app::connect::llama_lifecycle::MemoryMode::Active,
                "all" | "" => crate::app::connect::llama_lifecycle::MemoryMode::All,
                _ => {
                    info(app, "usage: /memory mode active|all");
                    return true;
                }
            };
            if let Some(tx) = app.llama_runtime_tx.as_ref() {
                let _ = tx.send(
                    crate::app::connect::llama_lifecycle::LlamaRuntimeCommand::SetMemoryMode(mode),
                );
            }
            let label = match mode {
                crate::app::connect::llama_lifecycle::MemoryMode::All => "all (inject every fact)",
                crate::app::connect::llama_lifecycle::MemoryMode::Active => {
                    "active (LLM-driven per-turn query proposal)"
                }
            };
            info(app, format!("memory mode: {label}"));
        }
        "restate" => {
            let slug = args.get(2).copied().unwrap_or("").trim();
            if slug.is_empty() {
                info(
                    app,
                    "usage: /memory restate <slug>\n(LLM rescans recent chat history and rewrites the state fact's answer)",
                );
                return true;
            }
            if !matches!(app.provider, crate::Provider::Llamacpp) {
                warn(app, "memory restate requires llamacpp provider");
                return true;
            }
            spawn_memory_restate(app, slug.to_owned());
        }
        "observe" => {
            let query = raw.splitn(3, char::is_whitespace).nth(2).unwrap_or("").trim();
            if query.is_empty() {
                info(
                    app,
                    "usage: /memory observe <behavioral query>\n(LLM scans past user messages, summarizes the user's pattern matching the query)",
                );
                return true;
            }
            if !matches!(app.provider, crate::Provider::Llamacpp) {
                warn(app, "memory observe requires llamacpp provider");
                return true;
            }
            spawn_memory_observe(app, query.to_owned());
        }
        _ => {
            info(
                app,
                "memory: subcommands are list | remember KIND SLUG CONTENT | forget SLUG | profile [TEXT] | consolidate | query <text> | mode active|all | restate <slug> | observe <behavioral query>",
            );
        }
    }
    true
}

fn collect_recent_user_messages(app: &App, max: usize) -> Vec<String> {
    app.messages
        .iter()
        .rev()
        .filter(|m| matches!(m.role, crate::app::MessageRole::User))
        .filter_map(|m| {
            let mut parts: Vec<String> = Vec::new();
            for block in &m.blocks {
                if let crate::app::MessageBlock::Text(t) = block {
                    parts.push(t.text.clone());
                }
            }
            if parts.is_empty() { None } else { Some(parts.join("\n")) }
        })
        .take(max)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn spawn_memory_restate(app: &mut App, slug: String) {
    let recent = collect_recent_user_messages(app, 20);
    if recent.is_empty() {
        info(app, "memory restate: no recent user messages to base the update on");
        return;
    }
    info(app, format!("memory: restating state[{slug}] from recent chat history... (~3-5s)"));
    let url = app.llama_url.clone();
    let event_tx = app.event_tx.clone();
    let rt_tx = app.llama_runtime_tx.clone();
    tokio::task::spawn_local(async move {
        let Some(path) = crate::memory::default_db_path() else { return };
        let Ok(store) = crate::memory::MemoryStore::open(&path) else { return };
        let existing = match store.list(Some(crate::memory::FactKind::State)) {
            Ok(v) => v,
            Err(_) => return,
        };
        let Some(state_fact) = existing.iter().find(|f| f.slug == slug) else {
            let _ = event_tx.send(crate::agent::events::ClientEvent::SlashCommandError(format!(
                "memory restate: no state fact named `{slug}` (use /memory list state)"
            )));
            return;
        };
        let config = crate::agent::llama_client::LlamaConfig {
            url,
            model: None,
            temperature: Some(0.1),
            max_tokens: Some(200),
        };
        let Some(new_answer) = crate::memory_judge::restate_from_history(
            &config,
            &state_fact.slug,
            &state_fact.content,
            &recent,
        )
        .await
        else {
            let _ = event_tx.send(crate::agent::events::ClientEvent::SlashCommandError(
                "memory restate: llama returned empty answer".to_owned(),
            ));
            return;
        };
        if new_answer == state_fact.content {
            let _ = event_tx.send(crate::agent::events::ClientEvent::SlashCommandError(format!(
                "memory restate[{slug}]: no change ({new_answer})"
            )));
            return;
        }
        match store.upsert(crate::memory::FactKind::State, &slug, &new_answer, None) {
            Ok(_) => {
                if let Some(tx) = rt_tx.as_ref() {
                    let _ = tx.send(
                        crate::app::connect::llama_lifecycle::LlamaRuntimeCommand::RebuildSystem,
                    );
                }
                let _ = event_tx.send(crate::agent::events::ClientEvent::SlashCommandError(
                    format!("memory restate[{slug}]: {new_answer}"),
                ));
            }
            Err(err) => {
                let _ = event_tx.send(crate::agent::events::ClientEvent::SlashCommandError(
                    format!("memory restate: store write failed: {err}"),
                ));
            }
        }
    });
}

fn spawn_memory_observe(app: &mut App, behavior_query: String) {
    let recent = collect_recent_user_messages(app, 20);
    if recent.is_empty() {
        info(app, "memory observe: no recent user messages to observe");
        return;
    }
    info(app, format!("memory: observing behavioral pattern for `{behavior_query}`... (~3-5s)"));
    let url = app.llama_url.clone();
    let event_tx = app.event_tx.clone();
    let rt_tx = app.llama_runtime_tx.clone();
    let active_skill_clone = app.managed_llama_server.borrow().as_ref().map(|_| String::new()); // placeholder

    // Behavioral facts are scoped per active skill. We don't have a direct
    // accessor on App here, so default to "default" scope for v0.1.
    let _ = active_skill_clone;
    let scope = "default".to_owned();

    tokio::task::spawn_local(async move {
        let config = crate::agent::llama_client::LlamaConfig {
            url,
            model: None,
            temperature: Some(0.1),
            max_tokens: Some(200),
        };
        let Some(pattern) =
            crate::memory_judge::observe_behavioral_pattern(&config, &behavior_query, &recent)
                .await
        else {
            let _ = event_tx.send(crate::agent::events::ClientEvent::SlashCommandError(format!(
                "memory observe[{behavior_query}]: no clear pattern from recent messages"
            )));
            return;
        };
        let slug = slugify_query(&behavior_query);
        let Some(path) = crate::memory::default_db_path() else { return };
        let Ok(store) = crate::memory::MemoryStore::open(&path) else { return };
        match store.upsert(crate::memory::FactKind::Behavioral, &slug, &pattern, Some(&scope)) {
            Ok(_) => {
                if let Some(tx) = rt_tx.as_ref() {
                    let _ = tx.send(
                        crate::app::connect::llama_lifecycle::LlamaRuntimeCommand::RebuildSystem,
                    );
                }
                let _ = event_tx.send(crate::agent::events::ClientEvent::SlashCommandError(
                    format!("memory observe[{slug}]: {pattern}"),
                ));
            }
            Err(err) => {
                let _ = event_tx.send(crate::agent::events::ClientEvent::SlashCommandError(
                    format!("memory observe: store write failed: {err}"),
                ));
            }
        }
    });
}

fn slugify_query(s: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for ch in s.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if ch.is_whitespace() || ch == '-' || ch == '_' {
            if !last_dash {
                out.push('-');
                last_dash = true;
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "observation".to_owned() } else { out.chars().take(40).collect() }
}

fn spawn_memory_query(app: &mut App, scene: String) {
    info(
        app,
        format!(
            "memory: proposing queries for scene `{}` ...",
            scene.chars().take(60).collect::<String>()
        ),
    );
    let url = app.llama_url.clone();
    let event_tx = app.event_tx.clone();
    tokio::task::spawn_local(async move {
        let Some(path) = crate::memory::default_db_path() else { return };
        let Ok(store) = crate::memory::MemoryStore::open(&path) else { return };
        let config = crate::agent::llama_client::LlamaConfig {
            url,
            model: None,
            temperature: Some(0.0),
            max_tokens: Some(200),
        };
        let (queries, facts) =
            crate::memory_query::fetch_relevant_facts(&config, &scene, &store, None).await;

        let mut out = String::from("memory query proposal:\n");
        if queries.is_empty() {
            out.push_str("  (LLM proposed no queries — scene may need no memory context)\n");
        } else {
            for q in &queries {
                out.push_str(&format!("  {} | {}\n", q.tag.as_str(), q.query));
            }
        }
        out.push_str("\nmatched facts:\n");
        if facts.is_empty() {
            out.push_str("  (no facts matched)\n");
        } else {
            for f in &facts {
                let scope = f.skill_scope.as_deref().unwrap_or("-");
                out.push_str(&format!(
                    "  [{:<10}] {} (scope={})  {}\n",
                    f.kind.as_str(),
                    f.slug,
                    scope,
                    f.content
                ));
            }
        }
        // Reuse SlashCommandError as a notification channel — pushes a system
        // message visible to the user. Severity styling is acceptable cost
        // until we add a dedicated Info-severity wire event.
        let _ = event_tx.send(crate::agent::events::ClientEvent::SlashCommandError(out));
    });
}

fn spawn_memory_consolidate(app: &mut App) {
    info(
        app,
        "memory: consolidating into narrative profile via local llama... (this may take ~10s)",
    );
    let url = app.llama_url.clone();
    let temperature = 0.2_f32;
    let event_tx = app.event_tx.clone();
    let rt_tx = app.llama_runtime_tx.clone();
    tokio::task::spawn_local(async move {
        let Some(path) = crate::memory::default_db_path() else { return };
        let Ok(store) = crate::memory::MemoryStore::open(&path) else { return };
        let facts = match store.list(None) {
            Ok(v) if !v.is_empty() => v,
            _ => {
                let _ = event_tx.send(crate::agent::events::ClientEvent::SlashCommandError(
                    "memory consolidate: no facts to consolidate".to_owned(),
                ));
                return;
            }
        };
        let config = crate::agent::llama_client::LlamaConfig {
            url,
            model: None,
            temperature: Some(temperature),
            max_tokens: Some(400),
        };
        let Some(narrative) = crate::memory_judge::consolidate_profile(&config, &facts).await
        else {
            let _ = event_tx.send(crate::agent::events::ClientEvent::SlashCommandError(
                "memory consolidate: llama returned empty profile".to_owned(),
            ));
            return;
        };
        match store.upsert(crate::memory::FactKind::Profile, "user", &narrative, None) {
            Ok(_) => {
                if let Some(tx) = rt_tx.as_ref() {
                    let _ = tx.send(
                        crate::app::connect::llama_lifecycle::LlamaRuntimeCommand::RebuildSystem,
                    );
                }
                tracing::info!(
                    target: crate::logging::targets::APP_SESSION,
                    event_name = "memory_profile_consolidated",
                    bytes = narrative.len(),
                    "profile consolidated from {} facts",
                    facts.len(),
                );
            }
            Err(err) => {
                let _ = event_tx.send(crate::agent::events::ClientEvent::SlashCommandError(
                    format!("memory consolidate: store write failed: {err}"),
                ));
            }
        }
    });
}

fn handle_skill_submit(app: &mut App, args: &[&str]) -> bool {
    use crate::skills;
    let Some(skills_dir) = skills::default_skills_dir() else {
        info(app, "skill: no home dir; cannot read ~/.mochi/skills.");
        return true;
    };
    let map = match skills::load_all(&skills_dir) {
        Ok(m) => m,
        Err(e) => {
            info(app, format!("skill: load failed: {e}"));
            return true;
        }
    };
    let sub = args.first().copied().unwrap_or("list");
    match sub {
        "list" | "" => {
            if map.is_empty() {
                info(
                    app,
                    "no skills installed. Put SKILL.md under ~/.mochi/skills/<name>/SKILL.md",
                );
            } else {
                let mut out = String::from("skills:\n");
                for (name, s) in &map {
                    let desc =
                        if s.description.is_empty() { "(no description)" } else { &s.description };
                    out.push_str(&format!("  - {name:<20} {desc}\n"));
                }
                info(app, out);
            }
        }
        "show" => {
            let Some(&name) = args.get(1) else {
                info(app, "usage: /skill show NAME");
                return true;
            };
            match map.get(name) {
                Some(s) => info(app, format!("--- {name} ---\n{}", s.body)),
                None => info(app, format!("unknown skill: {name}")),
            }
        }
        "use" => {
            let Some(&name) = args.get(1) else {
                info(app, "usage: /skill use NAME");
                return true;
            };
            if !map.contains_key(name) {
                info(app, format!("unknown skill: {name}"));
                return true;
            }
            signal_runtime(
                app,
                crate::app::connect::llama_lifecycle::LlamaRuntimeCommand::SetActiveSkill(Some(
                    name.to_owned(),
                )),
            );
            info(app, format!("active skill: {name}"));
        }
        "off" => {
            signal_runtime(
                app,
                crate::app::connect::llama_lifecycle::LlamaRuntimeCommand::SetActiveSkill(None),
            );
            info(app, "skill deactivated.");
        }
        _ => {
            info(app, "skill: subcommands are list | show NAME | use NAME | off");
        }
    }
    true
}

fn handle_provider_submit(app: &mut App, args: &[&str]) -> bool {
    let sub = args.first().copied().unwrap_or("show");
    match sub {
        "show" | "" => {
            let provider = match app.provider {
                crate::Provider::Anthropic => "anthropic",
                crate::Provider::Llamacpp => "llamacpp",
            };
            let mut out = format!("provider: {provider}\nllama_url: {}\n", app.llama_url);
            let managed_path = app
                .managed_llama_server
                .borrow()
                .as_ref()
                .map(|s| s.model_path.display().to_string());
            if let Some(path) = managed_path {
                out.push_str(&format!("managed model: {path}\n"));
            } else if matches!(app.provider, crate::Provider::Llamacpp) {
                out.push_str("no managed llama-server is running.\n");
                out.push_str("\n");
                out.push_str("To start one inline:\n");
                out.push_str("  /provider llamacpp <PATH-TO-GGUF>\n");
                out.push_str("\n");
                out.push_str("e.g. /provider llamacpp ~/gguf/qwen3-7b.Q5_K_M.gguf\n");
                out.push_str("(model load takes 10-30s; child process dies when Mochi exits)\n");
            }
            info(app, out);
        }
        "llamacpp" => {
            let Some(&path_str) = args.get(1) else {
                info(app, "usage: /provider llamacpp <PATH-TO-GGUF>");
                return true;
            };
            if !matches!(app.provider, crate::Provider::Llamacpp) {
                warn(
                    app,
                    "current provider is not llamacpp — restart with `mochi --provider llamacpp` first",
                );
                return true;
            }
            let model_path = std::path::PathBuf::from(path_str);
            if !model_path.is_file() {
                warn(app, format!("model not found at {}", model_path.display()));
                return true;
            }
            spawn_managed_swap(app, model_path);
        }
        "anthropic" => {
            warn(
                app,
                "switching to anthropic mid-session is not supported — restart with `mochi --provider anthropic`",
            );
        }
        _ => {
            info(app, "provider: subcommands are show | llamacpp <PATH> | anthropic");
        }
    }
    true
}

fn spawn_managed_swap(app: &mut App, model_path: std::path::PathBuf) {
    info(
        app,
        format!(
            "switching to llamacpp model {} ... (model load may take 10-30s)",
            model_path.display()
        ),
    );

    let managed_slot = std::rc::Rc::clone(&app.managed_llama_server);
    let event_tx = app.event_tx.clone();
    let rt_tx = app.llama_runtime_tx.clone();
    let port = app
        .managed_llama_server
        .borrow()
        .as_ref()
        .map_or(8765_u16, |s| extract_port(&s.url).unwrap_or(8765));

    tokio::task::spawn_local(async move {
        // Drop the old server FIRST so the port is freed before the new bind.
        // Take ownership BEFORE awaiting so the RefCell borrow doesn't span the await
        // point (clippy::await_holding_refcell_ref + real correctness hazard).
        let old_server = managed_slot.borrow_mut().take();
        if let Some(old) = old_server {
            let _ = old.shutdown().await;
        }

        let mut cfg = crate::llama_server::LlamaServerConfig::new(model_path.clone());
        cfg.port = port;

        let server = match crate::llama_server::ManagedLlamaServer::spawn(&cfg) {
            Ok(s) => s,
            Err(e) => {
                let _ = event_tx.send(crate::agent::events::ClientEvent::SlashCommandError(
                    format!("provider swap: spawn failed: {e}"),
                ));
                return;
            }
        };

        if let Err(e) = server.wait_for_ready(Some(std::time::Duration::from_secs(120))).await {
            let _ = event_tx.send(crate::agent::events::ClientEvent::SlashCommandError(format!(
                "provider swap: server not ready: {e}"
            )));
            return;
        }

        let new_url = server.url.clone();
        *managed_slot.borrow_mut() = Some(server);

        if let Some(tx) = rt_tx.as_ref() {
            let _ =
                tx.send(crate::app::connect::llama_lifecycle::LlamaRuntimeCommand::SetLlamaUrl(
                    new_url.clone(),
                ));
            let _ =
                tx.send(crate::app::connect::llama_lifecycle::LlamaRuntimeCommand::RebuildSystem);
        }

        tracing::info!(
            target: crate::logging::targets::APP_LIFECYCLE,
            event_name = "provider_swap_completed",
            url = %new_url,
            model = %model_path.display(),
            "provider swap completed",
        );
    });
}

fn extract_port(url: &str) -> Option<u16> {
    let after_scheme = url.split("://").nth(1)?;
    let host_port = after_scheme.split('/').next()?;
    host_port.rsplit(':').next()?.parse().ok()
}

fn handle_clear_submit(app: &mut App) -> bool {
    app.clear_messages_tracked();
    app.viewport.engage_auto_scroll();
    info(app, "chat cleared. Memory and active skill are preserved.");
    true
}

fn handle_pet_submit(app: &mut App, args: &[&str]) -> bool {
    use crate::pet::{PetCharacter, PetMood, sprite_for};
    let sub = args.first().copied().unwrap_or("list");
    match sub {
        "list" => {
            let mut out = String::from("pets:\n");
            for ch in PetCharacter::all() {
                out.push_str(&format!("  - {}\n", ch.name()));
            }
            info(app, out);
        }
        "show" => {
            let name = args.get(1).copied().unwrap_or("mochi");
            let Some(ch) = PetCharacter::parse(name) else {
                info(app, format!("unknown pet: {name}"));
                return true;
            };
            let mut out = format!("{} ({}):\n", ch.name(), "happy");
            out.push_str(sprite_for(ch, PetMood::Happy));
            info(app, out);
        }
        _ => {
            info(app, "pet: subcommands are list | show NAME");
        }
    }
    true
}

fn opus_model_id_for_version(version: &str) -> Option<&'static str> {
    match version {
        "4.5" => Some(OPUS_4_5_MODEL_ID),
        "4.6" => Some(OPUS_4_6_MODEL_ID),
        "4.7" => Some(OPUS_4_7_MODEL_ID),
        _ => None,
    }
}

fn opus_version_label_for_model_id(model_id: &str) -> Option<&'static str> {
    match model_id {
        OPUS_4_5_MODEL_ID => Some("4.5"),
        OPUS_4_6_MODEL_ID => Some("4.6"),
        OPUS_4_7_MODEL_ID => Some("4.7"),
        _ => None,
    }
}

fn handle_opus_version_submit(app: &mut App, args: &[&str]) -> bool {
    let [subcommand] = args else {
        push_system_message(app, OPUS_VERSION_USAGE);
        return true;
    };
    let subcommand = subcommand.trim();
    if subcommand.is_empty() || args.len() != 1 {
        push_system_message(app, OPUS_VERSION_USAGE);
        return true;
    }

    match subcommand {
        "status" => {
            match current_opus_version_pin(app) {
                Ok(Some(model_id)) => {
                    let message = if let Some(version) = opus_version_label_for_model_id(&model_id)
                    {
                        format!(
                            "Opus is pinned to {version} in this folder via `.claude/settings.local.json`."
                        )
                    } else {
                        format!(
                            "Opus is pinned to {model_id} in this folder via `.claude/settings.local.json`."
                        )
                    };
                    push_system_message_with_severity(app, Some(SystemSeverity::Info), &message);
                }
                Ok(None) => push_system_message_with_severity(
                    app,
                    Some(SystemSeverity::Info),
                    "Opus is using the default alias resolution in this folder.",
                ),
                Err(err) => {
                    push_system_message(app, format!("Failed to read /opus-version status: {err}"));
                }
            }
            true
        }
        "default" => {
            if let Err(err) = set_opus_version_pin(app, None) {
                push_system_message(app, format!("Failed to run /opus-version default: {err}"));
            }
            true
        }
        _ => {
            let Some(model_id) = opus_model_id_for_version(subcommand) else {
                push_system_message(app, OPUS_VERSION_USAGE);
                return true;
            };
            if let Err(err) = set_opus_version_pin(app, Some(model_id)) {
                push_system_message(
                    app,
                    format!("Failed to run /opus-version {subcommand}: {err}"),
                );
            }
            true
        }
    }
}

fn handle_1m_context_submit(app: &mut App, args: &[&str]) -> bool {
    let [subcommand] = args else {
        push_system_message(app, ONE_M_CONTEXT_USAGE);
        return true;
    };
    let subcommand = subcommand.trim();
    if subcommand.is_empty() || args.len() != 1 {
        push_system_message(app, ONE_M_CONTEXT_USAGE);
        return true;
    }

    match subcommand {
        "status" => {
            match current_1m_context_disabled(app) {
                Ok(true) => push_system_message_with_severity(
                    app,
                    Some(SystemSeverity::Info),
                    "1M context is disabled for future sessions in this folder via `.claude/settings.local.json`.",
                ),
                Ok(false) => push_system_message_with_severity(
                    app,
                    Some(SystemSeverity::Info),
                    "1M context is enabled for future sessions in this folder.",
                ),
                Err(err) => {
                    push_system_message(app, format!("Failed to read /1m-context status: {err}"));
                }
            }
            true
        }
        "disable" => {
            if let Err(err) = set_1m_context_disabled(app, true) {
                push_system_message(app, format!("Failed to run /1m-context disable: {err}"));
            }
            true
        }
        "enable" => {
            if let Err(err) = set_1m_context_disabled(app, false) {
                push_system_message(app, format!("Failed to run /1m-context enable: {err}"));
            }
            true
        }
        _ => {
            push_system_message(app, ONE_M_CONTEXT_USAGE);
            true
        }
    }
}

fn current_1m_context_disabled(app: &mut App) -> Result<bool, String> {
    config::initialize_shared_state(app)?;
    store::disable_1m_context(&app.config.committed_local_settings_document).map_err(|()| {
        "Expected `.claude/settings.local.json` env.CLAUDE_CODE_DISABLE_1M_CONTEXT to be a string"
            .to_owned()
    })
}

fn set_1m_context_disabled(app: &mut App, disabled: bool) -> Result<(), String> {
    if !app.is_project_trusted() {
        return Err(
            "Project trust must be accepted before editing folder-local 1M context settings"
                .to_owned(),
        );
    }

    config::initialize_shared_state(app)?;
    let Some(path) = app.config.path_for(SettingFile::LocalSettings).cloned() else {
        return Err("Local settings path is not available".to_owned());
    };

    let current = store::disable_1m_context(&app.config.committed_local_settings_document)
        .map_err(|()| {
            "Expected `.claude/settings.local.json` env.CLAUDE_CODE_DISABLE_1M_CONTEXT to be a string"
                .to_owned()
        })?;

    let mut next_document = app.config.committed_local_settings_document.clone();
    store::set_disable_1m_context(&mut next_document, disabled);
    store::save(&path, &next_document)?;
    app.config.committed_local_settings_document = next_document;
    app.reconcile_runtime_from_persisted_settings_change();
    app.config.last_error = None;

    let message = match (disabled, current == disabled) {
        (true, true) => {
            "1M context is already disabled for future sessions in this folder. Run /new-session to apply it."
        }
        (true, false) => {
            "Disabled 1M context for future sessions in this folder. Run /new-session to apply it."
        }
        (false, true) => {
            "1M context is already enabled for future sessions in this folder. Run /new-session to apply it."
        }
        (false, false) => {
            "Enabled 1M context for future sessions in this folder. Run /new-session to apply it."
        }
    };
    push_system_message_with_severity(app, Some(SystemSeverity::Info), message);
    Ok(())
}

fn current_opus_version_pin(app: &mut App) -> Result<Option<String>, String> {
    config::initialize_shared_state(app)?;
    store::opus_version_pin(&app.config.committed_local_settings_document).map_err(|()| {
        "Expected `.claude/settings.local.json` env.ANTHROPIC_DEFAULT_OPUS_MODEL to be a string"
            .to_owned()
    })
}

fn set_opus_version_pin(app: &mut App, model: Option<&str>) -> Result<(), String> {
    if !app.is_project_trusted() {
        return Err(
            "Project trust must be accepted before editing folder-local Opus version settings"
                .to_owned(),
        );
    }

    config::initialize_shared_state(app)?;
    let Some(path) = app.config.path_for(SettingFile::LocalSettings).cloned() else {
        return Err("Local settings path is not available".to_owned());
    };

    let current =
        store::opus_version_pin(&app.config.committed_local_settings_document).map_err(|()| {
            "Expected `.claude/settings.local.json` env.ANTHROPIC_DEFAULT_OPUS_MODEL to be a string"
                .to_owned()
        })?;

    let mut next_document = app.config.committed_local_settings_document.clone();
    store::set_opus_version_pin(&mut next_document, model);
    store::save(&path, &next_document)?;
    app.config.committed_local_settings_document = next_document;
    app.reconcile_runtime_from_persisted_settings_change();
    app.config.last_error = None;

    let message = match (model, current.as_deref()) {
        (Some(next_model), Some(current_model)) if current_model == next_model => {
            let version = opus_version_label_for_model_id(next_model).unwrap_or(next_model);
            format!(
                "Opus is already pinned to {version} for future sessions in this folder. Run /new-session to apply it."
            )
        }
        (Some(next_model), _) => {
            let version = opus_version_label_for_model_id(next_model).unwrap_or(next_model);
            format!(
                "Pinned Opus to {version} for future sessions in this folder. Run /new-session to apply it."
            )
        }
        (None, None) => "Opus is already using the default alias in this folder.".to_owned(),
        (None, Some(_)) => {
            "Cleared the project-local Opus version pin for future sessions in this folder. Run /new-session to apply it.".to_owned()
        }
    };
    push_system_message_with_severity(app, Some(SystemSeverity::Info), &message);
    Ok(())
}

fn handle_cancel_submit(app: &mut App) -> bool {
    if !matches!(app.status, AppStatus::Thinking | AppStatus::Running) {
        return true;
    }
    if let Err(message) = crate::app::input_submit::request_cancel(app, CancelOrigin::Manual) {
        push_system_message(app, format!("Failed to run /cancel: {message}"));
    }
    true
}

fn handle_compact_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /compact");
        return true;
    }
    if require_active_session(
        app,
        "Cannot compact: not connected yet.",
        "Cannot compact: no active session.",
    )
    .is_none()
    {
        return true;
    }

    app.is_compacting = true;
    false
}

fn handle_config_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /config");
        return true;
    }

    if let Err(err) = crate::app::config::open(app) {
        push_system_message(app, format!("Failed to open settings: {err}"));
    }
    true
}

fn handle_docs_submit(app: &mut App, args: &[&str]) -> bool {
    let topic = match args {
        [topic] if !topic.trim().is_empty() => topic.trim(),
        _ => {
            push_system_message(app, docs_usage());
            return true;
        }
    };

    let body = match topic {
        "mode" => build_docs_mode_markdown(app),
        "models" => build_docs_models_markdown(app),
        "shortcuts" => build_docs_shortcuts_markdown(app),
        "commands" => build_docs_commands_markdown(app),
        "agents" => build_docs_agents_markdown(app),
        other => {
            push_system_message(app, format!("Unknown docs topic: {other}\n{}", docs_usage()));
            return true;
        }
    };

    push_system_message_with_severity(app, Some(SystemSeverity::Info), &body);
    true
}

fn handle_plugins_submit(app: &mut App, args: &[&str]) -> bool {
    let _ = args;

    if let Err(err) = crate::app::config::open(app) {
        push_system_message(app, format!("Failed to open plugins: {err}"));
        return true;
    }
    crate::app::config::activate_tab(app, crate::app::ConfigTab::Plugins);
    true
}

fn handle_mcp_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /mcp");
        return true;
    }

    if let Err(err) = crate::app::config::open(app) {
        push_system_message(app, format!("Failed to open MCP: {err}"));
        return true;
    }
    crate::app::config::activate_tab(app, crate::app::ConfigTab::Mcp);
    true
}

fn handle_status_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /status");
        return true;
    }

    if let Err(err) = crate::app::config::open(app) {
        push_system_message(app, format!("Failed to open status: {err}"));
        return true;
    }
    crate::app::config::activate_tab(app, crate::app::ConfigTab::Status);
    true
}

fn handle_usage_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /usage");
        return true;
    }

    if let Err(err) = crate::app::config::open(app) {
        push_system_message(app, format!("Failed to open usage: {err}"));
        return true;
    }
    crate::app::config::activate_tab(app, crate::app::ConfigTab::Usage);
    true
}

fn handle_login_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /login");
        return true;
    }

    push_user_message(app, "/login");
    tracing::debug!(
        target: crate::logging::targets::APP_AUTH,
        event_name = "login_command_requested",
        message = "login slash command requested",
        outcome = "start",
    );

    if crate::app::auth::has_credentials() {
        push_system_message_with_severity(
            app,
            Some(SystemSeverity::Info),
            "Already authenticated. Use /logout first to re-authenticate.",
        );
        return true;
    }

    let Some(claude_path) = resolve_claude_cli(app, "login") else {
        return true;
    };

    set_command_pending(app, "Authenticating...", None);

    let tx = app.event_tx.clone();
    let conn = app.conn.clone();
    tokio::task::spawn_local(async move {
        tracing::debug!(
            target: crate::logging::targets::APP_AUTH,
            event_name = "auth_terminal_suspended",
            message = "terminal suspended for login command",
            outcome = "start",
            auth_command = "login",
        );
        crate::app::suspend_terminal();

        let result = tokio::process::Command::new(&claude_path)
            .args(["auth", "login"])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .await;

        crate::app::resume_terminal();

        match result {
            Ok(status) => {
                tracing::debug!(
                    target: crate::logging::targets::APP_AUTH,
                    event_name = "auth_command_completed",
                    message = "login command completed",
                    outcome = if status.success() { "success" } else { "failure" },
                    auth_command = "login",
                    success = status.success(),
                    exit_code = ?status.code(),
                );
                if status.success() {
                    if !crate::app::auth::has_credentials() {
                        let _ = tx.send(ClientEvent::SlashCommandError(
                            "Login exited successfully but no credentials were saved. \
                             Try /login again or run `claude auth login` in another terminal."
                                .to_owned(),
                        ));
                        return;
                    }
                    if let Some(conn) = conn {
                        let _ = tx.send(ClientEvent::AuthCompleted { conn });
                    } else {
                        let _ = tx.send(ClientEvent::SlashCommandError(
                            "Login succeeded but no connection available to start a session."
                                .to_owned(),
                        ));
                    }
                } else {
                    let _ = tx.send(ClientEvent::SlashCommandError(format!(
                        "/login failed (exit code: {})",
                        status.code().map_or("unknown".to_owned(), |c| c.to_string())
                    )));
                }
            }
            Err(e) => {
                let _ = tx.send(ClientEvent::SlashCommandError(format!(
                    "Failed to run claude auth login: {e}"
                )));
            }
        }
    });
    true
}

fn handle_logout_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /logout");
        return true;
    }

    push_user_message(app, "/logout");
    tracing::debug!(
        target: crate::logging::targets::APP_AUTH,
        event_name = "logout_command_requested",
        message = "logout slash command requested",
        outcome = "start",
    );

    if !crate::app::auth::has_credentials() {
        push_system_message_with_severity(
            app,
            Some(SystemSeverity::Info),
            "Not currently authenticated. Nothing to log out from.",
        );
        return true;
    }

    let Some(claude_path) = resolve_claude_cli(app, "logout") else {
        return true;
    };

    set_command_pending(app, "Signing out...", None);

    let tx = app.event_tx.clone();
    tokio::task::spawn_local(async move {
        tracing::debug!(
            target: crate::logging::targets::APP_AUTH,
            event_name = "auth_terminal_suspended",
            message = "terminal suspended for logout command",
            outcome = "start",
            auth_command = "logout",
        );
        crate::app::suspend_terminal();

        let result = tokio::process::Command::new(&claude_path)
            .args(["auth", "logout"])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .await;

        crate::app::resume_terminal();

        match result {
            Ok(status) => {
                tracing::debug!(
                    target: crate::logging::targets::APP_AUTH,
                    event_name = "auth_command_completed",
                    message = "logout command completed",
                    outcome = if status.success() { "success" } else { "failure" },
                    auth_command = "logout",
                    success = status.success(),
                    exit_code = ?status.code(),
                );
                if status.success() {
                    if crate::app::auth::has_credentials() {
                        let _ = tx.send(ClientEvent::SlashCommandError(
                            "Logout exited successfully but credentials are still present. \
                             Try /logout again or run `claude auth logout` in another terminal."
                                .to_owned(),
                        ));
                        return;
                    }
                    let _ = tx.send(ClientEvent::LogoutCompleted);
                } else {
                    let _ = tx.send(ClientEvent::SlashCommandError(format!(
                        "/logout failed (exit code: {})",
                        status.code().map_or("unknown".to_owned(), |c| c.to_string())
                    )));
                }
            }
            Err(e) => {
                let _ = tx.send(ClientEvent::SlashCommandError(format!(
                    "Failed to run claude auth logout: {e}"
                )));
            }
        }
    });
    true
}

/// Resolve the `claude` CLI binary from PATH, or push an error message and return `None`.
fn resolve_claude_cli(app: &mut App, subcommand: &str) -> Option<std::path::PathBuf> {
    if let Ok(path) = which::which("claude") {
        tracing::debug!(
            target: crate::logging::targets::APP_AUTH,
            event_name = "auth_cli_resolved",
            message = "resolved claude CLI binary",
            outcome = "success",
            auth_command = subcommand,
            path = %path.display(),
        );
        Some(path)
    } else {
        push_system_message(
            app,
            format!(
                "claude CLI not found in PATH. Install it and retry /{subcommand}, \
                 or run `claude auth {subcommand}` manually in another terminal."
            ),
        );
        None
    }
}

fn handle_mode_submit(app: &mut App, args: &[&str]) -> bool {
    let [requested_mode_arg] = args else {
        push_system_message(app, "Usage: /mode <id>");
        return true;
    };
    let requested_mode = requested_mode_arg.trim();
    if requested_mode.is_empty() {
        push_system_message(app, "Usage: /mode <id>");
        return true;
    }

    let Some((conn, sid)) = require_active_session(
        app,
        "Cannot switch mode: not connected yet.",
        "Cannot switch mode: no active session.",
    ) else {
        return true;
    };

    if let Some(ref mode) = app.mode
        && !mode.available_modes.iter().any(|m| m.id == requested_mode)
    {
        push_system_message(app, format!("Unknown mode: {requested_mode}"));
        return true;
    }

    set_command_pending(app, "Switching mode...", Some(crate::app::PendingCommandAck::CurrentMode));

    let tx = app.event_tx.clone();
    let requested_mode_owned = requested_mode.to_owned();
    tokio::task::spawn_local(async move {
        match conn.set_mode(sid.to_string(), requested_mode_owned) {
            Ok(()) => {}
            Err(e) => {
                let _ =
                    tx.send(ClientEvent::SlashCommandError(format!("Failed to run /mode: {e}")));
            }
        }
    });
    true
}

fn handle_model_submit(app: &mut App, args: &[&str]) -> bool {
    let [model_name_arg] = args else {
        push_system_message(app, "Usage: /model <id>");
        return true;
    };
    let model_name = model_name_arg.trim();
    if model_name.is_empty() {
        push_system_message(app, "Usage: /model <id>");
        return true;
    }

    let Some((conn, sid)) = require_active_session(
        app,
        "Cannot switch model: not connected yet.",
        "Cannot switch model: no active session.",
    ) else {
        return true;
    };

    if !app.available_models.is_empty()
        && !app.available_models.iter().any(|candidate| candidate.id == model_name)
    {
        push_system_message(app, format!("Unknown model: {model_name}"));
        return true;
    }

    set_command_pending(
        app,
        "Switching model...",
        Some(crate::app::PendingCommandAck::CurrentModel),
    );

    let tx = app.event_tx.clone();
    let model_name = model_name.to_owned();
    tokio::task::spawn_local(async move {
        match conn.set_model(sid.to_string(), model_name) {
            Ok(()) => {}
            Err(e) => {
                let _ =
                    tx.send(ClientEvent::SlashCommandError(format!("Failed to run /model: {e}")));
            }
        }
    });
    true
}

fn handle_new_session_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /new-session");
        return true;
    }

    push_user_message(app, "/new-session");

    let Some(conn) = require_connection(app, "Cannot create new session: not connected yet.")
    else {
        return true;
    };

    set_command_pending(app, "Starting new session...", None);

    if let Err(e) = start_new_session(app, &conn, SessionStartReason::NewSession) {
        let _ = app
            .event_tx
            .send(ClientEvent::SlashCommandError(format!("Failed to run /new-session: {e}")));
    }
    true
}

fn handle_resume_submit(app: &mut App, args: &[&str]) -> bool {
    let [session_id_arg] = args else {
        push_system_message(app, "Usage: /resume <session_id>");
        return true;
    };
    let session_id = session_id_arg.trim();
    if session_id.is_empty() {
        push_system_message(app, "Usage: /resume <session_id>");
        return true;
    }

    push_user_message(app, format!("/resume {session_id}"));
    let Some(conn) = require_connection(app, "Cannot resume session: not connected yet.") else {
        return true;
    };

    set_command_pending(app, &format!("Resuming session {session_id}..."), None);
    let session_id = session_id.to_owned();
    if let Err(e) = begin_resume_session(app, &conn, session_id) {
        let _ = app
            .event_tx
            .send(ClientEvent::SlashCommandError(format!("Failed to run /resume: {e}")));
    }
    true
}

fn handle_unknown_submit(app: &mut App, command_name: &str) -> bool {
    if super::candidates::is_supported_command(app, command_name) {
        return false;
    }
    push_system_message(app, format!("{command_name} is not yet supported"));
    true
}

fn docs_usage() -> &'static str {
    "Usage: /docs <mode|models|shortcuts|commands|agents>"
}

fn build_docs_mode_markdown(app: &App) -> String {
    let rows = app.mode.as_ref().map_or_else(
        || vec![("Unavailable".to_owned(), "Connect to load the current session mode.".to_owned())],
        |mode| {
            let mut rows: Vec<(String, String)> = mode
                .available_modes
                .iter()
                .map(|entry| {
                    let mut details = format!("ID `{}`", entry.id);
                    if entry.id == mode.current_mode_id {
                        details.push_str("; current");
                    }
                    (entry.name.clone(), details)
                })
                .collect();
            if rows.is_empty() {
                rows.push((
                    mode.current_mode_name.clone(),
                    format!("ID `{}`; current", mode.current_mode_id),
                ));
            }
            rows
        },
    );

    render_docs_table(
        "Docs: Mode",
        "Current and available session modes.",
        ("Mode", "Details"),
        rows,
    )
}

fn build_docs_models_markdown(app: &App) -> String {
    let rows = if app.available_models.is_empty() {
        vec![("Unavailable".to_owned(), "Connect to load advertised models.".to_owned())]
    } else {
        app.available_models
            .iter()
            .map(|model| {
                let name = if model.display_name.trim().is_empty() {
                    model.id.clone()
                } else {
                    model.display_name.clone()
                };
                (name, model_details(model))
            })
            .collect()
    };

    render_docs_table(
        "Docs: Models",
        "Advertised models and capabilities for the current session.",
        ("Model", "Details"),
        rows,
    )
}

fn build_docs_shortcuts_markdown(app: &App) -> String {
    render_docs_table(
        "Docs: Shortcuts",
        "Live keyboard shortcuts for the current app state.",
        ("Shortcut", "Action"),
        crate::ui::help::key_help_items(app),
    )
}

fn build_docs_commands_markdown(app: &App) -> String {
    render_docs_table(
        "Docs: Commands",
        "App-owned and advertised slash commands.",
        ("Command", "Description"),
        crate::ui::help::docs_command_items(app),
    )
}

fn build_docs_agents_markdown(app: &App) -> String {
    render_docs_table(
        "Docs: Agents",
        "Advertised subagents for the current session.",
        ("Agent", "Description"),
        crate::ui::help::subagent_help_items(app),
    )
}

fn model_details(model: &crate::agent::model::AvailableModel) -> String {
    let mut parts = Vec::new();
    parts.push(format!("ID `{}`", model.id));
    if let Some(description) = model.description.as_deref()
        && !description.trim().is_empty()
    {
        parts.push(description.trim().to_owned());
    }
    if model.supports_effort {
        parts.push("Effort".to_owned());
    }
    if model.supports_adaptive_thinking == Some(true) {
        parts.push("Adaptive thinking".to_owned());
    }
    if model.supports_fast_mode == Some(true) {
        parts.push("Fast mode".to_owned());
    }
    if model.supports_auto_mode == Some(true) {
        parts.push("Auto mode".to_owned());
    }
    parts.join("; ")
}

fn render_docs_table(
    title: &str,
    intro: &str,
    headers: (&str, &str),
    rows: Vec<(String, String)>,
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(&mut markdown, "# {title}");
    let _ = writeln!(&mut markdown);
    let _ = writeln!(&mut markdown, "{intro}");
    let _ = writeln!(&mut markdown);
    let _ = writeln!(&mut markdown, "| {} | {} |", headers.0, headers.1);
    let _ = writeln!(&mut markdown, "| --- | --- |");
    for (left, right) in rows {
        let _ = writeln!(
            &mut markdown,
            "| {} | {} |",
            markdown_table_cell(&left),
            markdown_table_cell(&right),
        );
    }
    markdown
}

fn markdown_table_cell(value: &str) -> String {
    value.trim().replace('|', "\\|").replace('\r', "").replace('\n', " - ")
}

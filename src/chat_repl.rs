use crate::agent::llama_client::{ChatEvent, LlamaConfig, Message, stream_chat};
use crate::memory::{self, FactKind, MemoryStore, render_memory_section};
use crate::pet::{PetMood, sprite};
use crate::skills::{self, Skill};
use futures::StreamExt as _;
use std::collections::BTreeMap;
use std::io::{BufRead as _, Write as _};

fn print_pet(mood: PetMood, caption: &str) {
    let pet = sprite(mood);
    let lines: Vec<&str> = pet.lines().collect();
    let pad = " ".repeat(2);
    for (i, line) in lines.iter().enumerate() {
        if i == 1 {
            println!("{pad}{line}   {caption}");
        } else {
            println!("{pad}{line}");
        }
    }
}

fn compose_system(base: &str, active: Option<&Skill>, memory: Option<&MemoryStore>) -> String {
    let mut out = String::new();
    if let Some(store) = memory {
        let facts = store.list(None).unwrap_or_default();
        let section = render_memory_section(&facts, active.map(|s| s.name.as_str()));
        if !section.is_empty() {
            out.push_str(&section);
        }
    }
    out.push_str(base);
    if let Some(skill) = active {
        out.push_str(&format!("\n\n[Active skill: {}]\n{}", skill.name, skill.body));
    }
    out
}

enum SlashOutcome {
    Handled,
    Quit,
    Passthrough,
}

struct ReplState<'a> {
    skills: &'a BTreeMap<String, Skill>,
    memory: Option<&'a MemoryStore>,
    active_skill: &'a mut Option<String>,
    history: &'a mut Vec<Message>,
    base_system: &'a str,
}

fn rebuild_system(state: &mut ReplState<'_>) {
    let active = state.active_skill.as_deref().and_then(|n| state.skills.get(n));
    let new_system = compose_system(state.base_system, active, state.memory);
    if let Some(first) = state.history.first_mut() {
        first.content = new_system;
    }
}

fn handle_slash(input: &str, state: &mut ReplState<'_>) -> SlashOutcome {
    let parts: Vec<&str> = input.split_whitespace().collect();
    let cmd = parts.first().copied().unwrap_or("");
    match cmd {
        "/quit" | "/exit" => SlashOutcome::Quit,
        "/help" => {
            println!("  /quit                                  exit");
            println!(
                "  /reset                                 clear conversation (keep system+memory)"
            );
            println!("  /skill list|use NAME|off|show NAME     skill management");
            println!(
                "  /memory list [KIND]                    list facts (profile|concept|state|behavioral)"
            );
            println!("  /memory remember KIND SLUG CONTENT     store a fact");
            println!("  /memory forget SLUG                    delete a fact");
            println!("  /memory profile CONTENT                set the user profile blurb");
            println!();
            SlashOutcome::Handled
        }
        "/reset" => {
            rebuild_system(state);
            state.history.truncate(1);
            println!("  conversation cleared.\n");
            SlashOutcome::Handled
        }
        "/skill" => handle_skill(parts.as_slice(), state),
        "/memory" => handle_memory(input, parts.as_slice(), state),
        _ if cmd.starts_with('/') => {
            println!("  unknown command `{cmd}`. /help for list.\n");
            SlashOutcome::Handled
        }
        _ => SlashOutcome::Passthrough,
    }
}

fn handle_skill(parts: &[&str], state: &mut ReplState<'_>) -> SlashOutcome {
    let sub = parts.get(1).copied().unwrap_or("");
    match sub {
        "list" | "" => {
            if state.skills.is_empty() {
                println!("  no skills installed.");
                println!("  put SKILL.md files under ~/.mochi/skills/<name>/SKILL.md");
            } else {
                for (name, skill) in state.skills {
                    let mark = if state.active_skill.as_deref() == Some(name.as_str()) {
                        "*"
                    } else {
                        " "
                    };
                    let desc = if skill.description.is_empty() {
                        "(no description)"
                    } else {
                        skill.description.as_str()
                    };
                    println!("  {mark} {name:<20} {desc}");
                }
            }
            println!();
        }
        "use" => {
            let name = parts.get(2).copied().unwrap_or("");
            if name.is_empty() {
                println!("  usage: /skill use NAME\n");
            } else if state.skills.contains_key(name) {
                *state.active_skill = Some(name.to_owned());
                rebuild_system(state);
                println!("  active skill: {name}\n");
            } else {
                println!("  unknown skill: {name}\n");
            }
        }
        "off" => {
            *state.active_skill = None;
            rebuild_system(state);
            println!("  skill deactivated.\n");
        }
        "show" => {
            let name = parts.get(2).copied().unwrap_or("");
            if let Some(skill) = state.skills.get(name) {
                println!("  --- {name} ---");
                println!("{}", skill.body);
                println!("  --- end ---\n");
            } else {
                println!("  unknown skill: {name}\n");
            }
        }
        _ => {
            println!("  /skill: unknown subcommand `{sub}`. Try: list, use, off, show\n");
        }
    }
    SlashOutcome::Handled
}

fn handle_memory(raw: &str, parts: &[&str], state: &mut ReplState<'_>) -> SlashOutcome {
    let Some(store) = state.memory else {
        println!("  memory unavailable (failed to open DB at startup).\n");
        return SlashOutcome::Handled;
    };
    let sub = parts.get(1).copied().unwrap_or("");
    match sub {
        "list" | "" => {
            let kind = parts.get(2).and_then(|s| FactKind::parse(s));
            let facts = store.list(kind).unwrap_or_default();
            if facts.is_empty() {
                println!("  no facts stored.\n");
            } else {
                for f in &facts {
                    let scope = f.skill_scope.as_deref().unwrap_or("-");
                    println!(
                        "  [{:<10}] {:<16} scope={:<10}  {}",
                        f.kind.as_str(),
                        f.slug,
                        scope,
                        f.content
                    );
                }
                println!();
            }
        }
        "remember" => {
            let kind_str = parts.get(2).copied().unwrap_or("");
            let slug = parts.get(3).copied().unwrap_or("");
            let Some(kind) = FactKind::parse(kind_str) else {
                println!(
                    "  usage: /memory remember KIND SLUG CONTENT  (KIND: profile|concept|state|behavioral)\n"
                );
                return SlashOutcome::Handled;
            };
            if slug.is_empty() {
                println!("  usage: /memory remember KIND SLUG CONTENT\n");
                return SlashOutcome::Handled;
            }
            let content_start = raw.splitn(5, char::is_whitespace).nth(4).unwrap_or("").trim();
            if content_start.is_empty() {
                println!("  usage: /memory remember KIND SLUG CONTENT\n");
                return SlashOutcome::Handled;
            }
            let scope_owned: Option<String> = if kind == FactKind::Behavioral {
                Some(state.active_skill.as_deref().unwrap_or("default").to_owned())
            } else {
                None
            };
            match store.upsert(kind, slug, content_start, scope_owned.as_deref()) {
                Ok(_) => {
                    rebuild_system(state);
                    let scope_label = scope_owned.as_deref().unwrap_or("-");
                    println!("  remembered [{}] {slug} (scope={scope_label})\n", kind.as_str());
                }
                Err(err) => println!("  error: {err}\n"),
            }
        }
        "forget" => {
            let slug = parts.get(2).copied().unwrap_or("");
            if slug.is_empty() {
                println!("  usage: /memory forget SLUG\n");
                return SlashOutcome::Handled;
            }
            match store.forget(slug) {
                Ok(0) => println!("  no fact with slug `{slug}`.\n"),
                Ok(n) => {
                    rebuild_system(state);
                    println!("  forgot {n} fact(s) named `{slug}`.\n");
                }
                Err(err) => println!("  error: {err}\n"),
            }
        }
        "profile" => {
            let content = raw.splitn(3, char::is_whitespace).nth(2).unwrap_or("").trim();
            if content.is_empty() {
                match store.profile() {
                    Ok(Some(p)) => println!("  profile:\n{p}\n"),
                    Ok(None) => println!("  no profile set. usage: /memory profile <text>\n"),
                    Err(err) => println!("  error: {err}\n"),
                }
            } else {
                match store.upsert(FactKind::Profile, "user", content, None) {
                    Ok(_) => {
                        rebuild_system(state);
                        println!("  profile updated.\n");
                    }
                    Err(err) => println!("  error: {err}\n"),
                }
            }
        }
        _ => {
            println!(
                "  /memory: unknown subcommand `{sub}`. Try: list, remember, forget, profile\n"
            );
        }
    }
    SlashOutcome::Handled
}

pub async fn run(url: String, system: String, temperature: f32) -> anyhow::Result<()> {
    let config =
        LlamaConfig { url, model: None, temperature: Some(temperature), max_tokens: Some(512) };

    let skills_dir = skills::default_skills_dir();
    let skills_map = match skills_dir.as_deref() {
        Some(path) => skills::load_all(path).unwrap_or_default(),
        None => BTreeMap::new(),
    };

    let memory_store = memory::default_db_path().and_then(|p| match MemoryStore::open(&p) {
        Ok(s) => Some(s),
        Err(err) => {
            eprintln!("warn: failed to open memory db: {err}");
            None
        }
    });

    println!();
    print_pet(PetMood::Idle, &format!("mochi @ {}", config.url));
    println!();
    eprintln!("type a message and hit Enter. /help for commands, /quit to exit.");
    eprintln!(
        "{} skills loaded, memory {}.\n",
        skills_map.len(),
        if memory_store.is_some() { "on" } else { "off" }
    );

    let mut active_skill: Option<String> = None;
    let initial_system = compose_system(&system, None, memory_store.as_ref());
    let mut history: Vec<Message> = vec![Message::system(initial_system)];
    let stdin = std::io::stdin();
    let mut stdin_lock = stdin.lock();
    let mut buf = String::new();

    loop {
        print!("you: ");
        std::io::stdout().flush().ok();
        buf.clear();
        let n = stdin_lock.read_line(&mut buf)?;
        if n == 0 {
            break;
        }
        let user_text = buf.trim();
        if user_text.is_empty() {
            continue;
        }

        if user_text.starts_with('/') {
            let mut state = ReplState {
                skills: &skills_map,
                memory: memory_store.as_ref(),
                active_skill: &mut active_skill,
                history: &mut history,
                base_system: &system,
            };
            match handle_slash(user_text, &mut state) {
                SlashOutcome::Quit => break,
                SlashOutcome::Handled => continue,
                SlashOutcome::Passthrough => {}
            }
        }

        history.push(Message::user(user_text));

        print_pet(PetMood::Thinking, "");

        print!("mochi: ");
        std::io::stdout().flush().ok();
        let mut assistant = String::new();
        let mut stream_failed: Option<anyhow::Error> = None;
        {
            let mut stream = Box::pin(stream_chat(&config, &history).await?);
            while let Some(item) = stream.next().await {
                match item {
                    Ok(ChatEvent::Delta(text)) => {
                        print!("{text}");
                        std::io::stdout().flush().ok();
                        assistant.push_str(&text);
                    }
                    Ok(ChatEvent::ToolCall(_)) => {
                        // REPL mode does not advertise tools — ignore stray calls.
                    }
                    Ok(ChatEvent::Done) => break,
                    Err(err) => {
                        stream_failed = Some(err);
                        break;
                    }
                }
            }
        }
        println!();

        if let Some(err) = stream_failed {
            print_pet(PetMood::Sad, "stream error");
            eprintln!("error: {err}\n");
            history.pop();
            continue;
        }

        history.push(Message::assistant(assistant));
        print_pet(PetMood::Happy, "");
        println!();
    }

    print_pet(PetMood::Sleeping, "bye ~");
    Ok(())
}

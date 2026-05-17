//! BOOKMARKS-style dedup + consolidation for the memory layer.
//!
//! Two LLM-driven operations live here:
//!
//! - [`judge_capture`] — before persisting a freshly captured fact, ask the
//!   model whether it is a duplicate (`reuse`), a refinement of an existing
//!   fact (`derive`), or genuinely new (`new`). Keeps memory bloat under
//!   control without flat-fact mush.
//!
//! - [`consolidate_profile`] — ask the model to fold all stored facts into a
//!   single ~200-word narrative blurb written in third person about the user.
//!   Mirrors the BOOKMARKS paper's `profile_extract` / `profile_aggregate`
//!   pattern at session scope.
//!
//! Both rely on the same llama.cpp HTTP endpoint as conversation responses,
//! so they cost one extra LLM call each. Calls run on `tokio::task::spawn_local`
//! background so the user's chat stream is never blocked.

use crate::agent::llama_client::{ChatEvent, LlamaConfig, Message, stream_chat};
use crate::memory::Fact;
use crate::memory_capture::CapturedFact;
use futures::StreamExt as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JudgeOutcome {
    /// Same slot — overwrite the matched existing fact's content.
    Reuse { existing_id: i64 },
    /// Related but distinct — keep both, store new fact as a fresh slug.
    Derive { existing_id: i64 },
    /// Unrelated — store as a brand-new fact.
    New,
}

const JUDGE_TEMPERATURE: f32 = 0.0;
const JUDGE_MAX_TOKENS: u32 = 16;
const JUDGE_SYSTEM: &str = "Compare a newly captured user fact against an existing fact. Decide if they cover the SAME attribute of the user. \
Output EXACTLY ONE lowercase word: `reuse`, `derive`, or `new`.\n\n\
- `reuse`: both facts describe the SAME attribute even if slugs or wording differ. \
  The new fact REPLACES the old. Use whenever the two are paraphrases / synonyms / restatements of the same user attribute.\n\
- `derive`: same general theme but different facets. Keep BOTH (e.g. broader vs narrower scope, or related but distinct attributes within the same area).\n\
- `new`: unrelated attributes.\n\n\
Bias toward `reuse` when the two values would naturally update the same slot in a structured profile. \
Different slug strings alone are NEVER a reason to keep duplicates. \
Output one word.";

const CONSOLIDATE_TEMPERATURE: f32 = 0.2;
const CONSOLIDATE_MAX_TOKENS: u32 = 400;
const RESTATE_TEMPERATURE: f32 = 0.1;
const RESTATE_MAX_TOKENS: u32 = 200;
const OBSERVE_TEMPERATURE: f32 = 0.1;
const OBSERVE_MAX_TOKENS: u32 = 200;
const CONSOLIDATE_SYSTEM: &str = "You are a profile editor. Given a list of durable facts about a user, \
write a single ~200-word narrative paragraph in third person about the user. \
Mention name, role, preferences, location, current focus, communication style. \
Be factual — do NOT invent details not present in the input. \
If the input is sparse, write a short profile (one or two sentences). \
Output only the paragraph; no headings, no markdown, no commentary.";

const RESTATE_SYSTEM: &str = "You maintain ONE state fact about a user across an ongoing chat. \
Given the fact's current answer and a list of recent user messages, decide what the answer should be NOW. \
Apply transitions implied by later messages over earlier ones. \
If recent messages don't touch this state, return the current answer unchanged. \
If the user has clearly moved on / contradicted the old state, write the new state in a short clause. \
Output ONLY the updated answer text (one short sentence or phrase). \
No prose, no preamble, no quotes, no third-person framing.";

const OBSERVE_SYSTEM: &str = "You observe behavioral patterns in how a user communicates. \
Given a behavioral query and a list of recent user messages, summarize the pattern in ONE short clause (3-12 words) \
that could be stored as the user's behavioral preference. \
Examples of valid output: `prefers terse 1-3 sentence replies`, `often mixes Vietnamese and English`, `pushes back on hype phrasing`. \
If no clear pattern emerges from the messages, output exactly: `none`. \
Output ONLY the clause or `none`. No prose, no preamble.";

/// Ask the model to classify the relation between a newly captured fact and the
/// closest existing fact of the same kind. Returns `JudgeOutcome::New` if no
/// candidates exist or the LLM call fails — never blocks the capture pipeline.
pub async fn judge_capture(
    config: &LlamaConfig,
    candidate: &CapturedFact,
    existing_same_kind: &[&Fact],
) -> JudgeOutcome {
    let Some(best) = pick_best_candidate(candidate, existing_same_kind) else {
        return JudgeOutcome::New;
    };
    let judge_config = LlamaConfig {
        url: config.url.clone(),
        model: config.model.clone(),
        temperature: Some(JUDGE_TEMPERATURE),
        max_tokens: Some(JUDGE_MAX_TOKENS),
    };
    let user_prompt = format!(
        "Existing fact (id={}):\n  kind: {}\n  slug: {}\n  content: {}\n\n\
         New fact:\n  kind: {}\n  slug: {}\n  content: {}\n\n\
         Output one word: reuse, derive, or new.",
        best.id,
        best.kind.as_str(),
        best.slug,
        best.content,
        candidate.kind.as_str(),
        candidate.slug,
        candidate.content,
    );
    let messages = vec![Message::system(JUDGE_SYSTEM), Message::user(user_prompt)];
    let response = match collect_stream(&judge_config, &messages).await {
        Some(s) => s,
        None => return JudgeOutcome::New,
    };
    parse_judge_response(&response, best.id)
}

/// Restate a single state fact's answer from recent user messages. Returns the
/// new content or `None` if the LLM returns empty / failure. Caller is
/// expected to compare against the current answer to decide whether to write.
pub async fn restate_from_history(
    config: &LlamaConfig,
    slug: &str,
    current_answer: &str,
    recent_user_messages: &[String],
) -> Option<String> {
    if recent_user_messages.is_empty() {
        return None;
    }
    let cfg = LlamaConfig {
        url: config.url.clone(),
        model: config.model.clone(),
        temperature: Some(RESTATE_TEMPERATURE),
        max_tokens: Some(RESTATE_MAX_TOKENS),
    };
    let mut prompt = format!(
        "State slug: {slug}\nCurrent answer: {current_answer}\n\nRecent user messages (oldest → newest):\n"
    );
    for (i, msg) in recent_user_messages.iter().enumerate() {
        prompt.push_str(&format!("{}. {msg}\n", i + 1));
    }
    prompt.push_str("\nWrite the updated answer now.");
    let messages = vec![Message::system(RESTATE_SYSTEM), Message::user(prompt)];
    let raw = collect_stream(&cfg, &messages).await?;
    let cleaned = raw.trim().trim_matches('"').trim_matches('\'').trim().to_owned();
    if cleaned.is_empty() { None } else { Some(cleaned) }
}

/// Summarize a behavioral pattern from recent user messages. Returns the
/// pattern clause or `None` (model returned `none`, or empty/failure).
pub async fn observe_behavioral_pattern(
    config: &LlamaConfig,
    behavior_query: &str,
    recent_user_messages: &[String],
) -> Option<String> {
    if recent_user_messages.is_empty() {
        return None;
    }
    let cfg = LlamaConfig {
        url: config.url.clone(),
        model: config.model.clone(),
        temperature: Some(OBSERVE_TEMPERATURE),
        max_tokens: Some(OBSERVE_MAX_TOKENS),
    };
    let mut prompt = format!("Behavioral query: {behavior_query}\n\nRecent user messages:\n");
    for (i, msg) in recent_user_messages.iter().enumerate() {
        prompt.push_str(&format!("{}. {msg}\n", i + 1));
    }
    prompt.push_str("\nSummarize the pattern as a short clause, or output `none`.");
    let messages = vec![Message::system(OBSERVE_SYSTEM), Message::user(prompt)];
    let raw = collect_stream(&cfg, &messages).await?;
    let cleaned = raw.trim().trim_matches('"').trim_matches('\'').trim().to_owned();
    if cleaned.is_empty() || cleaned.to_lowercase() == "none" { None } else { Some(cleaned) }
}

/// Build a consolidated narrative profile from the current set of facts.
/// Returns the rewritten paragraph or `None` if the LLM call fails / produces
/// empty output. Caller is responsible for storing it as the `profile/user`
/// fact.
pub async fn consolidate_profile(config: &LlamaConfig, facts: &[Fact]) -> Option<String> {
    if facts.is_empty() {
        return None;
    }
    let consolidate_config = LlamaConfig {
        url: config.url.clone(),
        model: config.model.clone(),
        temperature: Some(CONSOLIDATE_TEMPERATURE),
        max_tokens: Some(CONSOLIDATE_MAX_TOKENS),
    };
    let mut input = String::from("Facts:\n");
    for f in facts {
        let scope = f.skill_scope.as_deref().unwrap_or("-");
        input.push_str(&format!(
            "- [{}] {} (scope={}): {}\n",
            f.kind.as_str(),
            f.slug,
            scope,
            f.content
        ));
    }
    input.push_str("\nWrite the ~200-word narrative profile now.");
    let messages = vec![Message::system(CONSOLIDATE_SYSTEM), Message::user(input)];
    let raw = collect_stream(&consolidate_config, &messages).await?;
    let cleaned = raw.trim().to_owned();
    if cleaned.is_empty() { None } else { Some(cleaned) }
}

pub fn parse_judge_response(raw: &str, existing_id: i64) -> JudgeOutcome {
    let word = raw
        .lines()
        .find_map(|line| line.split_whitespace().next())
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphabetic())
        .to_ascii_lowercase();
    match word.as_str() {
        "reuse" => JudgeOutcome::Reuse { existing_id },
        "derive" => JudgeOutcome::Derive { existing_id },
        _ => JudgeOutcome::New,
    }
}

/// Pick the most lexically similar candidate by token overlap on slug+content.
/// We keep this cheap because it runs on every capture; the LLM only sees the
/// top candidate.
fn pick_best_candidate<'a>(candidate: &CapturedFact, existing: &'a [&'a Fact]) -> Option<&'a Fact> {
    if existing.is_empty() {
        return None;
    }
    let cand_tokens = tokenize(&format!("{} {}", candidate.slug, candidate.content));
    if cand_tokens.is_empty() {
        return existing.first().copied();
    }
    existing
        .iter()
        .map(|f| (overlap(&cand_tokens, &tokenize(&format!("{} {}", f.slug, f.content))), *f))
        .max_by_key(|(score, _)| *score)
        .map(|(_, f)| f)
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .filter(|w| w.len() >= 2 && !is_stopword(w))
        .collect()
}

fn overlap(a: &[String], b: &[String]) -> usize {
    use std::collections::HashSet;
    let set_a: HashSet<&String> = a.iter().collect();
    b.iter().filter(|t| set_a.contains(t)).count()
}

fn is_stopword(w: &str) -> bool {
    matches!(
        w,
        "the"
            | "a"
            | "an"
            | "is"
            | "of"
            | "in"
            | "on"
            | "at"
            | "to"
            | "for"
            | "and"
            | "or"
            | "user"
            | "user's"
            | "users"
            | "with"
            | "by"
            | "from"
            | "as"
    )
}

async fn collect_stream(config: &LlamaConfig, messages: &[Message]) -> Option<String> {
    let mut stream = match stream_chat(config, messages).await {
        Ok(s) => Box::pin(s),
        Err(err) => {
            tracing::warn!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "memory_judge_request_failed",
                error = %err,
            );
            return None;
        }
    };
    let mut full = String::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(ChatEvent::Delta(text)) => full.push_str(&text),
            Ok(ChatEvent::ToolCall(_)) => {}
            Ok(ChatEvent::Done) => break,
            Err(_) => break,
        }
    }
    Some(full)
}

#[cfg(test)]
mod tests {
    use super::{JudgeOutcome, overlap, parse_judge_response, pick_best_candidate, tokenize};
    use crate::memory::{Fact, FactKind};
    use crate::memory_capture::CapturedFact;

    fn fact(id: i64, slug: &str, content: &str) -> Fact {
        Fact {
            id,
            kind: FactKind::Concept,
            slug: slug.to_owned(),
            content: content.to_owned(),
            skill_scope: None,
            created_at: 0,
            last_used: 0,
        }
    }

    #[test]
    fn parser_accepts_single_word_lowercase() {
        assert!(matches!(parse_judge_response("reuse", 7), JudgeOutcome::Reuse { existing_id: 7 }));
        assert!(matches!(
            parse_judge_response("derive", 7),
            JudgeOutcome::Derive { existing_id: 7 }
        ));
        assert!(matches!(parse_judge_response("new", 7), JudgeOutcome::New));
    }

    #[test]
    fn parser_strips_punctuation_and_capitalization() {
        assert!(matches!(
            parse_judge_response("Reuse.", 1),
            JudgeOutcome::Reuse { existing_id: 1 }
        ));
        assert!(matches!(parse_judge_response("**NEW**\n", 2), JudgeOutcome::New));
    }

    #[test]
    fn parser_defaults_to_new_on_garbage() {
        assert!(matches!(parse_judge_response("hello world", 1), JudgeOutcome::New));
        assert!(matches!(parse_judge_response("", 1), JudgeOutcome::New));
    }

    #[test]
    fn parser_takes_first_word_only() {
        // Some models output "reuse — same slot"
        assert!(matches!(
            parse_judge_response("reuse — same", 5),
            JudgeOutcome::Reuse { existing_id: 5 }
        ));
    }

    #[test]
    fn tokenize_drops_stopwords_and_short_tokens() {
        let toks = tokenize("The user lives in Saigon, Vietnam");
        assert!(toks.contains(&"lives".to_owned()));
        assert!(toks.contains(&"saigon".to_owned()));
        assert!(toks.contains(&"vietnam".to_owned()));
        assert!(!toks.contains(&"the".to_owned()));
        assert!(!toks.contains(&"in".to_owned()));
        assert!(!toks.contains(&"user".to_owned()));
    }

    #[test]
    fn overlap_counts_shared_tokens() {
        let a = tokenize("user lives in Saigon");
        let b = tokenize("user works in Saigon office");
        assert_eq!(overlap(&a, &b), 1); // only "saigon"
    }

    #[test]
    fn pick_best_returns_highest_overlap() {
        let candidate = CapturedFact {
            kind: FactKind::Concept,
            slug: "location".into(),
            content: "User lives in Saigon Vietnam".into(),
        };
        let a = fact(1, "city", "Tokyo");
        let b = fact(2, "location", "Saigon, Vietnam");
        let c = fact(3, "language", "Rust");
        let existing = vec![&a, &b, &c];
        let best = pick_best_candidate(&candidate, &existing).unwrap();
        assert_eq!(best.id, 2);
    }

    #[test]
    fn pick_best_returns_none_for_empty() {
        let candidate =
            CapturedFact { kind: FactKind::Concept, slug: "x".into(), content: "y".into() };
        let existing: Vec<&Fact> = vec![];
        assert!(pick_best_candidate(&candidate, &existing).is_none());
    }
}

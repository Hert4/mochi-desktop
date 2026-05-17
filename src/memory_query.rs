//! BOOKMARKS-style active query proposal + fact matching.
//!
//! Two responsibilities:
//!
//! 1. [`propose_queries`] — ask the model what it needs to look up before
//!    responding to the current scene. Returns up to 3 typed search queries
//!    (`concept` / `state` / `behavioral`). Cheap structured prompt, output
//!    parsed as `TAG|QUERY` lines.
//!
//! 2. [`match_facts`] — translate proposed queries into a list of stored
//!    facts. Token-overlap match against slug + content. Behavioral queries
//!    can be scoped to the active skill.
//!
//! Glued together in [`fetch_relevant_facts`] for callers that want one
//! function: scene + active_skill → relevant facts.

use crate::agent::llama_client::{ChatEvent, LlamaConfig, Message, stream_chat};
use crate::memory::{Fact, FactKind, MemoryStore};
use futures::StreamExt as _;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryQuery {
    pub tag: FactKind,
    pub query: String,
}

const PROPOSE_TEMPERATURE: f32 = 0.0;
const PROPOSE_MAX_TOKENS: u32 = 200;
const MAX_QUERIES: usize = 3;
const MATCH_LIMIT_PER_QUERY: usize = 3;

const PROPOSE_SYSTEM: &str = "You are a memory query planner. Given a chat scene, propose up to 3 short search queries the assistant should run against its long-term memory BEFORE replying. Each query must specify which memory bucket to search.\n\n\
Buckets:\n\
- profile: user identity (name, role)\n\
- concept: named entities the user has mentioned (city, language, project)\n\
- state: the user's current task or focus\n\
- behavioral: user preferences or communication patterns\n\n\
Output ONE query per line in the exact format `TAG|QUERY`. Keep queries short and lexical (3-8 words, no quotes). \
Do NOT speculate; only propose lookups that are clearly needed to ground the reply. If nothing is needed, output NOTHING.\n\n\
GOOD examples:\n\
profile|user name\n\
concept|user location city\n\
behavioral|reply length preference\n\n\
BAD (do NOT output):\n\
- prose, JSON, markdown\n\
- queries longer than 8 words\n\
- speculative or repeated queries";

pub async fn propose_queries(base_config: &LlamaConfig, scene: &str) -> Vec<MemoryQuery> {
    if scene.trim().is_empty() {
        return Vec::new();
    }
    let config = LlamaConfig {
        url: base_config.url.clone(),
        model: base_config.model.clone(),
        temperature: Some(PROPOSE_TEMPERATURE),
        max_tokens: Some(PROPOSE_MAX_TOKENS),
    };
    let messages = vec![Message::system(PROPOSE_SYSTEM), Message::user(scene.to_owned())];
    let raw = collect_text(&config, &messages).await;
    parse_propose_response(&raw)
}

pub fn parse_propose_response(text: &str) -> Vec<MemoryQuery> {
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = trimmed.splitn(2, '|').collect();
        if parts.len() != 2 {
            continue;
        }
        let Some(tag) = FactKind::parse(parts[0].trim()) else { continue };
        let query = parts[1].trim().trim_matches(|c: char| c == '"' || c == '\'').to_owned();
        if query.is_empty() || query.split_whitespace().count() > 12 {
            continue;
        }
        out.push(MemoryQuery { tag, query });
        if out.len() >= MAX_QUERIES {
            break;
        }
    }
    out
}

/// Look up facts matching each query via token overlap on slug+content.
/// Returns deduped facts sorted by best-match-first. `active_skill` filters
/// behavioral queries to that skill's scope (falling back to "default").
pub fn match_facts(
    queries: &[MemoryQuery],
    store: &MemoryStore,
    active_skill: Option<&str>,
) -> Vec<Fact> {
    let mut hits: Vec<(usize, Fact)> = Vec::new();
    let mut seen_ids: HashSet<i64> = HashSet::new();
    for query in queries {
        let candidates = match store.list(Some(query.tag)) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let q_tokens = tokenize(&query.query);
        let scope = active_skill.unwrap_or("default");
        let mut scored: Vec<(usize, Fact)> = candidates
            .into_iter()
            .filter(|f| {
                if f.kind != FactKind::Behavioral {
                    return true;
                }
                f.skill_scope.as_deref().unwrap_or("default") == scope
            })
            .map(|f| {
                let score = overlap(&q_tokens, &tokenize(&format!("{} {}", f.slug, f.content)));
                (score, f)
            })
            .filter(|(score, _)| *score > 0)
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        for (score, fact) in scored.into_iter().take(MATCH_LIMIT_PER_QUERY) {
            if seen_ids.insert(fact.id) {
                hits.push((score, fact));
            }
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    hits.into_iter().map(|(_, f)| f).collect()
}

/// End-to-end: propose + match. Convenience for callers.
pub async fn fetch_relevant_facts(
    config: &LlamaConfig,
    scene: &str,
    store: &MemoryStore,
    active_skill: Option<&str>,
) -> (Vec<MemoryQuery>, Vec<Fact>) {
    let queries = propose_queries(config, scene).await;
    if queries.is_empty() {
        return (queries, Vec::new());
    }
    let facts = match_facts(&queries, store, active_skill);
    (queries, facts)
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .filter(|w| w.len() >= 2 && !is_stopword(w))
        .collect()
}

fn overlap(a: &[String], b: &[String]) -> usize {
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
            | "with"
            | "by"
            | "from"
            | "as"
            | "what"
            | "who"
            | "where"
            | "when"
            | "how"
            | "do"
            | "does"
    )
}

async fn collect_text(config: &LlamaConfig, messages: &[Message]) -> String {
    let mut stream = match stream_chat(config, messages).await {
        Ok(s) => Box::pin(s),
        Err(_) => return String::new(),
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
    full
}

#[cfg(test)]
mod tests {
    use super::{MemoryQuery, match_facts, parse_propose_response};
    use crate::memory::{FactKind, MemoryStore};

    #[test]
    fn parser_extracts_typed_queries() {
        let raw = "profile|user name\nconcept|user city\nbehavioral|reply length";
        let qs = parse_propose_response(raw);
        assert_eq!(qs.len(), 3);
        assert_eq!(qs[0].tag, FactKind::Profile);
        assert_eq!(qs[0].query, "user name");
        assert_eq!(qs[1].tag, FactKind::Concept);
        assert_eq!(qs[2].tag, FactKind::Behavioral);
    }

    #[test]
    fn parser_caps_at_three_queries() {
        let raw = "profile|a\nconcept|b\nstate|c\nbehavioral|d\nprofile|e";
        let qs = parse_propose_response(raw);
        assert_eq!(qs.len(), 3);
    }

    #[test]
    fn parser_drops_unknown_tags_and_overlong_queries() {
        let raw = "rubbish|x\nprofile|short ok\nconcept|this query is way too long because it has more than twelve words at minimum okay";
        let qs = parse_propose_response(raw);
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].query, "short ok");
    }

    #[test]
    fn parser_ignores_prose_around_lines() {
        let raw = "Here are the queries:\nprofile|name\nthat's it.\n";
        assert_eq!(parse_propose_response(raw).len(), 1);
    }

    #[test]
    fn parser_handles_empty_input() {
        assert!(parse_propose_response("").is_empty());
        assert!(parse_propose_response("\n\n\n").is_empty());
    }

    #[test]
    fn match_facts_returns_token_overlap_hits() {
        let store = MemoryStore::open_in_memory().unwrap();
        store.upsert(FactKind::Concept, "city", "User lives in Saigon, Vietnam.", None).unwrap();
        store.upsert(FactKind::Concept, "language", "Vietnamese, English.", None).unwrap();
        store.upsert(FactKind::Profile, "name", "Duc", None).unwrap();

        let queries =
            vec![MemoryQuery { tag: FactKind::Concept, query: "user saigon location".into() }];
        let hits = match_facts(&queries, &store, None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].slug, "city");
    }

    #[test]
    fn match_facts_filters_behavioral_by_active_skill() {
        let store = MemoryStore::open_in_memory().unwrap();
        store.upsert(FactKind::Behavioral, "tone", "prefers terse", Some("default")).unwrap();
        store
            .upsert(FactKind::Behavioral, "tone", "very dramatic, sighs", Some("grumpy-cat"))
            .unwrap();

        let queries =
            vec![MemoryQuery { tag: FactKind::Behavioral, query: "tone preference".into() }];
        let hits_default = match_facts(&queries, &store, None);
        assert_eq!(hits_default.len(), 1);
        assert!(hits_default[0].content.contains("terse"));

        let hits_grumpy = match_facts(&queries, &store, Some("grumpy-cat"));
        assert_eq!(hits_grumpy.len(), 1);
        assert!(hits_grumpy[0].content.contains("dramatic"));
    }

    #[test]
    fn match_facts_dedupes_across_queries() {
        let store = MemoryStore::open_in_memory().unwrap();
        store.upsert(FactKind::Concept, "rust", "Daily driver language", None).unwrap();
        let queries = vec![
            MemoryQuery { tag: FactKind::Concept, query: "rust language".into() },
            MemoryQuery { tag: FactKind::Concept, query: "driver rust".into() },
        ];
        let hits = match_facts(&queries, &store, None);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn match_facts_returns_empty_when_no_overlap() {
        let store = MemoryStore::open_in_memory().unwrap();
        store.upsert(FactKind::Concept, "rust", "language", None).unwrap();
        let queries = vec![MemoryQuery {
            tag: FactKind::Concept,
            query: "completely unrelated cabbage".into(),
        }];
        let hits = match_facts(&queries, &store, None);
        assert!(hits.is_empty());
    }
}

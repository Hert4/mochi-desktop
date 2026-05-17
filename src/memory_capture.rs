use crate::agent::llama_client::{ChatEvent, LlamaConfig, Message, stream_chat};
use crate::memory::FactKind;
use futures::StreamExt as _;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFact {
    pub kind: FactKind,
    pub slug: String,
    pub content: String,
}

const EXTRACTION_SYSTEM: &str = "You extract DURABLE user facts from a chat message — facts that would still be true next week, in a separate conversation. \
Output ONE fact per line in the exact format: `KIND|SLUG|VALUE`.\n\n\
KIND ∈ {profile, concept, state, behavioral}:\n\
- profile: stable user identity (name, age, profession)\n\
- concept: attributes ABOUT the user — their location, their languages, their employer, their hardware. NOT subjects/topics they merely brought up in chat.\n\
- state: the user's persistent current project or focus. NOT the immediate conversation flow.\n\
- behavioral: how the user prefers Mochi to communicate (length, tone, language mix)\n\n\
SLUG: short kebab-case key. Reuse stable slugs across messages (`name`, `location`, `tone`) so the same attribute updates the same slot.\n\
VALUE: terse literal value. No sentences, no meta-commentary, no third-person preamble.\n\n\
The single test for every emitted fact:\n\
> If the user opened a NEW Mochi session next month and never mentioned this again, would this fact still describe them?\n\n\
If yes → emit. If no → do NOT emit.\n\n\
Reject these categories outright:\n\
- topics, media, entities the user merely asked or talked about\n\
- one-shot interests, single-mention preferences without strong commitment language\n\
- meta-observations about how the user phrases or formats messages\n\
- describes the current conversation flow rather than the user themselves\n\
- anything the assistant said or did\n\n\
Hard rules:\n\
- Small-talk / greeting / question / request → output NOTHING.\n\
- When unsure → output NOTHING. False negatives are cheaper than memory bloat.\n\
- No prose. No JSON. No markdown. Only fact lines, or nothing.";

const EXTRACTION_TEMPERATURE: f32 = 0.0;
const EXTRACTION_MAX_TOKENS: u32 = 256;

pub async fn capture_facts(base_config: &LlamaConfig, user_text: &str) -> Vec<CapturedFact> {
    if user_text.trim().is_empty() {
        return Vec::new();
    }
    let config = LlamaConfig {
        url: base_config.url.clone(),
        model: base_config.model.clone(),
        temperature: Some(EXTRACTION_TEMPERATURE),
        max_tokens: Some(EXTRACTION_MAX_TOKENS),
    };
    let messages = vec![Message::system(EXTRACTION_SYSTEM), Message::user(user_text)];
    let mut stream = match stream_chat(&config, &messages).await {
        Ok(s) => Box::pin(s),
        Err(_) => return Vec::new(),
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
    parse_capture_response(&full)
}

pub fn parse_capture_response(text: &str) -> Vec<CapturedFact> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, '|').collect();
        if parts.len() != 3 {
            continue;
        }
        let Some(kind) = FactKind::parse(parts[0].trim()) else {
            continue;
        };
        let slug = normalize_slug(parts[1].trim());
        let content = parts[2].trim().trim_matches('"').trim_matches('\'').to_owned();
        if slug.is_empty() || content.is_empty() {
            continue;
        }
        out.push(CapturedFact { kind, slug, content });
    }
    out
}

fn normalize_slug(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_was_dash = true;
    for ch in raw.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_was_dash = false;
        } else if ch == '-' || ch == '_' || ch.is_whitespace() {
            if !last_was_dash {
                out.push('-');
                last_was_dash = true;
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{CapturedFact, normalize_slug, parse_capture_response};
    use crate::memory::FactKind;

    #[test]
    fn parses_two_well_formed_lines() {
        let txt =
            "profile|name|User's name is Duc.\nconcept|location|User lives in Saigon, Vietnam.";
        let facts = parse_capture_response(txt);
        assert_eq!(
            facts,
            vec![
                CapturedFact {
                    kind: FactKind::Profile,
                    slug: "name".to_owned(),
                    content: "User's name is Duc.".to_owned(),
                },
                CapturedFact {
                    kind: FactKind::Concept,
                    slug: "location".to_owned(),
                    content: "User lives in Saigon, Vietnam.".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn ignores_prose_around_facts() {
        let txt = "Here are the facts:\nprofile|name|Duc\nThat's all.\n";
        assert_eq!(parse_capture_response(txt).len(), 1);
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert!(parse_capture_response("").is_empty());
        assert!(parse_capture_response("   \n\n   ").is_empty());
    }

    #[test]
    fn rejects_unknown_kind() {
        assert!(parse_capture_response("rubbish|slug|content").is_empty());
    }

    #[test]
    fn handles_pipes_inside_content() {
        let facts = parse_capture_response("concept|languages|knows Rust|Python|TypeScript");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "knows Rust|Python|TypeScript");
    }

    #[test]
    fn normalize_slug_kebab_lower() {
        assert_eq!(normalize_slug("My Cool Slug"), "my-cool-slug");
        assert_eq!(normalize_slug("UPPER_case"), "upper-case");
        assert_eq!(normalize_slug("--leading-and-trailing--"), "leading-and-trailing");
        assert_eq!(normalize_slug("with!!punct??"), "withpunct");
    }
}

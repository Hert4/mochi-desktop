use crate::agent::llama_client::{ChatEvent, LlamaConfig, Message, stream_chat};
use crate::memory::FactKind;
use futures::StreamExt as _;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFact {
    pub kind: FactKind,
    pub slug: String,
    pub content: String,
}

const EXTRACTION_SYSTEM: &str = "You extract durable user facts from a chat message. \
Output ONE fact per line in the exact format: `KIND|SLUG|VALUE`.\n\n\
KIND ∈ {profile, concept, state, behavioral}:\n\
- profile: user identity (name, age, role)\n\
- concept: named entities about the user (city, language, project, employer)\n\
- state: user's current task or focus\n\
- behavioral: user preferences or communication patterns\n\n\
SLUG: short kebab-case key like `name`, `location`, `prefers-terse`.\n\
VALUE: the literal value or single-clause phrase. Be terse. NO sentences, NO meta-commentary.\n\n\
GOOD examples:\n\
- profile|name|Duc\n\
- concept|location|Saigon, Vietnam\n\
- concept|language|Vietnamese, English\n\
- behavioral|tone|prefers terse 1-3 sentence replies\n\
- state|focus|building a terminal AI pet in Rust\n\n\
BAD (DO NOT emit anything like these — too verbose/meta):\n\
- profile|name|Duc is a user who has shared their name\n\
- behavioral|prefers-terse|The user prefers concise communication and avoids elaborate expressions\n\n\
Rules:\n\
- Only emit facts the user EXPLICITLY stated about themselves in THIS message.\n\
- DO NOT extract facts about you (the assistant), other people, or speculation.\n\
- If no durable user fact (greeting, joke, question, story), output NOTHING.\n\
- No prose, no JSON, no markdown — only fact lines, or nothing at all.";

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

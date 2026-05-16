use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub frontmatter: BTreeMap<String, String>,
    pub body: String,
    pub source: PathBuf,
}

#[must_use]
pub fn default_skills_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".mochi").join("skills"))
}

pub fn load_all(dir: &Path) -> anyhow::Result<BTreeMap<String, Skill>> {
    let mut out = BTreeMap::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }
        let raw = std::fs::read_to_string(&skill_md)?;
        match parse_skill(&raw, &skill_md) {
            Ok(skill) => {
                out.insert(skill.name.clone(), skill);
            }
            Err(err) => {
                eprintln!("warn: failed to load skill at {}: {err}", skill_md.display());
            }
        }
    }
    Ok(out)
}

pub fn parse_skill(raw: &str, source: &Path) -> anyhow::Result<Skill> {
    let (frontmatter, body) = split_frontmatter(raw)?;
    let name = frontmatter
        .get("name")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing `name` in frontmatter"))?;
    let description = frontmatter.get("description").cloned().unwrap_or_default();
    Ok(Skill {
        name,
        description,
        frontmatter,
        body: body.trim().to_owned(),
        source: source.to_path_buf(),
    })
}

fn split_frontmatter(raw: &str) -> anyhow::Result<(BTreeMap<String, String>, String)> {
    let normalized: String =
        if raw.contains("\r\n") { raw.replace("\r\n", "\n") } else { raw.to_owned() };
    let trimmed = normalized.trim_start_matches(|c: char| c == '\u{feff}' || c.is_whitespace());
    let Some(rest) = trimmed.strip_prefix("---") else {
        return Err(anyhow::anyhow!("file does not start with `---` frontmatter delimiter"));
    };
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let end = rest
        .find("\n---")
        .ok_or_else(|| anyhow::anyhow!("missing closing `---` frontmatter delimiter"))?;
    let fm_block = &rest[..end];
    let body_start = end + 4;
    let body = if body_start >= rest.len() {
        String::new()
    } else {
        let after = &rest[body_start..];
        after.strip_prefix('\n').unwrap_or(after).to_owned()
    };

    let mut fm = BTreeMap::new();
    for line in fm_block.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_owned();
            let val = v.trim().trim_matches('"').trim_matches('\'').to_owned();
            if !key.is_empty() {
                fm.insert(key, val);
            }
        }
    }
    Ok((fm, body))
}

#[cfg(test)]
mod tests {
    use super::{parse_skill, split_frontmatter};
    use std::path::Path;

    #[test]
    fn parses_minimal_skill() {
        let raw = "---\nname: grumpy\ndescription: Be grumpy\n---\nYou are grumpy.\n";
        let skill = parse_skill(raw, Path::new("/tmp/SKILL.md")).unwrap();
        assert_eq!(skill.name, "grumpy");
        assert_eq!(skill.description, "Be grumpy");
        assert_eq!(skill.body, "You are grumpy.");
    }

    #[test]
    fn parses_quoted_values_and_preserves_inner_colons() {
        let raw = "---\nname: \"q-name\"\ndescription: 'with: colons'\n---\nbody\n";
        let skill = parse_skill(raw, Path::new("/tmp/SKILL.md")).unwrap();
        assert_eq!(skill.name, "q-name");
        assert_eq!(skill.description, "with: colons");
    }

    #[test]
    fn rejects_missing_opening_delimiter() {
        let raw = "name: grumpy\n";
        assert!(parse_skill(raw, Path::new("/tmp/SKILL.md")).is_err());
    }

    #[test]
    fn rejects_missing_closing_delimiter() {
        let raw = "---\nname: grumpy\n";
        assert!(parse_skill(raw, Path::new("/tmp/SKILL.md")).is_err());
    }

    #[test]
    fn rejects_missing_name() {
        let raw = "---\ndescription: only\n---\nbody\n";
        assert!(parse_skill(raw, Path::new("/tmp/SKILL.md")).is_err());
    }

    #[test]
    fn body_can_be_empty() {
        let raw = "---\nname: only\n---\n";
        let (fm, body) = split_frontmatter(raw).unwrap();
        assert_eq!(fm.get("name").unwrap(), "only");
        assert!(body.is_empty());
    }
}

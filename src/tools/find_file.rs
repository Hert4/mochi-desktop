use crate::agent::llama_client::ToolDef;
use ignore::WalkBuilder;
use serde_json::{Value, json};
use std::path::PathBuf;

const MAX_HITS: usize = 20;
const MAX_DEPTH: usize = 8;

pub fn spec() -> ToolDef {
    super::tool_def(
        "find_file",
        "Find files whose name matches a substring, recursively from a base directory. Respects .gitignore. Use this BEFORE read_file when the user mentions a filename but the exact path is unknown — call find_file first to locate it, then read_file on the discovered path. Case-insensitive. Returns up to 20 matches.",
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Filename or substring to search for. Case-insensitive."
                },
                "base": {
                    "type": "string",
                    "description": "Base directory to search. Defaults to the current working directory. Supports `~` for home."
                }
            },
            "required": ["name"],
            "additionalProperties": false,
        }),
    )
}

pub async fn execute(args: &Value) -> anyhow::Result<String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("find_file: missing `name`"))?;
    let base = args.get("base").and_then(Value::as_str).unwrap_or(".");
    let base_path = expand(base)?;

    if !base_path.exists() {
        return Err(anyhow::anyhow!("find_file: base {} does not exist", base_path.display()));
    }

    let needle = name.to_lowercase();
    let mut hits: Vec<PathBuf> = Vec::new();

    let walk = WalkBuilder::new(&base_path)
        .max_depth(Some(MAX_DEPTH))
        .follow_links(false)
        .standard_filters(true)
        .build();

    for entry in walk {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(fname) = entry.file_name().to_str() else { continue };
        if fname.to_lowercase().contains(&needle) {
            hits.push(entry.path().to_path_buf());
            if hits.len() >= MAX_HITS {
                break;
            }
        }
    }

    if hits.is_empty() {
        return Ok(format!(
            "no files matching `{name}` found under {} (depth ≤ {MAX_DEPTH}, .gitignore respected)",
            base_path.display()
        ));
    }

    let mut out = format!("found {} match(es) for `{name}`:\n", hits.len());
    for p in hits {
        out.push_str(&format!("- {}\n", p.display()));
    }
    if out.len() > 4096 {
        out.truncate(4096);
        out.push_str("\n[truncated]");
    }
    Ok(out)
}

fn expand(s: &str) -> anyhow::Result<PathBuf> {
    if let Some(rest) = s.strip_prefix("~/") {
        dirs::home_dir()
            .map(|h| h.join(rest))
            .ok_or_else(|| anyhow::anyhow!("cannot resolve home directory"))
    } else if s == "~" {
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot resolve home directory"))
    } else {
        Ok(PathBuf::from(s))
    }
}

#[cfg(test)]
mod tests {
    use super::execute;
    use serde_json::json;
    use std::io::Write as _;
    use tempfile::tempdir;

    #[tokio::test]
    async fn finds_a_file_by_substring() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("Cargo.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "[package]").unwrap();

        let out = execute(&json!({
            "name": "cargo",
            "base": dir.path().to_string_lossy()
        }))
        .await
        .unwrap();
        assert!(out.contains("Cargo.toml"));
        assert!(out.contains("1 match"));
    }

    #[tokio::test]
    async fn returns_no_matches_message_when_empty() {
        let dir = tempdir().unwrap();
        let out = execute(&json!({
            "name": "nonexistent-xyz-mochi",
            "base": dir.path().to_string_lossy()
        }))
        .await
        .unwrap();
        assert!(out.contains("no files matching"));
    }

    #[tokio::test]
    async fn missing_name_arg_returns_err() {
        let result = execute(&json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn nonexistent_base_returns_err() {
        let result = execute(&json!({
            "name": "x",
            "base": "/nonexistent/deeply/mochi/test"
        }))
        .await;
        assert!(result.is_err());
    }
}

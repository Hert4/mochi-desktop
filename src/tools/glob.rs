//! `Glob` tool — match files by glob pattern. Mirrors the Anthropic SDK `Glob`
//! schema: `pattern` (required), `path` (optional base, defaults to CWD).
//! Patterns without wildcards are matched as `**/{pattern}` so a bare filename
//! like "Cargo.toml" still finds nested matches.

use crate::agent::llama_client::ToolDef;
use globset::{Glob, GlobMatcher};
use ignore::WalkBuilder;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const MAX_HITS: usize = 50;
const MAX_DEPTH: usize = 10;

pub fn spec() -> ToolDef {
    super::tool_def(
        "Glob",
        "Find files by glob pattern, recursively from a base directory. Use BEFORE Read when the user mentions a filename but the path is uncertain. Patterns: `**/Cargo.toml`, `src/**/*.rs`, `*.md`. Bare filenames like `Cargo.toml` are auto-expanded to `**/Cargo.toml`. Respects .gitignore. Returns up to 50 matches.",
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern (e.g. `**/*.rs`, `Cargo.toml`, `docs/*.md`)."
                },
                "path": {
                    "type": "string",
                    "description": "Optional base directory to search. Defaults to the current working directory. Supports `~`."
                }
            },
            "required": ["pattern"],
            "additionalProperties": false,
        }),
    )
}

pub async fn execute(args: &Value) -> anyhow::Result<String> {
    let pattern = args
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Glob: missing `pattern`"))?;
    let base = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let base_path = expand(base)?;
    if !base_path.exists() {
        return Err(anyhow::anyhow!("Glob: base {} does not exist", base_path.display()));
    }

    let canonical_pattern =
        if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
            pattern.to_owned()
        } else {
            format!("**/{pattern}")
        };

    let matcher = Glob::new(&canonical_pattern)
        .map_err(|e| anyhow::anyhow!("Glob: invalid pattern `{canonical_pattern}`: {e}"))?
        .compile_matcher();

    let hits = collect_hits(&base_path, &matcher);

    if hits.is_empty() {
        return Ok(format!(
            "no files matching `{canonical_pattern}` under {} (.gitignore respected, depth ≤ {MAX_DEPTH})",
            base_path.display()
        ));
    }

    let shown = hits.len().min(MAX_HITS);
    let mut out =
        format!("{} match(es) for `{canonical_pattern}` (showing {shown}):\n", hits.len());
    for p in hits.into_iter().take(MAX_HITS) {
        out.push_str(&format!("- {}\n", p.display()));
    }
    Ok(out)
}

fn collect_hits(base: &Path, matcher: &GlobMatcher) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    let walk = WalkBuilder::new(base)
        .max_depth(Some(MAX_DEPTH))
        .follow_links(false)
        .standard_filters(true)
        .build();
    for entry in walk {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = entry.path().strip_prefix(base).unwrap_or(entry.path());
        if matcher.is_match(rel) || matcher.is_match(entry.path()) {
            hits.push(entry.path().to_path_buf());
            if hits.len() >= MAX_HITS * 2 {
                break;
            }
        }
    }
    hits
}

fn expand(s: &str) -> anyhow::Result<PathBuf> {
    if let Some(rest) = s.strip_prefix("~/") {
        dirs::home_dir().map(|h| h.join(rest)).ok_or_else(|| anyhow::anyhow!("cannot resolve home"))
    } else if s == "~" {
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot resolve home"))
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
    async fn bare_filename_is_treated_as_recursive_glob() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        let path = dir.path().join("nested").join("Cargo.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "[package]").unwrap();

        let out = execute(&json!({
            "pattern": "Cargo.toml",
            "path": dir.path().to_string_lossy()
        }))
        .await
        .unwrap();
        assert!(out.contains("Cargo.toml"));
        assert!(out.contains("1 match"));
    }

    #[tokio::test]
    async fn star_pattern_works() {
        let dir = tempdir().unwrap();
        for name in &["a.rs", "b.rs", "c.txt"] {
            std::fs::File::create(dir.path().join(name)).unwrap();
        }
        let out = execute(&json!({
            "pattern": "*.rs",
            "path": dir.path().to_string_lossy()
        }))
        .await
        .unwrap();
        assert!(out.contains("a.rs"));
        assert!(out.contains("b.rs"));
        assert!(!out.contains("c.txt"));
    }

    #[tokio::test]
    async fn missing_pattern_errs() {
        assert!(execute(&json!({})).await.is_err());
    }

    #[tokio::test]
    async fn invalid_pattern_errs() {
        let result = execute(&json!({"pattern": "[unclosed"})).await;
        assert!(result.is_err());
    }
}

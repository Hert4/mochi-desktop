//! `Edit` tool. Mirrors Anthropic SDK `Edit` schema: `file_path`, `old_string`,
//! `new_string`, optional `replace_all` (default false). Unique-match semantics:
//! if `replace_all` is false, `old_string` must occur exactly once.

use super::ToolResult;
use crate::agent::llama_client::ToolDef;
use serde_json::{Value, json};
use std::path::PathBuf;

pub fn spec() -> ToolDef {
    super::tool_def(
        "Edit",
        "Replace `old_string` with `new_string` inside a file. By default `old_string` must occur exactly once; set `replace_all: true` to replace every occurrence. Requires user permission. Always Read the file first to know the exact `old_string` (including indentation).",
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute or relative path to the file to edit."
                },
                "old_string": {
                    "type": "string",
                    "description": "Exact text to find and replace. Must match including whitespace."
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement text."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace every occurrence (default false — requires unique match)."
                }
            },
            "required": ["file_path", "old_string", "new_string"],
            "additionalProperties": false,
        }),
    )
}

pub async fn execute(args: &Value) -> anyhow::Result<ToolResult> {
    let path_str = args
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Edit: missing `file_path`"))?;
    let old_string = args
        .get("old_string")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Edit: missing `old_string`"))?;
    let new_string = args
        .get("new_string")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Edit: missing `new_string`"))?;
    let replace_all = args.get("replace_all").and_then(Value::as_bool).unwrap_or(false);

    if old_string.is_empty() {
        return Err(anyhow::anyhow!(
            "Edit: `old_string` cannot be empty (use Write for new files)"
        ));
    }
    if old_string == new_string {
        return Err(anyhow::anyhow!("Edit: `old_string` and `new_string` are identical — no-op"));
    }

    let resolved = expand_tilde(path_str)?;
    let original = tokio::fs::read_to_string(&resolved)
        .await
        .map_err(|e| anyhow::anyhow!("Edit: cannot read {}: {e}", resolved.display()))?;

    let occurrences = original.matches(old_string).count();
    if occurrences == 0 {
        return Err(anyhow::anyhow!(
            "Edit: `old_string` not found in {}. Read the file first and copy the exact text including indentation.",
            resolved.display()
        ));
    }
    if !replace_all && occurrences > 1 {
        return Err(anyhow::anyhow!(
            "Edit: `old_string` found {occurrences} times in {} but `replace_all` is false. Set replace_all: true or provide a more specific old_string.",
            resolved.display()
        ));
    }

    let updated = if replace_all {
        original.replace(old_string, new_string)
    } else {
        original.replacen(old_string, new_string, 1)
    };

    tokio::fs::write(&resolved, &updated)
        .await
        .map_err(|e| anyhow::anyhow!("Edit: cannot write {}: {e}", resolved.display()))?;

    let summary = if replace_all {
        format!("Edited {} (replaced {occurrences} occurrence(s))", resolved.display())
    } else {
        format!("Edited {} (1 of {occurrences} occurrence(s))", resolved.display())
    };

    Ok(ToolResult::diff(resolved.to_string_lossy(), original, updated, summary))
}

fn expand_tilde(s: &str) -> anyhow::Result<PathBuf> {
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
    use crate::agent::types::ToolCallContent;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn unique_match_replaces_once() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "alpha beta gamma").await.unwrap();
        let result = execute(&json!({
            "file_path": path.to_string_lossy(),
            "old_string": "beta",
            "new_string": "BETA",
        }))
        .await
        .unwrap();
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "alpha BETA gamma");
        if let ToolCallContent::Diff { old, new, .. } = &result.ui_content[0] {
            assert_eq!(old, "alpha beta gamma");
            assert_eq!(new, "alpha BETA gamma");
        } else {
            panic!("expected Diff variant");
        }
    }

    #[tokio::test]
    async fn multiple_matches_without_replace_all_errs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "x x x").await.unwrap();
        let result = execute(&json!({
            "file_path": path.to_string_lossy(),
            "old_string": "x",
            "new_string": "y",
        }))
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("3 times") || err.contains("3 occurrence"));
        // File untouched
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "x x x");
    }

    #[tokio::test]
    async fn replace_all_replaces_every_occurrence() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "x x x").await.unwrap();
        execute(&json!({
            "file_path": path.to_string_lossy(),
            "old_string": "x",
            "new_string": "y",
            "replace_all": true,
        }))
        .await
        .unwrap();
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "y y y");
    }

    #[tokio::test]
    async fn no_match_errs_with_hint() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "alpha").await.unwrap();
        let result = execute(&json!({
            "file_path": path.to_string_lossy(),
            "old_string": "beta",
            "new_string": "gamma",
        }))
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn empty_old_string_errs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "x").await.unwrap();
        let result = execute(&json!({
            "file_path": path.to_string_lossy(),
            "old_string": "",
            "new_string": "y",
        }))
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn identical_old_new_errs_as_noop() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "same").await.unwrap();
        let result = execute(&json!({
            "file_path": path.to_string_lossy(),
            "old_string": "same",
            "new_string": "same",
        }))
        .await;
        assert!(result.is_err());
    }
}

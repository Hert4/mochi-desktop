//! `MultiEdit` tool. Mirrors Anthropic SDK schema: `file_path` + `edits: [{old_string, new_string, replace_all?}]`.
//! All edits apply atomically in memory — if any one fails the file is left
//! untouched. Emits one `Diff` block (initial → final) for the UI.

use super::ToolResult;
use crate::agent::llama_client::ToolDef;
use serde_json::{Value, json};
use std::path::PathBuf;

pub fn spec() -> ToolDef {
    super::tool_def(
        "MultiEdit",
        "Apply multiple sequential find-and-replace edits to a single file atomically. Each edit's `old_string` is matched against the state PRODUCED BY PRIOR EDITS in the batch (so the first edit may transform text the second edit relies on). If any edit fails (no match / ambiguous match / identical) the file is not modified. Requires user permission.",
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to edit."
                },
                "edits": {
                    "type": "array",
                    "description": "Ordered list of find-and-replace edits.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": {"type": "string"},
                            "new_string": {"type": "string"},
                            "replace_all": {"type": "boolean"}
                        },
                        "required": ["old_string", "new_string"],
                        "additionalProperties": false
                    },
                    "minItems": 1
                }
            },
            "required": ["file_path", "edits"],
            "additionalProperties": false,
        }),
    )
}

pub async fn execute(args: &Value) -> anyhow::Result<ToolResult> {
    let path_str = args
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("MultiEdit: missing `file_path`"))?;
    let edits_val = args
        .get("edits")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("MultiEdit: missing `edits` array"))?;
    if edits_val.is_empty() {
        return Err(anyhow::anyhow!("MultiEdit: `edits` array is empty"));
    }

    let resolved = expand_tilde(path_str)?;
    let original = tokio::fs::read_to_string(&resolved)
        .await
        .map_err(|e| anyhow::anyhow!("MultiEdit: cannot read {}: {e}", resolved.display()))?;

    let mut current = original.clone();
    for (idx, edit) in edits_val.iter().enumerate() {
        let old_string = edit
            .get("old_string")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("MultiEdit: edit #{idx} missing `old_string`"))?;
        let new_string = edit
            .get("new_string")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("MultiEdit: edit #{idx} missing `new_string`"))?;
        let replace_all = edit.get("replace_all").and_then(Value::as_bool).unwrap_or(false);

        if old_string.is_empty() {
            return Err(anyhow::anyhow!("MultiEdit: edit #{idx} has empty old_string"));
        }
        if old_string == new_string {
            return Err(anyhow::anyhow!("MultiEdit: edit #{idx} is a no-op"));
        }

        let occurrences = current.matches(old_string).count();
        if occurrences == 0 {
            return Err(anyhow::anyhow!(
                "MultiEdit: edit #{idx} old_string not found in current state of {}",
                resolved.display()
            ));
        }
        if !replace_all && occurrences > 1 {
            return Err(anyhow::anyhow!(
                "MultiEdit: edit #{idx} matched {occurrences} times but `replace_all` is false"
            ));
        }

        current = if replace_all {
            current.replace(old_string, new_string)
        } else {
            current.replacen(old_string, new_string, 1)
        };
    }

    tokio::fs::write(&resolved, &current)
        .await
        .map_err(|e| anyhow::anyhow!("MultiEdit: cannot write {}: {e}", resolved.display()))?;

    let summary = format!("Applied {} edits to {}", edits_val.len(), resolved.display());
    Ok(ToolResult::diff(resolved.to_string_lossy(), original, current, summary))
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
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn applies_sequential_edits_atomically() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "alpha\nbeta\ngamma").await.unwrap();
        execute(&json!({
            "file_path": path.to_string_lossy(),
            "edits": [
                {"old_string": "alpha", "new_string": "ALPHA"},
                {"old_string": "beta", "new_string": "BETA"},
            ]
        }))
        .await
        .unwrap();
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "ALPHA\nBETA\ngamma");
    }

    #[tokio::test]
    async fn second_edit_sees_first_edits_output() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "foo").await.unwrap();
        execute(&json!({
            "file_path": path.to_string_lossy(),
            "edits": [
                {"old_string": "foo", "new_string": "bar"},
                {"old_string": "bar", "new_string": "baz"},
            ]
        }))
        .await
        .unwrap();
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "baz");
    }

    #[tokio::test]
    async fn rollback_on_any_failure() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "alpha beta").await.unwrap();
        let result = execute(&json!({
            "file_path": path.to_string_lossy(),
            "edits": [
                {"old_string": "alpha", "new_string": "ALPHA"},
                {"old_string": "MISSING", "new_string": "X"},
            ]
        }))
        .await;
        assert!(result.is_err());
        // File untouched — first edit was never persisted.
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "alpha beta");
    }

    #[tokio::test]
    async fn empty_edits_array_errs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "x").await.unwrap();
        let result = execute(&json!({
            "file_path": path.to_string_lossy(),
            "edits": [],
        }))
        .await;
        assert!(result.is_err());
    }
}

//! `Write` tool. Mirrors Anthropic SDK `Write` schema: `file_path`, `content`.
//! Creates a new file or overwrites an existing one. Emits `ToolCallContent::Diff`
//! so CCR renders an inline diff for user review before approval.

use super::ToolResult;
use crate::agent::llama_client::ToolDef;
use serde_json::{Value, json};
use std::path::PathBuf;

pub fn spec() -> ToolDef {
    super::tool_def(
        "Write",
        "Write content to a file, creating it if missing or overwriting if it exists. Always pair with Read first if you intend to overwrite an existing file. Requires user permission.",
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute or relative path to the file to write. Tilde (~) expands to home."
                },
                "content": {
                    "type": "string",
                    "description": "Full file contents to write."
                }
            },
            "required": ["file_path", "content"],
            "additionalProperties": false,
        }),
    )
}

pub async fn execute(args: &Value) -> anyhow::Result<ToolResult> {
    let path_str = args
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Write: missing `file_path`"))?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Write: missing `content`"))?;

    let resolved = expand_tilde(path_str)?;

    let old = match tokio::fs::read_to_string(&resolved).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(anyhow::anyhow!("Write: cannot read existing file: {e}")),
    };

    if let Some(parent) = resolved.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| anyhow::anyhow!("Write: cannot create parent dir: {e}"))?;
        }
    }

    tokio::fs::write(&resolved, content)
        .await
        .map_err(|e| anyhow::anyhow!("Write: cannot write {}: {e}", resolved.display()))?;

    let summary = if old.is_empty() {
        format!("Wrote {} bytes to new file {}", content.len(), resolved.display())
    } else {
        format!("Overwrote {} ({} bytes → {} bytes)", resolved.display(), old.len(), content.len())
    };

    Ok(ToolResult::diff(resolved.to_string_lossy(), old, content.to_owned(), summary))
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
    async fn creates_new_file_and_emits_diff() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("new.txt");
        let result = execute(&json!({
            "file_path": path.to_string_lossy(),
            "content": "hello mochi\n"
        }))
        .await
        .unwrap();
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "hello mochi\n");
        assert!(result.model_text.contains("new file"));
        assert_eq!(result.ui_content.len(), 1);
        if let ToolCallContent::Diff { old, new, .. } = &result.ui_content[0] {
            assert!(old.is_empty());
            assert_eq!(new, "hello mochi\n");
        } else {
            panic!("expected Diff variant");
        }
    }

    #[tokio::test]
    async fn overwrites_existing_with_diff_showing_old_and_new() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("exists.txt");
        tokio::fs::write(&path, "before").await.unwrap();
        let result = execute(&json!({
            "file_path": path.to_string_lossy(),
            "content": "after"
        }))
        .await
        .unwrap();
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "after");
        if let ToolCallContent::Diff { old, new, .. } = &result.ui_content[0] {
            assert_eq!(old, "before");
            assert_eq!(new, "after");
        } else {
            panic!("expected Diff variant");
        }
    }

    #[tokio::test]
    async fn creates_parent_directories() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a/b/c/deep.txt");
        execute(&json!({
            "file_path": path.to_string_lossy(),
            "content": "deep"
        }))
        .await
        .unwrap();
        assert!(path.exists());
    }

    #[tokio::test]
    async fn missing_required_args_err() {
        assert!(execute(&json!({})).await.is_err());
        assert!(execute(&json!({"file_path": "/tmp/x"})).await.is_err());
    }
}

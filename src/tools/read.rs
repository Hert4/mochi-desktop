//! `Read` tool. Mirrors the Anthropic SDK `Read` schema: `file_path`, optional
//! `offset` and `limit` for line-windowed reads.

use crate::agent::llama_client::ToolDef;
use serde_json::{Value, json};
use std::path::PathBuf;

const MAX_BYTES: u64 = 200 * 1024;

pub fn spec() -> ToolDef {
    super::tool_def(
        "Read",
        "Read a file from disk and return its contents. Use when the user asks about a specific file's content, configuration, or code. If the path is uncertain, call Glob first to locate it. Returns at most 200KB.",
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute or relative path to the file. Tilde (~) expands to the user home directory."
                },
                "offset": {
                    "type": "integer",
                    "description": "Optional 1-indexed line number to start reading from."
                },
                "limit": {
                    "type": "integer",
                    "description": "Optional maximum number of lines to return."
                }
            },
            "required": ["file_path"],
            "additionalProperties": false,
        }),
    )
}

pub async fn execute(args: &Value) -> anyhow::Result<String> {
    let path_str = args
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Read: missing `file_path` string argument"))?;
    let offset = args.get("offset").and_then(Value::as_u64).map(|n| n as usize);
    let limit = args.get("limit").and_then(Value::as_u64).map(|n| n as usize);

    let resolved = expand_tilde(path_str)?;
    let metadata = tokio::fs::metadata(&resolved)
        .await
        .map_err(|e| anyhow::anyhow!("Read: cannot stat {}: {e}", resolved.display()))?;
    if !metadata.is_file() {
        return Err(anyhow::anyhow!("Read: {} is not a regular file", resolved.display()));
    }

    let len = metadata.len();
    let bytes = tokio::fs::read(&resolved)
        .await
        .map_err(|e| anyhow::anyhow!("Read: cannot read {}: {e}", resolved.display()))?;

    let truncated_size = bytes.len() as u64 > MAX_BYTES;
    let slice = if truncated_size { &bytes[..MAX_BYTES as usize] } else { &bytes[..] };
    let full_text = String::from_utf8_lossy(slice);

    let body = match (offset, limit) {
        (None, None) => full_text.into_owned(),
        _ => {
            let start = offset.unwrap_or(1).saturating_sub(1);
            let take = limit.unwrap_or(usize::MAX);
            full_text.lines().skip(start).take(take).collect::<Vec<_>>().join("\n")
        }
    };

    let mut out = format!("# {} ({} bytes)\n\n", resolved.display(), len);
    out.push_str(&body);
    if truncated_size {
        out.push_str("\n\n[truncated: file exceeds 200KB read limit]");
    }
    Ok(out)
}

fn expand_tilde(s: &str) -> anyhow::Result<PathBuf> {
    if let Some(rest) = s.strip_prefix("~/") {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("cannot resolve home directory for `~`"))?;
        Ok(home.join(rest))
    } else if s == "~" {
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot resolve home directory for `~`"))
    } else {
        Ok(PathBuf::from(s))
    }
}

#[cfg(test)]
mod tests {
    use super::execute;
    use serde_json::json;
    use std::io::Write as _;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn reads_full_file() {
        let mut tmp = NamedTempFile::new().unwrap();
        writeln!(tmp, "alpha\nbeta\ngamma").unwrap();
        let out = execute(&json!({"file_path": tmp.path().to_string_lossy()})).await.unwrap();
        assert!(out.contains("alpha"));
        assert!(out.contains("gamma"));
    }

    #[tokio::test]
    async fn respects_offset_and_limit() {
        let mut tmp = NamedTempFile::new().unwrap();
        writeln!(tmp, "1\n2\n3\n4\n5").unwrap();
        let out = execute(&json!({
            "file_path": tmp.path().to_string_lossy(),
            "offset": 2,
            "limit": 2,
        }))
        .await
        .unwrap();
        assert!(out.contains("2"));
        assert!(out.contains("3"));
        assert!(!out.contains("\n1"));
        assert!(!out.contains("\n5"));
    }

    #[tokio::test]
    async fn missing_file_path_arg_errs() {
        let result = execute(&json!({})).await;
        assert!(result.is_err());
    }
}

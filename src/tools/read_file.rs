use crate::agent::llama_client::ToolDef;
use serde_json::{Value, json};
use std::path::PathBuf;

const MAX_BYTES: u64 = 200 * 1024;

pub fn spec() -> ToolDef {
    super::tool_def(
        "read_file",
        "Read a file from disk and return its contents as text. Use this when the user asks about a specific file's content, configuration, or code. Returns at most 200KB; longer files are truncated.",
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or relative path to the file. Tilde (~) is expanded to the user home directory.",
                }
            },
            "required": ["path"],
            "additionalProperties": false,
        }),
    )
}

pub async fn execute(args: &Value) -> anyhow::Result<String> {
    let path_str = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("read_file: missing `path` string argument"))?;

    let resolved = expand_tilde(path_str)?;
    let metadata = tokio::fs::metadata(&resolved)
        .await
        .map_err(|e| anyhow::anyhow!("read_file: cannot stat {}: {e}", resolved.display()))?;
    if !metadata.is_file() {
        return Err(anyhow::anyhow!(
            "read_file: {} is not a regular file",
            resolved.display()
        ));
    }

    let len = metadata.len();
    let bytes = tokio::fs::read(&resolved)
        .await
        .map_err(|e| anyhow::anyhow!("read_file: cannot read {}: {e}", resolved.display()))?;

    let truncated = bytes.len() as u64 > MAX_BYTES;
    let slice = if truncated { &bytes[..MAX_BYTES as usize] } else { &bytes[..] };
    let text = String::from_utf8_lossy(slice).into_owned();

    let mut out = format!("# {} ({} bytes)\n\n", resolved.display(), len);
    out.push_str(&text);
    if truncated {
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
    use super::{execute, expand_tilde};
    use serde_json::json;
    use tempfile::NamedTempFile;
    use std::io::Write as _;

    #[tokio::test]
    async fn reads_a_file_with_content_header() {
        let mut tmp = NamedTempFile::new().unwrap();
        writeln!(tmp, "hello mochi").unwrap();
        let path = tmp.path().to_string_lossy().to_string();
        let out = execute(&json!({"path": path})).await.unwrap();
        assert!(out.contains("hello mochi"));
        assert!(out.starts_with("# "));
    }

    #[tokio::test]
    async fn returns_err_for_missing_path_arg() {
        let result = execute(&json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn returns_err_for_nonexistent_file() {
        let result = execute(&json!({"path": "/tmp/definitely-does-not-exist-mochi-test"})).await;
        assert!(result.is_err());
    }

    #[test]
    fn tilde_expands_to_home() {
        let home = dirs::home_dir().unwrap();
        let expanded = expand_tilde("~/foo").unwrap();
        assert_eq!(expanded, home.join("foo"));
        assert_eq!(expand_tilde("~").unwrap(), home);
        assert_eq!(expand_tilde("/abs/path").unwrap(), std::path::PathBuf::from("/abs/path"));
    }
}

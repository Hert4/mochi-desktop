//! `Bash` tool. Mirrors the Anthropic SDK `Bash` schema: `command` (required),
//! `timeout` (optional ms). Execution is sandbox-free for v0.1 — the runner is
//! expected to gate every Bash call behind a user permission prompt.

use super::ToolResult;
use crate::agent::llama_client::ToolDef;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::process::Command;

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 5 * 60 * 1000;
const MAX_OUTPUT_BYTES: usize = 100 * 1024;

pub fn spec() -> ToolDef {
    super::tool_def(
        "Bash",
        "Run a shell command via `bash -lc`. Returns stdout + stderr joined. Requires user permission before each call (the runtime will prompt). Use for build/test commands, file listings, environment checks. Default timeout 30s; max 5 min.",
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to run. Will be invoked as `bash -lc <command>`."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional timeout in milliseconds (default 30000, max 300000)."
                }
            },
            "required": ["command"],
            "additionalProperties": false,
        }),
    )
}

pub async fn execute(args: &Value) -> anyhow::Result<ToolResult> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Bash: missing `command`"))?;
    let timeout_ms = args
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .min(MAX_TIMEOUT_MS);

    let child = Command::new("bash")
        .arg("-lc")
        .arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Bash: spawn failed: {e}"))?;

    let wait = tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output());
    let output = match wait.await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(anyhow::anyhow!("Bash: process error: {e}")),
        Err(_) => {
            return Err(anyhow::anyhow!("Bash: timed out after {timeout_ms} ms"));
        }
    };

    let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        if !stdout.is_empty() && !stdout.ends_with('\n') {
            stdout.push('\n');
        }
        stdout.push_str("[stderr]\n");
        stdout.push_str(&stderr);
    }
    let truncated = stdout.len() > MAX_OUTPUT_BYTES;
    if truncated {
        stdout.truncate(MAX_OUTPUT_BYTES);
        stdout.push_str("\n[truncated: output exceeds 100KB]");
    }
    let exit = output.status.code().unwrap_or(-1);
    if exit != 0 {
        return Ok(ToolResult::text(format!("$ {command}\n[exit {exit}]\n{stdout}")));
    }
    Ok(ToolResult::text(format!("$ {command}\n{stdout}")))
}

#[cfg(test)]
mod tests {
    use super::execute;
    use serde_json::json;

    #[tokio::test]
    async fn captures_stdout() {
        let out = execute(&json!({"command": "echo hello mochi"})).await.unwrap();
        assert!(out.model_text.contains("hello mochi"));
        assert!(!out.model_text.contains("[exit"));
    }

    #[tokio::test]
    async fn captures_stderr_and_nonzero_exit() {
        let out = execute(&json!({"command": "echo on-stdout; echo on-stderr 1>&2; exit 3"}))
            .await
            .unwrap();
        assert!(out.model_text.contains("on-stdout"));
        assert!(out.model_text.contains("on-stderr"));
        assert!(out.model_text.contains("[exit 3]"));
    }

    #[tokio::test]
    async fn enforces_timeout() {
        let result = execute(&json!({"command": "sleep 5", "timeout": 100})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn missing_command_errs() {
        assert!(execute(&json!({})).await.is_err());
    }
}

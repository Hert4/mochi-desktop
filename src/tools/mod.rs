//! Tool implementations available to the llama provider.
//!
//! Tool names follow Anthropic SDK conventions (`Read`, `Glob`, `WebFetch`,
//! `Bash`, `Write` …) so the CCR-inherited renderer in `src/ui/tool_call/`
//! dispatches to the right icon and label without a name-mapping shim.

use crate::agent::llama_client::{ToolDef, ToolDefFunction};
use crate::agent::types::{ContentBlock, ToolCallContent};
use serde_json::{Value, json};

mod bash;
mod edit;
mod glob;
mod multi_edit;
mod read;
mod web_fetch;
mod web_search;
mod write;

/// Result of a tool invocation. Splits the model-facing summary from the
/// rich UI content blocks so write-side tools can emit `Diff` for inline
/// review while still feeding a compact string back to the LLM.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub model_text: String,
    pub ui_content: Vec<ToolCallContent>,
}

impl ToolResult {
    pub fn text(msg: impl Into<String>) -> Self {
        let s = msg.into();
        Self {
            ui_content: vec![ToolCallContent::Content {
                content: ContentBlock::Text { text: s.clone() },
            }],
            model_text: s,
        }
    }

    pub fn diff(
        path: impl Into<String>,
        old: String,
        new: String,
        summary: impl Into<String>,
    ) -> Self {
        let path_s = path.into();
        Self {
            ui_content: vec![ToolCallContent::Diff {
                old_path: path_s.clone(),
                new_path: path_s,
                old,
                new,
                repository: None,
            }],
            model_text: summary.into(),
        }
    }

    #[must_use]
    pub fn with_multiple_diffs(diffs: Vec<ToolCallContent>, summary: impl Into<String>) -> Self {
        Self { ui_content: diffs, model_text: summary.into() }
    }
}

/// All tool specs advertised to the model on every turn.
#[must_use]
pub fn available_tools() -> Vec<ToolDef> {
    vec![
        read::spec(),
        glob::spec(),
        web_search::spec(),
        web_fetch::spec(),
        bash::spec(),
        write::spec(),
        edit::spec(),
        multi_edit::spec(),
    ]
}

/// Whether running a tool requires explicit user permission via the
/// `PermissionRequest`/`PermissionResponse` channel before execution.
#[must_use]
pub fn needs_permission(name: &str) -> bool {
    matches!(name, "Bash" | "Write" | "Edit" | "MultiEdit")
}

pub async fn execute(name: &str, arguments: &str) -> anyhow::Result<ToolResult> {
    let args: Value = if arguments.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(arguments)
            .map_err(|e| anyhow::anyhow!("tool `{name}` arguments are not valid JSON: {e}"))?
    };
    match name {
        "Read" => read::execute(&args).await,
        "Glob" => glob::execute(&args).await,
        "WebFetch" => web_fetch::execute(&args).await,
        "WebSearch" => web_search::execute(&args).await,
        "Bash" => bash::execute(&args).await,
        "Write" => write::execute(&args).await,
        "Edit" => edit::execute(&args).await,
        "MultiEdit" => multi_edit::execute(&args).await,
        _ => Err(anyhow::anyhow!("unknown tool: {name}")),
    }
}

pub(crate) fn tool_def(
    name: impl Into<String>,
    description: impl Into<String>,
    parameters: Value,
) -> ToolDef {
    ToolDef {
        kind: "function",
        function: ToolDefFunction {
            name: name.into(),
            description: description.into(),
            parameters,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{ToolResult, available_tools, execute, needs_permission};
    use crate::agent::types::ToolCallContent;

    #[tokio::test]
    async fn unknown_tool_returns_err() {
        let result = execute("nonexistent", "{}").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn invalid_json_args_return_err() {
        let result = execute("Read", "{not-json").await;
        assert!(result.is_err());
    }

    #[test]
    fn registered_tool_names_match_anthropic_sdk_pascal_case() {
        let tools = available_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.function.name.as_str()).collect();
        for expected in
            ["Read", "Glob", "WebFetch", "WebSearch", "Bash", "Write", "Edit", "MultiEdit"]
        {
            assert!(names.contains(&expected), "missing tool {expected}");
        }
    }

    #[test]
    fn write_side_effects_need_permission_read_side_does_not() {
        for name in ["Bash", "Write", "Edit", "MultiEdit"] {
            assert!(needs_permission(name));
        }
        for name in ["Read", "Glob", "WebFetch", "WebSearch"] {
            assert!(!needs_permission(name));
        }
    }

    #[test]
    fn text_helper_round_trips_text_in_both_fields() {
        let r = ToolResult::text("hello");
        assert_eq!(r.model_text, "hello");
        assert_eq!(r.ui_content.len(), 1);
        matches!(r.ui_content[0], ToolCallContent::Content { .. });
    }

    #[test]
    fn diff_helper_keeps_summary_separate_from_diff_payload() {
        let r = ToolResult::diff("/tmp/x", "old".into(), "new".into(), "wrote 1 byte");
        assert_eq!(r.model_text, "wrote 1 byte");
        assert_eq!(r.ui_content.len(), 1);
        if let ToolCallContent::Diff { old_path, new_path, old, new, .. } = &r.ui_content[0] {
            assert_eq!(old_path, "/tmp/x");
            assert_eq!(new_path, "/tmp/x");
            assert_eq!(old, "old");
            assert_eq!(new, "new");
        } else {
            panic!("expected Diff variant");
        }
    }
}

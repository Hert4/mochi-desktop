//! Tool implementations available to the llama provider.
//!
//! Tool names follow Anthropic SDK conventions (`Read`, `Glob`, `WebFetch`,
//! `Bash`, `Write` …) so the CCR-inherited renderer in `src/ui/tool_call/`
//! dispatches to the right icon and label without a name-mapping shim.
//!
//! Add new tools by registering them in [`available_tools`] and dispatching
//! them in [`execute`].

use crate::agent::llama_client::{ToolDef, ToolDefFunction};
use serde_json::{Value, json};

mod bash;
mod glob;
mod read;
mod web_fetch;
mod web_search;

/// All tool specs advertised to the model on every turn.
#[must_use]
pub fn available_tools() -> Vec<ToolDef> {
    vec![
        read::spec(),
        glob::spec(),
        web_search::spec(),
        web_fetch::spec(),
        bash::spec(),
    ]
}

/// Whether running a tool requires explicit user permission via the
/// `PermissionRequest`/`PermissionResponse` channel before execution.
#[must_use]
pub fn needs_permission(name: &str) -> bool {
    matches!(name, "Bash" | "Write" | "Edit" | "MultiEdit")
}

/// Execute a tool call by name. Returns a string suitable for feeding back
/// into the model as the `tool` role message body. Permission gating happens
/// in the caller — this function unconditionally runs the side effect.
pub async fn execute(name: &str, arguments: &str) -> anyhow::Result<String> {
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
    use super::{available_tools, execute};

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
        assert!(names.contains(&"Read"));
        assert!(names.contains(&"Glob"));
        assert!(names.contains(&"WebFetch"));
        assert!(names.contains(&"Bash"));
        assert!(names.contains(&"WebSearch"));
    }

    #[test]
    fn write_side_effects_need_permission_read_side_does_not() {
        assert!(super::needs_permission("Bash"));
        assert!(super::needs_permission("Write"));
        assert!(super::needs_permission("Edit"));
        assert!(!super::needs_permission("Read"));
        assert!(!super::needs_permission("Glob"));
        assert!(!super::needs_permission("WebFetch"));
    }
}

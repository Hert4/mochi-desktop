//! Tool implementations available to the llama provider.
//!
//! v0.1 ships `read_file` only. Add new tools by registering them in
//! [`available_tools`] and dispatching them in [`execute`].

use crate::agent::llama_client::{ToolDef, ToolDefFunction};
use serde_json::{Value, json};

mod find_file;
mod read_file;

/// All tool specs advertised to the model on every turn.
#[must_use]
pub fn available_tools() -> Vec<ToolDef> {
    vec![find_file::spec(), read_file::spec()]
}

/// Execute a tool call by name. Returns a string suitable for feeding back
/// into the model as the `tool` role message body.
pub async fn execute(name: &str, arguments: &str) -> anyhow::Result<String> {
    let args: Value = if arguments.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(arguments)
            .map_err(|e| anyhow::anyhow!("tool `{name}` arguments are not valid JSON: {e}"))?
    };
    match name {
        "read_file" => read_file::execute(&args).await,
        "find_file" => find_file::execute(&args).await,
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
        let result = execute("read_file", "{not-json").await;
        assert!(result.is_err());
    }

    #[test]
    fn at_least_one_tool_registered() {
        assert!(!available_tools().is_empty());
    }
}

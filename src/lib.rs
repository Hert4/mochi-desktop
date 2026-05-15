// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

pub mod agent;
pub mod app;
pub mod chat_repl;
pub mod error;
pub mod logging;
pub mod memory;
pub mod memory_capture;
pub mod perf;
pub mod pet;
pub mod skills;
pub mod tools;
pub mod ui;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    Llamacpp,
}

#[derive(Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum DiagnosticsPreset {
    Runtime,
    Session,
    Render,
    Bridge,
    Full,
}

impl DiagnosticsPreset {
    #[must_use]
    pub fn filter_directives(&self) -> &'static str {
        match self {
            Self::Runtime => {
                "info,bridge.lifecycle=debug,bridge.protocol=debug,app.session=debug,app.tool=debug,app.command=debug,app.permission=debug,app.network=debug,app.update=debug"
            }
            Self::Session => {
                "info,bridge.lifecycle=debug,bridge.protocol=debug,app.session=debug,app.permission=debug,app.command=debug"
            }
            Self::Render => {
                "info,app.render=trace,app.cache=debug,app.input=debug,app.paste=debug,app.perf=info"
            }
            Self::Bridge => {
                "info,bridge.lifecycle=debug,bridge.protocol=debug,bridge.sdk=debug,bridge.permission=debug,bridge.mcp=debug"
            }
            Self::Full => {
                "info,app.render=trace,app.perf=info,bridge.lifecycle=debug,bridge.protocol=debug,bridge.sdk=debug,bridge.permission=debug,bridge.mcp=debug,app.session=debug,app.tool=debug,app.command=debug,app.permission=debug,app.network=debug,app.update=debug,app.cache=debug,app.input=debug,app.paste=debug,app.config=debug,app.auth=debug"
            }
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "mochi", version, about = "Terminal AI agent with a pixel-art pet companion, local-first via llama.cpp")]
#[command(
    after_help = "Examples:\n  mochi --provider llamacpp\n  mochi chat\n  mochi --provider llamacpp --pet bunny\n  mochi --enable-logs --diagnostics-preset session"
)]
#[allow(clippy::struct_excessive_bools)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Disable startup update checks.
    #[arg(long)]
    pub no_update_check: bool,

    /// Working directory (defaults to cwd)
    #[arg(long, short = 'C')]
    pub dir: Option<std::path::PathBuf>,

    /// Path to the agent bridge script (defaults to agent-sdk/dist/bridge.js).
    #[arg(long)]
    pub bridge_script: Option<std::path::PathBuf>,

    /// LLM provider backing the TUI. Default: llamacpp (local-first). Use `--provider anthropic` for the legacy CCR Claude Code bridge.
    #[arg(long, value_enum, default_value_t = Provider::Llamacpp)]
    pub provider: Provider,

    /// llama.cpp server base URL (used when --provider llamacpp).
    #[arg(long, default_value = "http://127.0.0.1:8765")]
    pub llama_url: String,

    /// Sampling temperature for llama provider.
    #[arg(long, default_value_t = 0.7_f32)]
    pub llama_temperature: f32,

    /// Pick which pet character to display (mochi=cat, bunny, frog, robot, dragon).
    #[arg(long, default_value = "mochi")]
    pub pet: String,

    /// Enable runtime diagnostics using a default log path when `--log-file` is omitted.
    #[arg(long)]
    pub enable_logs: bool,

    /// Named diagnostics preset for common logging workflows.
    /// Ignored when `--log-filter` is provided explicitly.
    #[arg(long, value_enum)]
    pub diagnostics_preset: Option<DiagnosticsPreset>,

    /// Write tracing diagnostics to a file.
    ///
    /// When omitted but logging is otherwise enabled via `--enable-logs`,
    /// `--diagnostics-preset`, `--log-filter`, `--log-append`, or `RUST_LOG`,
    /// a default log path is used.
    #[arg(long, value_name = "PATH")]
    pub log_file: Option<std::path::PathBuf>,

    /// Tracing filter directives (example: `info,app.render=trace`).
    /// Overrides `--diagnostics-preset` and falls back to `RUST_LOG` when omitted.
    #[arg(long, value_name = "FILTER")]
    pub log_filter: Option<String>,

    /// Append to the active log file instead of resetting the current log window on startup.
    #[arg(long)]
    pub log_append: bool,

    /// Enable perf telemetry using a default sidecar path when `--perf-log` is omitted.
    /// Requires a binary built with `--features perf`.
    #[arg(long)]
    pub enable_perf: bool,

    /// Write high-frequency perf telemetry to a sidecar JSON file (requires `--features perf` build).
    #[arg(long, value_name = "PATH")]
    pub perf_log: Option<std::path::PathBuf>,

    /// Append to `--perf-log` instead of truncating on startup.
    #[arg(long)]
    pub perf_append: bool,
}

#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum Command {
    /// Resume a previous session by ID, or pick from recent sessions
    Resume {
        /// Session ID to resume directly. Omit to show a session picker.
        session_id: Option<String>,
    },
    /// Simple REPL chat against a local llama.cpp server (no TUI, no Anthropic).
    Chat {
        /// llama.cpp server URL.
        #[arg(long, default_value = "http://127.0.0.1:8765")]
        url: String,
        /// System prompt to set Mochi's persona.
        #[arg(
            long,
            default_value = "You are Mochi, a cute, terse terminal pet companion. Reply in 1-3 sentences."
        )]
        system: String,
        /// Sampling temperature.
        #[arg(long, default_value_t = 0.7_f32)]
        temperature: f32,
    },
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use clap::Parser;

    #[test]
    fn cli_without_subcommand_starts_new_session() {
        let cli = Cli::try_parse_from(["claude-rs"]).expect("parse");
        assert!(cli.command.is_none());
    }

    #[test]
    fn cli_resume_without_id_requests_picker() {
        let cli = Cli::try_parse_from(["claude-rs", "resume"]).expect("parse");
        assert_eq!(cli.command, Some(Command::Resume { session_id: None }));
    }

    #[test]
    fn cli_resume_with_id_resumes_directly() {
        let cli = Cli::try_parse_from(["claude-rs", "resume", "abc-123"]).expect("parse");
        assert_eq!(cli.command, Some(Command::Resume { session_id: Some("abc-123".to_owned()) }));
    }

    #[test]
    fn cli_rejects_legacy_resume_flag() {
        assert!(Cli::try_parse_from(["claude-rs", "--resume", "abc-123"]).is_err());
    }
}

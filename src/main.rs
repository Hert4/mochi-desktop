// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use clap::Parser;
use claude_code_rust::Cli;
use claude_code_rust::error::AppError;
use std::time::Instant;
use tracing::info_span;

#[allow(clippy::exit)]
fn main() {
    if let Err(err) = run() {
        if let Some(app_error) = extract_app_error(&err) {
            eprintln!("{}", app_error.user_message());
            std::process::exit(app_error.exit_code());
        }
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let _logging = claude_code_rust::logging::LoggingRuntime::init(&cli)?;
    let perf_path = claude_code_rust::logging::resolve_perf_path(&cli)?;

    if let Some(claude_code_rust::Command::Chat { url, system, temperature }) = cli.command.clone()
    {
        let rt = tokio::runtime::Runtime::new()?;
        return rt.block_on(claude_code_rust::chat_repl::run(url, system, temperature));
    }

    #[cfg(not(feature = "perf"))]
    if perf_path.is_some() {
        return Err(anyhow::anyhow!(
            "perf telemetry requires a binary built with `--features perf`"
        ));
    }

    if cli.provider == claude_code_rust::Provider::Anthropic {
        let startup_bootstrap_span = info_span!(
            target: claude_code_rust::logging::targets::APP_LIFECYCLE,
            "startup_bootstrap",
            resume_requested = matches!(
                cli.command,
                Some(claude_code_rust::Command::Resume { .. })
            ),
            perf_telemetry_requested = perf_path.is_some(),
            explicit_bridge_script = cli.bridge_script.is_some(),
        );
        let _entered = startup_bootstrap_span.enter();
        let resolve_started = Instant::now();
        let bridge_launcher =
            claude_code_rust::agent::bridge::resolve_bridge_launcher(cli.bridge_script.as_deref())?;
        let duration_ms = u64::try_from(resolve_started.elapsed().as_millis()).unwrap_or(u64::MAX);
        tracing::info!(
            target: claude_code_rust::logging::targets::BRIDGE_LIFECYCLE,
            event_name = "bridge_launcher_resolved",
            message = "resolved agent bridge launcher",
            duration_ms,
            launcher = %bridge_launcher.describe(),
        );
    }

    let rt = tokio::runtime::Runtime::new()?;
    let local_set = tokio::task::LocalSet::new();

    rt.block_on(local_set.run_until(async move {
        let mut cli = cli;
        let managed_server = maybe_spawn_managed_llama_server(&cli).await?;
        if let Some(server) = managed_server.as_ref() {
            cli.llama_url = server.url.clone();
        }

        let mut app = claude_code_rust::app::create_app(&cli);
        if let Some(server) = managed_server {
            *app.managed_llama_server.borrow_mut() = Some(server);
        }

        claude_code_rust::app::start_service_status_check(&app);
        let result = claude_code_rust::app::run_tui(&mut app).await;
        maybe_print_resume_hint(&app, result.is_ok());

        claude_code_rust::agent::events::kill_all_terminals(&app.terminals);

        // Explicit graceful shutdown of managed llama-server (in addition to
        // the Drop kill that fires when the Rc drops — belt and braces).
        // Take ownership BEFORE awaiting so the RefCell borrow doesn't span the await
        // point (clippy::await_holding_refcell_ref).
        let owned_server = app.managed_llama_server.borrow_mut().take();
        if let Some(server) = owned_server {
            let _ = server.shutdown().await;
        }

        if let Some(app_error) = app.exit_error.take() {
            return Err(anyhow::Error::new(app_error));
        }

        result
    }))
}

async fn maybe_spawn_managed_llama_server(
    cli: &Cli,
) -> anyhow::Result<Option<claude_code_rust::llama_server::ManagedLlamaServer>> {
    if cli.provider != claude_code_rust::Provider::Llamacpp {
        return Ok(None);
    }
    let Some(model_path) = cli.llama_model.as_ref() else {
        return Ok(None);
    };
    eprintln!("[mochi] starting managed llama-server with {} ...", model_path.display());
    let mut config = claude_code_rust::llama_server::LlamaServerConfig::new(model_path.clone());
    config.context_size = cli.llama_context;
    let server = claude_code_rust::llama_server::ManagedLlamaServer::spawn(&config)?;
    eprintln!("[mochi] waiting for {} to load the model...", server.url);
    server.wait_for_ready(None).await?;
    eprintln!("[mochi] llama-server ready at {}", server.url);
    Ok(Some(server))
}

fn extract_app_error(err: &anyhow::Error) -> Option<AppError> {
    err.chain().find_map(|cause| cause.downcast_ref::<AppError>().cloned())
}

fn maybe_print_resume_hint(app: &claude_code_rust::app::App, success: bool) {
    if !success {
        return;
    }
    let Some(session_id) = app.session_id.as_ref() else {
        return;
    };
    eprintln!("Resume this session: mochi resume {session_id}");
}

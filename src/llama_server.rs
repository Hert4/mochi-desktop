//! Manage a child `llama-server` process tied to the Mochi session lifecycle.
//! When the [`ManagedLlamaServer`] is dropped, the child is sent SIGKILL so the
//! model unloads as soon as Mochi exits — even on panic.
//!
//! Designed for single-process Mochi runs. If you already have a long-running
//! llama-server, skip this and point `--llama-url` at it instead.

use std::path::PathBuf;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep};

const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(300);
const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone)]
pub struct LlamaServerConfig {
    pub binary: PathBuf,
    pub model: PathBuf,
    pub host: String,
    pub port: u16,
    pub context_size: u32,
    pub extra_args: Vec<String>,
}

impl LlamaServerConfig {
    #[must_use]
    pub fn new(model: PathBuf) -> Self {
        Self {
            binary: PathBuf::from("llama-server"),
            model,
            host: "127.0.0.1".to_owned(),
            port: 8765,
            context_size: 32768,
            extra_args: Vec::new(),
        }
    }

    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

pub struct ManagedLlamaServer {
    child: Option<Child>,
    pub url: String,
    pub model_path: PathBuf,
}

impl ManagedLlamaServer {
    pub fn spawn(config: &LlamaServerConfig) -> anyhow::Result<Self> {
        if !config.model.is_file() {
            anyhow::bail!(
                "llama model not found at {}. Pass a valid GGUF file.",
                config.model.display()
            );
        }
        let mut cmd = Command::new(&config.binary);
        cmd.arg("-m").arg(&config.model);
        cmd.arg("--host").arg(&config.host);
        cmd.arg("--port").arg(config.port.to_string());
        cmd.arg("-c").arg(config.context_size.to_string());
        for arg in &config.extra_args {
            cmd.arg(arg);
        }
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        cmd.kill_on_drop(true);

        let child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn `{}`: {e}", config.binary.display()))?;

        tracing::info!(
            target: crate::logging::targets::APP_LIFECYCLE,
            event_name = "llama_server_spawned",
            pid = child.id().unwrap_or(0),
            model = %config.model.display(),
            port = config.port,
            "spawned managed llama-server",
        );

        Ok(Self {
            child: Some(child),
            url: config.url(),
            model_path: config.model.clone(),
        })
    }

    pub async fn wait_for_ready(&self, timeout: Option<Duration>) -> anyhow::Result<()> {
        let deadline = Instant::now() + timeout.unwrap_or(DEFAULT_READY_TIMEOUT);
        // Poll /v1/models specifically: llama-server's /health returns "ok" as
        // soon as the HTTP server starts, but /v1/models only succeeds after
        // the model weights are fully loaded into memory. The previous /health
        // poll was racing — user prompts arrived during load and got 503.
        let probe = format!("{}/v1/models", self.url);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|e| anyhow::anyhow!("health client build: {e}"))?;
        loop {
            if let Ok(resp) = client.get(&probe).send().await {
                if resp.status().is_success() {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "llama-server at {} did not finish loading within {:?}",
                    self.url,
                    timeout.unwrap_or(DEFAULT_READY_TIMEOUT)
                );
            }
            sleep(HEALTH_POLL_INTERVAL).await;
        }
    }

    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        if let Some(mut child) = self.child.take() {
            tracing::info!(
                target: crate::logging::targets::APP_LIFECYCLE,
                event_name = "llama_server_shutdown_requested",
                pid = child.id().unwrap_or(0),
                "shutting down managed llama-server",
            );
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        Ok(())
    }
}

impl Drop for ManagedLlamaServer {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // best-effort synchronous kill on drop; kill_on_drop on the Command
            // is the safety net for panics that skip this path.
            let _ = child.start_kill();
        }
    }
}

/// Probe an existing llama-server's /health endpoint. Returns Ok(true) when ready.
pub async fn probe_health(url: &str) -> bool {
    let target = format!("{}/health", url.trim_end_matches('/'));
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(2)).build() else {
        return false;
    };
    if let Ok(resp) = client.get(&target).send().await {
        if resp.status().is_success() {
            if let Ok(body) = resp.text().await {
                return body.contains("\"status\":\"ok\"");
            }
        }
    }
    false
}

/// Look for `llama-server` on PATH. Returns Some(absolute) if found.
#[must_use]
pub fn discover_binary() -> Option<PathBuf> {
    which::which("llama-server").ok()
}

#[cfg(test)]
mod tests {
    use super::{LlamaServerConfig, discover_binary};
    use std::path::PathBuf;

    #[test]
    fn url_matches_host_and_port() {
        let cfg = LlamaServerConfig {
            host: "0.0.0.0".to_owned(),
            port: 12345,
            ..LlamaServerConfig::new(PathBuf::from("/tmp/m.gguf"))
        };
        assert_eq!(cfg.url(), "http://0.0.0.0:12345");
    }

    #[test]
    fn discovery_returns_none_or_existing_path() {
        match discover_binary() {
            Some(p) => assert!(p.exists()),
            None => {}
        }
    }
}


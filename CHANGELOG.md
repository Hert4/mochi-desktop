# Changelog

All notable changes to Mochi are documented in this file.

## v0.1.0 — initial fork-and-rebrand

First Mochi release. Forked from [claude-code-rust](https://github.com/srothgan/claude-code-rust) v0.10 at commit-time, Apache-2.0.

### Added

- **`--provider {anthropic, llamacpp}`** CLI flag (`src/lib.rs`). Default: `anthropic` for parity with upstream; switch to `llamacpp` for a fully local session.
- **Llama.cpp HTTP+SSE client** (`src/agent/llama_client.rs`) — OpenAI-compatible `/v1/chat/completions` streaming.
- **Llama lifecycle runner** (`src/app/connect/llama_lifecycle.rs`) — synthesizes the BridgeEvents the inherited TUI consumes so the Ratatui chat view drives a local llama-server with zero Anthropic-SDK dependency.
- **`mochi chat` REPL subcommand** — line-buffered fallback that bypasses the TUI; useful when raw-mode IME issues block the full TUI.
- **Skills system** (`src/skills.rs`) — load Markdown `SKILL.md` files from `~/.mochi/skills/<name>/SKILL.md`, activate via `/skill use NAME` mid-session. Two bundled skills: `grumpy-cat` and `code-coach`.
- **Memory layer** (`src/memory.rs`) — SQLite at `~/.mochi/memory/memory.db` with 4-kind schema (profile / concept / state / behavioral). Behavioral facts are scoped per active skill so different personas have distinct preferences.
- **Auto-memory capture** (`src/memory_capture.rs`) — background LLM extraction call after each user turn writes durable facts and triggers a system-prompt rebuild via an internal runtime channel. Memory updates are visible the next turn without restart.
- **Pet character roster** (`src/pet.rs`) — `mochi`, `bunny`, `frog`, `robot`, `dragon`, each with 5 mood sprites.
- Slash commands `/memory`, `/skill`, `/pet` registered in the TUI (`src/app/slash/executors.rs`).
- Bundled example skills under `skills/`.

### Changed

- Welcome banner now a Mochi cat with rebranded greeting and Mochi-flavored tips (`src/ui/message.rs`).
- Assistant role label rendered as `Mochi` (`src/ui/message.rs`).
- OS-level notifications use `Mochi` instead of `Claude Code` (`src/app/notify.rs`).
- Trust dialog body text rebranded (`src/ui/trusted.rs`).
- Welcome screen trimmed: removed `Subscription` and `Session ID` rows.
- Cargo package + binary name → `mochi` (`Cargo.toml`).
- Default `--llama-url` is `http://127.0.0.1:8765` (avoids port 8080 conflict with common dev services).
- Paste-burst detector passes through non-ASCII characters immediately (`src/app/paste_burst.rs`) so CJK / IME composition output isn't swallowed.

### Removed

- Startup update check that pointed at the upstream `claude-code-rust` npm package.

### Known limitations

- macOS Vietnamese-Telex (and similar OS-level IMEs) does not engage inside raw-mode terminals. Install [EVKey](https://evkeyvn.com/) for full Vietnamese input in the TUI. `mochi chat` (REPL) is unaffected.
- No in-flight cancellation for llama.cpp prompts.
- MCP, plugins, model picker, mode switcher in the TUI are Anthropic-only — they show empty / no-op when `--provider llamacpp`.
- `--pet` selection currently affects the inline REPL pet only; the TUI welcome banner is still the Mochi cat.

# Mochi

A local-first terminal pet companion that remembers you. Powered by [llama.cpp](https://github.com/ggerganov/llama.cpp). Rust + Ratatui TUI.

[![CI](https://github.com/Hert4/mochi/actions/workflows/ci.yml/badge.svg)](https://github.com/Hert4/mochi/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/rust-1.89%2B-orange?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](./LICENSE)
[![Version](https://img.shields.io/badge/version-0.1.0-purple)](./CHANGELOG.md)
[![Status](https://img.shields.io/badge/status-alpha-yellow)](#known-limitations-v01)
[![llama.cpp](https://img.shields.io/badge/llama.cpp-compatible-success)](https://github.com/ggerganov/llama.cpp)
[![Ratatui](https://img.shields.io/badge/TUI-Ratatui-blueviolet)](https://github.com/ratatui-org/ratatui)
[![Local-first](https://img.shields.io/badge/local--first-yes-brightgreen)](#what-it-is)
[![Stars](https://img.shields.io/github/stars/Hert4/mochi?style=social)](https://github.com/Hert4/mochi)

```
   ____________________________
  <  Mochi is here ~ nya~ hi!  >
   ‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾
         \
          \      ／l、
            ﾞ（=^･ｪ･^=）
              l、ﾞ ~ヽ
              じしf_, )ノ
```

## What it is

- **Local LLM by default.** Point Mochi at your llama.cpp server — no API key, no cloud round-trips, no Anthropic auth.
- **Persona via Skills.** Drop a Markdown `SKILL.md` under `~/.mochi/skills/<name>/` to give Mochi a character (grumpy cat, code coach, whatever).
- **Long-term memory.** Mochi auto-captures durable user facts during chat (name, location, preferences) and persists them across sessions via SQLite. Schema follows the [BOOKMARKS paper](https://arxiv.org/abs/2605.14169)'s 4-kind structure: profile / concept / state / behavioral.
- **Pet character roster.** Multiple ASCII characters (mochi cat, bunny, frog, robot, dragon) selectable via `--pet`.
- **Forked from [claude-code-rust](https://github.com/srothgan/claude-code-rust)** under Apache-2.0 — kept the TUI bones, swapped the LLM backend.

## Install

Requires Rust 1.89+ and a running [llama.cpp server](https://github.com/ggerganov/llama.cpp).

```bash
git clone <your-fork-url>/mochi
cd mochi
cargo install --path .
```

## Run

Start a llama.cpp server (any GGUF model):

```bash
llama-server -m /path/to/model.gguf --port 8765 -c 4096
```

> Mochi defaults to port 8765 to avoid the common 8080 conflict. Override with `--llama-url`.

Then launch Mochi:

```bash
# Full Ratatui TUI
mochi --provider llamacpp

# Pick a different pet
mochi --provider llamacpp --pet bunny

# Quick REPL mode (no TUI)
mochi chat
```

## Slash commands

Inside the TUI (or `mochi chat`):

| Command | Purpose |
|---------|---------|
| `/help` | Show all commands |
| `/memory list [KIND]` | Show stored facts (profile, concept, state, behavioral) |
| `/memory remember KIND SLUG CONTENT` | Manually add a fact |
| `/memory forget SLUG` | Remove a fact |
| `/memory profile [TEXT]` | View or set your one-line user bio |
| `/skill list` | List installed skills |
| `/skill use NAME` | Activate a skill mid-conversation (Mochi adopts its persona immediately) |
| `/skill off` | Drop the active skill |
| `/pet list` / `/pet show NAME` | Pet character roster |

## Skills

A skill is a Markdown file with YAML frontmatter. Two examples ship in `skills/`:

```markdown
---
name: grumpy-cat
description: Respond like a perpetually annoyed cat.
---

You are no longer Mochi. You are a grumpy cat who tolerates the user only because they feed you.

Rules:
- Reply in 1-3 sentences max.
- Be sarcastic and mildly insulting, never cruel.
...
```

Install bundled skills once:

```bash
mkdir -p ~/.mochi/skills && cp -r skills/* ~/.mochi/skills/
```

## Memory

Mochi runs a small background LLM extraction call after every user turn. Detected durable facts are written to `~/.mochi/memory/memory.db` and surfaced in the next system prompt as authoritative facts the assistant must honor.

Memory is layered:

- **profile** — user identity (name, role)
- **concept** — entities the user mentioned (city, language, project)
- **state** — current task or focus
- **behavioral** — preferences and communication patterns, scoped per active skill

You can always inspect and override via `/memory list` and `/memory remember`.

## Known limitations (v0.1)

- **Vietnamese / CJK / IME input in TUI mode.** macOS Terminal-style raw-mode bypasses OS-level Input Methods. Affects most Rust TUIs (helix, zellij, lapce-terminal). Install [EVKey](https://evkeyvn.com/) (macOS) or equivalent OS-level Vietnamese IME — it composes before keystrokes reach the terminal and works universally. REPL mode (`mochi chat`) reads from stdin and respects OS IME naturally.
- **No in-flight cancellation** for llama.cpp prompts in v0.1.
- **`--pet` flag** changes default pet but the welcome banner still shows the Mochi cat (wired to REPL renderer only). Banner-per-character is follow-up.
- **MCP / plugins / mode picker** in TUI are Anthropic-only — they appear empty or no-op when `--provider llamacpp`.
- **Recommended model**: any instruction-tuned 7B+ GGUF. Heavily RP-tuned 3-4B models may ignore the system prompt and drift away from stored facts.

## Architecture

```
┌────────────────────────────────────────┐
│  Ratatui TUI (chat view, slash, input) │
│         (inherited from CCR)           │
└──────────┬─────────────────────────────┘
           │  CommandEnvelope (mpsc)
           ▼
   ┌───────────────────────┐
   │ provider dispatch     │
   ├───────────┬───────────┤
   │ Anthropic │ Llamacpp  │  ← --provider flag
   └───────────┴─────┬─────┘
                    ▼
        ┌─────────────────────┐
        │ run_llama_task      │
        │ (synthetic bridge   │
        │  events + HTTP+SSE  │
        │  to llama.cpp)      │
        └──────┬───────┬──────┘
               │       │
               │       └─ background memory_capture (each turn)
               │
               ├─ stream_chat → /v1/chat/completions
               │
               └─ slash side-channel: rebuild system prompt
                  on /memory and /skill activity
```

## License

Apache-2.0. Forked from [claude-code-rust](https://github.com/srothgan/claude-code-rust) (Simon Peter Rothgang, Apache-2.0). See `LICENSE` for the full text.

This project is not affiliated with Anthropic or the original `claude-code-rust` author.

## Inspiration & references

### Papers / research

- **BOOKMARKS — Efficient Active Storyline Memory for Role-playing** (Koishi's Day 2026, [arxiv 2605.14169](https://arxiv.org/abs/2605.14169))
  → Adopted: 4-kind memory schema (profile / concept / state / behavioral), per-character behavioral scoping
  → Deferred: per-turn LLM query proposal, reuse/derive judge, recursive state update (see [ROADMAP.md](./ROADMAP.md) Sprint 11)
- **Anthropic Agent Skills** ([spec](https://docs.anthropic.com/en/docs/build-with-claude/agent-skills))
  → Adopted: Markdown `SKILL.md` with YAML frontmatter; load-on-activate, progressive context
- **OpenAI function-calling spec** ([reference](https://platform.openai.com/docs/guides/function-calling))
  → Adopted: tool schema, `tool_choice: "auto"`, streaming `delta.tool_calls[]` accumulation

### Projects we stood on / learned from

| Project | What we took |
|---|---|
| [claude-code-rust](https://github.com/srothgan/claude-code-rust) (Simon Peter Rothgang, Apache-2.0) | The whole TUI base — chat view, slash dispatcher, input area, layout, syntax highlight, permission UI, theme. Mochi is a fork. |
| [DeerFlow](https://github.com/bytedance/deer-flow) (ByteDance, MIT) | Skills system architecture (Markdown SKILL.md, load-on-demand), sub-agent vision (planned Sprint 9), sandbox abstraction (planned) |
| [Anthropic Claude Code](https://docs.anthropic.com/en/docs/claude-code) | Memory pattern — SQLite + Markdown facts file mirror; per-call permission UX; SDK tool naming (`Read`/`Write`/`Bash`/`Glob`/`WebFetch`) |
| [llama.cpp](https://github.com/ggerganov/llama.cpp) (ggerganov, MIT) | Local inference + OpenAI-compatible HTTP server (`/v1/chat/completions`) — Mochi's default backend |

### Techniques implemented

- **Synthetic BridgeEvent emission** — Mochi's llama runner fakes the same `Connected` / `SessionUpdate` / `TurnComplete` events Anthropic's Node bridge sends, so CCR's TUI renders local llama output without a Node dependency.
- **Side-channel runtime control** — `LlamaRuntimeCommand` mpsc lets `/memory` and `/skill` slash handlers trigger live system-prompt rebuilds inside the running llama task without restart.
- **Background memory capture** — after each user turn, a `tokio::task::spawn_local` runs an extraction prompt (`KIND|SLUG|CONTENT` format, 0.0 temperature) and writes durable facts; results arrive in time for the next prompt.
- **Tool-call loop with permission gating** — max 6 iterations per user prompt; per-tool `needs_permission` flag; per-session `allow_set` so "Allow for session" doesn't re-prompt the same tool.
- **DDG HTML scraping for `WebSearch`** — no API key, parses `uddg=` redirect wrappers back to clean URLs, returns `title | url | snippet` per result.
- **Hand-rolled HTML→text stripper for `WebFetch`** — drops `<script>`/`<style>`/comments, decodes common entities, collapses whitespace. Good enough for an LLM to read article-like pages.

### Tech stack

| Layer | Crate / tool |
|---|---|
| Async runtime | [`tokio`](https://tokio.rs/) + `LocalSet` for `!Send` UI state |
| Terminal UI | [`ratatui`](https://github.com/ratatui-org/ratatui) + [`crossterm`](https://github.com/crossterm-rs/crossterm) |
| HTTP / SSE | [`reqwest`](https://github.com/seanmonstar/reqwest) + [`eventsource-stream`](https://github.com/jpopesculian/eventsource-stream) + [`async-stream`](https://github.com/tokio-rs/async-stream) |
| LLM backend | llama.cpp HTTP server (OpenAI-compatible) |
| Memory store | [`rusqlite`](https://github.com/rusqlite/rusqlite) (bundled SQLite) |
| Filesystem walk / glob | [`ignore`](https://github.com/BurntSushi/ripgrep/tree/master/crates/ignore) + [`globset`](https://github.com/BurntSushi/ripgrep/tree/master/crates/globset) (both BurntSushi) |
| Markdown render | [`pulldown_cmark`](https://github.com/raphlinus/pulldown-cmark) + [`tui-markdown`](https://github.com/joshka/tui-markdown) |
| Syntax highlight | [`syntect`](https://github.com/trishume/syntect) |
| CLI | [`clap`](https://github.com/clap-rs/clap) |
| Logging | [`tracing`](https://github.com/tokio-rs/tracing) + JSON appender |

### Conventions followed

- **YAML frontmatter** for `SKILL.md` files — hand-rolled minimal parser (key-value, no full YAML; quoted-string support; `---` delimiters).
- **PascalCase tool names** — `Read`, `Write`, `Bash`, `Glob`, `WebFetch`, `WebSearch` — matching Anthropic SDK so CCR's `tool_name_label` dispatches to the right icon without a mapper shim.
- **Anthropic-SDK arg names** — `file_path`, `command`, `pattern`, `url`, `query` — same wire schema as Claude Code, so swapping providers later is a 1-line change.
- **3-option permission menu** — `allow_once` / `allow_session` / `reject_once`, matching CCR's `PermissionOptionKind` enum.

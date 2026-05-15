# Mochi

A terminal AI agent with a pixel-art pet companion. Local-first via [llama.cpp](https://github.com/ggerganov/llama.cpp). Rust + Ratatui TUI.

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

## Credits

- **claude-code-rust** by Simon Peter Rothgang — the TUI base
- **BOOKMARKS** (Koishi's Day 2026, [arxiv 2605.14169](https://arxiv.org/abs/2605.14169)) — 4-kind memory schema
- **Anthropic Agent Skills** spec — Markdown `SKILL.md` pattern
- **llama.cpp** — the local inference workhorse

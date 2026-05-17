# Mochi — Usage guide

Complete reference for every feature shipped in Mochi v0.1.x. For the elevator pitch, see [`README.md`](./README.md). For what's next, see [`ROADMAP.md`](./ROADMAP.md).

## Table of contents

- [Setup](#setup)
- [Starting Mochi](#starting-mochi)
- [Slash command reference](#slash-command-reference)
- [Tools](#tools)
- [Memory model](#memory-model)
- [Skills](#skills)
- [Pet companion](#pet-companion)
- [Workflows / recipes](#workflows--recipes)
- [Configuration files](#configuration-files)
- [Troubleshooting](#troubleshooting)

---

## Setup

Requirements:

- macOS or Linux (Windows untested in v0.1)
- Rust 1.89+
- [`llama.cpp`](https://github.com/ggerganov/llama.cpp) — needs the `llama-server` binary on `PATH` (e.g. `brew install llama.cpp`)
- A GGUF model. Tested with Qwen3-based models (Luna-SRSA, Qwen3-Instruct). Models < 7B with strong instruction-following work; pure RP finetunes may need prompt tuning.

Install:

```bash
git clone <your-fork-url>/mochi
cd mochi
cargo install --path .
```

Bundled skills (optional one-time copy):

```bash
mkdir -p ~/.mochi/skills
cp -r skills/* ~/.mochi/skills/
```

Mochi creates the rest of its state under `~/.mochi/` on first run (`memory/memory.db`, etc).

---

## Starting Mochi

Three ways, pick whichever fits your workflow.

### 1. One-line: Mochi manages the llama-server child process

```bash
mochi --provider llamacpp --llama-model /path/to/model.gguf
```

What happens:
1. Mochi spawns `llama-server -m <model> --port 8765 -c 32768` as a child process.
2. Mochi polls `/v1/models` until the weights are fully loaded (10-30s for 4-7B Q4-Q8).
3. The TUI opens.
4. When you `/quit` (or Ctrl+C, or panic) → the child is SIGKILLed automatically. No orphan processes.

Optional flags:

| Flag | Default | Notes |
|------|---------|-------|
| `--llama-context N` | `32768` | Context window. Bump for long web fetches. |
| `--llama-temperature F` | `0.7` | Sampling temperature for main chat. Memory judge / extraction always use their own (0.0–0.2). |
| `--pet NAME` | `mochi` | `mochi`, `bunny`, `frog`, `robot`, `dragon`. |

### 2. Inline: launch Mochi first, configure via slash

```bash
mochi          # default --provider llamacpp, no model loaded yet
```

In the TUI:

```text
> /provider llamacpp /path/to/model.gguf
[Info] switching to llamacpp model ... (model load may take 10-30s)
```

Same Drop-on-exit semantics. Lets you swap models mid-session without restarting Mochi.

### 3. External: connect to an already-running llama-server

```bash
# In one terminal:
llama-server -m /path/to/model.gguf --port 8765 -c 32768

# In another:
mochi --provider llamacpp --llama-url http://127.0.0.1:8765
```

Mochi will NOT kill this server on exit. Useful when you want multiple clients sharing one llama-server.

### Default provider

`mochi` (no flags) defaults to `--provider llamacpp` (local-first). The legacy CCR Anthropic bridge is still wired — pass `--provider anthropic` to use it (requires Claude Code login).

### REPL fallback

```bash
mochi chat
```

Line-buffered REPL that bypasses the Ratatui TUI entirely. Useful when IME issues (see [Troubleshooting](#troubleshooting)) block the TUI, or when you want a smaller surface for scripting.

---

## Slash command reference

All slash commands work inside the TUI (`mochi --provider llamacpp`) once you've sent at least one message or activated a connection. Autocomplete: type `/` to see candidates.

### General

| Command | Effect |
|---------|--------|
| `/help` | List all slash commands and shortcuts. |
| `/clear` | Clear the chat view. Memory, active skill, and session are preserved. |
| `/reset` | (REPL only) Clear conversation history but keep system+skill. |
| `/quit`, `/exit` | (REPL only) Exit Mochi cleanly. In the TUI, use Ctrl+C / Ctrl+Q. |
| `/status` | Inherited from CCR — current session status. |

### Provider & lifecycle

| Command | Effect |
|---------|--------|
| `/provider` | Show current provider, llama URL, and managed model path if any. Suggests `/provider llamacpp <PATH>` if no model is loaded. |
| `/provider llamacpp <PATH>` | Spawn (or replace) the managed llama-server with `<PATH>`. Old child is killed and its port freed before the new one binds. Polls `/v1/models` until ready. |
| `/provider anthropic` | Rejected mid-session — restart with `mochi --provider anthropic`. |

### Pet

| Command | Effect |
|---------|--------|
| `/pet` / `/pet list` | List installed pet characters. |
| `/pet show NAME` | Print the sprite frames for a character. |

The pet rendered in the TUI corner reacts to chat state automatically: Idle / Thinking / Happy / Sad / Sleeping.

### Skills

| Command | Effect |
|---------|--------|
| `/skill list` | List all SKILL.md files installed under `~/.mochi/skills/`. |
| `/skill show NAME` | Print the full skill body. |
| `/skill use NAME` | Activate a skill. Its body is injected into the system prompt at the next prompt. |
| `/skill off` | Drop the active skill. |

### Memory

| Command | Effect |
|---------|--------|
| `/memory list [KIND]` | List stored facts. KIND ∈ `profile`, `concept`, `state`, `behavioral`. |
| `/memory remember KIND SLUG CONTENT` | Manually add a fact. Bypasses the dedup judge (you specified the slug). |
| `/memory forget SLUG` | Remove every fact with that slug. |
| `/memory profile [TEXT]` | View or set the one-paragraph user profile blurb. |
| `/memory consolidate` | LLM rewrites all stored facts into a ~200-word narrative profile (replaces the existing `profile/user`). |
| `/memory query <text>` | Manual debug: LLM proposes search queries for a hypothetical scene, then matches against stored facts. Shows the proposal + matched facts. |
| `/memory mode active\|all` | Switch memory injection strategy. `all` (default) dumps every fact each session. `active` runs the query proposer before every prompt and injects only matched facts (+ profile). |
| `/memory restate <slug>` | LLM rescans recent user messages and rewrites a state fact's answer. Useful when the user has moved on from the old state. |
| `/memory observe <behavioral query>` | LLM scans recent user messages, summarizes the user's pattern matching the query, stores as a behavioral fact (scoped to `default`). |

---

## Tools

Mochi advertises 8 tools to the model. The model decides when to call them based on the user's prompt; you don't invoke them directly.

| Tool | Args | Permission | Notes |
|------|------|------------|-------|
| `Read` | `file_path`, `offset?`, `limit?` | none | Returns up to 200KB. `~` expands to home. Use `offset`/`limit` for line windows. |
| `Glob` | `pattern`, `path?` | none | Bare names like `Cargo.toml` are auto-expanded to `**/Cargo.toml`. Respects `.gitignore`. Returns up to 50 matches. |
| `WebSearch` | `query`, `count?` | none | DuckDuckGo HTML scraper. No API key. Returns `title | url | snippet` lines. |
| `WebFetch` | `url` | none | HTML responses are stripped to text. 32KB output cap. 20s timeout. |
| `Bash` | `command`, `timeout?` | per-call | Run via `bash -lc`. 30s default timeout (max 5 min). Output capped 100KB. |
| `Write` | `file_path`, `content` | per-call | Auto-creates parent dirs. Emits `ToolCallContent::Diff` so you see the diff before approving. |
| `Edit` | `file_path`, `old_string`, `new_string`, `replace_all?` | per-call | Unique-match by default. Refuses empty `old_string` and identical-string no-ops. |
| `MultiEdit` | `file_path`, `edits[]` | per-call | Sequential edits applied in memory; rollback the whole batch on any failure. |

### Permission flow

When the model calls a permission-gated tool (Bash / Write / Edit / MultiEdit), the TUI shows an inline overlay:

```
⫦ Bash {"command": "ls"}     ← in_progress / pending
  Mochi wants to run: ls
  ❯ Allow once (Ctrl+y)
    Allow Bash for this session (Ctrl+a)
    Reject (Ctrl+n)
```

- **Allow once** — execute this call only. Re-prompt for the next Bash call.
- **Allow Bash for this session** — add Bash to the session allow-set. No more prompts for Bash until you restart Mochi.
- **Reject** — feed "User denied permission for this tool call." back to the model as the tool result.

The allow-set is per-tool-name and per-session. There's no granular `Bash:rm` vs `Bash:ls` filtering yet.

---

## Memory model

Mochi's memory is organized into 4 kinds, adapted from the [BOOKMARKS paper](https://arxiv.org/abs/2605.14169).

| Kind | What goes here | Examples |
|------|----------------|----------|
| `profile` | Stable user identity | name, age, profession |
| `concept` | Attributes about the user | location, languages, employer, hardware |
| `state` | Current persistent project/focus | "building Mochi v0.1.7" |
| `behavioral` | Communication preferences | "prefers terse replies", "mixes Vietnamese + English" |

### Auto-capture pipeline

After every user message:

1. **Extract** — a background LLM call extracts durable facts as `KIND|SLUG|CONTENT` lines. Strict prompt: only emit facts that would still describe the user next month.
2. **Judge** — each extracted fact is compared against existing same-kind facts. The LLM decides `reuse` (overwrite same slot), `derive` (different facet, keep both), or `new`.
3. **Upsert** — the fact lands in `~/.mochi/memory/memory.db` with the slug from the judge's decision.

Cost: 1 extraction LLM call + 1 judge LLM call per extracted fact. All background — never blocks chat.

### Behavioral scoping

Behavioral facts have a `skill_scope` column. When you activate a skill via `/skill use grumpy-cat`, new behavioral facts get `scope=grumpy-cat`. When you `/skill off`, they go to `scope=default`. The renderer filters behavioral facts to the active scope, so each persona has its own behavioral memory.

### Active vs all mode

By default, every memory fact is injected into the system prompt at session start (`mode=all`). Once memory exceeds ~20 facts, this becomes context bloat. Switch with:

```text
> /memory mode active
```

In active mode, before every user prompt:

1. LLM proposes up to 3 search queries (`profile|user name`, `concept|user location`, etc.)
2. Mochi token-matches the queries against stored facts.
3. Only matched facts (+ all profile facts, which are small and always relevant) are injected.

Cost: 1 extra LLM call per turn (~1-2s on local 7B). Trade-off: lean prompt as memory grows.

---

## Skills

A skill is a Markdown file with YAML frontmatter, stored at `~/.mochi/skills/<name>/SKILL.md`.

### Minimal example

```markdown
---
name: grumpy-cat
description: Respond like a perpetually annoyed cat.
version: 0.1.0
---

You are no longer Mochi. You are a grumpy cat.

- Reply in 1-3 sentences max.
- Be sarcastic and mildly insulting, never cruel.
- Sigh audibly in writing.
- End each reply with "🐾".
```

### Activation

```text
> /skill list
    grumpy-cat           Respond like a perpetually annoyed cat
    code-coach           Senior engineering coach

> /skill use grumpy-cat
  active skill: grumpy-cat
```

The skill body is appended to the system prompt at the next user message. Toggle off with `/skill off`.

### Authoring tips

- **Be directive.** The skill body REPLACES Mochi's default persona while active. Use imperative voice ("You are X. Reply in Y.").
- **Stay terse.** Keep skills under ~30 lines. Long skills eat the context window.
- **One skill = one persona.** Don't try to combine code-coach + grumpy-cat into one skill — activate one at a time.
- **Frontmatter required fields:** `name` (must match the directory name), `description` (one-line summary shown in `/skill list`). `version` is optional.

---

## Pet companion

The pet sprite in the TUI corner reflects Mochi's current state:

| Mood | When |
|------|------|
| `Idle` | Default, no activity. |
| `Thinking` | You sent a prompt; LLM is responding. |
| `Happy` | Turn completed successfully. |
| `Sad` | Turn errored or was cancelled. |
| `Sleeping` | (Reserved for inactivity timeout — not yet auto-triggered.) |

Pick a character with `--pet bunny|frog|robot|dragon|mochi`. Each has 5 mood sprites.

---

## Workflows / recipes

### Read + edit a file

```text
> read Cargo.toml and add `serde_yml = "0.0.12"` after the `serde` line

✓ ⫚ Read /Users/.../Cargo.toml
✓ ⫦ Edit /Users/.../Cargo.toml
  [Permission overlay with inline diff]
  - serde = "1.0.228"
  + serde = "1.0.228"
  + serde_yml = "0.0.12"
  ❯ Allow once
```

### Web research with citation

```text
> what is the BOOKMARKS paper about? search the web and summarize

✓ ⊕ WebSearch {"query": "BOOKMARKS paper LLM memory role-playing"}
✓ ⊕ WebFetch {"url": "https://arxiv.org/abs/2605.14169"}

The BOOKMARKS paper (Koishi's Day 2026, arxiv 2605.14169) proposes ...
```

### Set up a persona then chat

```text
> /skill use grumpy-cat
  active skill: grumpy-cat
> how's your day going?
mochi: *sigh* Tolerable, I suppose. What do you want, human? 🐾
```

### Lean-mode for long-running sessions

```text
> /memory mode active
  memory mode: active (LLM-driven per-turn query proposal)

> what's my profession again?
[before-prompt: LLM proposes `profile|user profession`, matches profile/profession fact, injects only that]
mochi: AI engineer.
```

### Capture + verify

```text
> hi, I'm Duc, AI engineer in Hanoi, code Rust and Python
[wait ~3s — capture + judge + dedup]

> /memory list
[profile  ] name        Duc
[profile  ] profession  AI engineer
[concept  ] location    Hanoi
[concept  ] language    Rust, Python
```

### Provider swap mid-session

```text
> /provider llamacpp /Users/me/gguf/qwen3-7b-instruct.Q5_K_M.gguf
[Info] switching to llamacpp model ... (model load may take 10-30s)

[managed child for Luna gets SIGKILLed, new managed child spawns, /v1/models polls until ready]

> /provider
provider: llamacpp
llama_url: http://127.0.0.1:8765
managed model: /Users/me/gguf/qwen3-7b-instruct.Q5_K_M.gguf
```

---

## Configuration files

| Path | Purpose |
|------|---------|
| `~/.mochi/memory/memory.db` | SQLite store for facts. Schema: `facts(kind, slug, content, skill_scope, created_at, last_used)`. |
| `~/.mochi/skills/<name>/SKILL.md` | Markdown skills with YAML frontmatter. |
| `~/.claude/` | Inherited from CCR — Anthropic mode auth, sessions, plans. Untouched by `--provider llamacpp`. |

CLI flags are listed in `mochi --help`. There is no `~/.mochi/config.toml` yet — all config is CLI flag + slash command.

---

## Troubleshooting

### Vietnamese / CJK input doesn't work in the TUI

Symptom: typing Vietnamese-Telex (`u + w` → `ư`) only produces the single-key shortcut chars; multi-key composition (`aa → â`, `dd → đ`, tone marks `s/f/r/x/j`) fails.

Cause: macOS Vietnamese-Telex IME does NOT engage inside raw-mode terminals. Affects most Rust TUIs (helix, zellij, lapce-terminal).

Fix: install [EVKey](https://evkeyvn.com/) (macOS) or equivalent OS-level Vietnamese IME. EVKey hooks at the Accessibility API level and composes BEFORE the terminal receives the keystroke. Works universally.

Workaround: `mochi chat` (REPL mode) reads from stdin and respects OS-level IME normally.

### `Turn failed: llama returned 503 Service Unavailable`

Cause: you sent a prompt while a freshly-spawned llama-server was still loading model weights. The default `/health` endpoint returns OK before the model is queryable.

Fix: should not happen in v0.1.6+ — Mochi's `wait_for_ready` now polls `/v1/models` (only ready when actually queryable), and the chat path retries 503-with-"loading" up to 8× × 2s. If you see this anyway, the model load is taking longer than 16s — increase `--llama-context` may help (lighter mem footprint) or wait + retry.

### `Input disabled after an error. Press Ctrl+Q to quit and try again.`

Cause: an unexpected TurnError (network, non-503 HTTP error, JSON decode failure). The TUI puts the app into `AppStatus::Error` to prevent further damage.

Fix: Ctrl+Q to quit, then restart Mochi. Check `/tmp/mochi.log` if you ran with `--enable-logs`.

### Memory bloat with topical noise

Symptom: `/memory list` shows facts like `concept|game = Honkai Star Rail`, `concept|interest = cats`, `state|focus = acknowledging greeting`. These are session-specific topics the LLM hallucinated as durable user facts.

Fix:
1. Clean: `/memory forget <slug>` for each noisy entry.
2. Test the new strict capture prompt by clearing and gradually adding facts: `> tao tên Duc, sống Hà Nội, code Rust` should produce only `profile/name`, `concept/location`, `concept/language`.
3. If a 3-4B model still hallucinates, swap to a stronger instruction-tuned model: `Qwen3-7B-Instruct` (Q5_K_M ~5 GB), `Hermes-3-Llama-3.1-8B`, etc.

### Port 8765 already in use

Cause: an external llama-server (or another Mochi instance) is bound to 8765.

Fix: kill the process (`pkill -f llama-server`) OR pass `--llama-url http://127.0.0.1:8766` and start llama-server on 8766 separately.

Note: Mochi avoids port 8080 deliberately — it's a common conflict with other dev servers.

### `mochi chat` works but `mochi` (TUI) doesn't

Likely cause: terminal incompatibility. The TUI requires a real terminal that supports raw mode + alternate screen (most terminals do, but some IDE-embedded terminals don't).

Try: a standalone Terminal.app, iTerm2, kitty, foot, wezterm, or ghostty.

### Tool icon shows generic ○ instead of specific (⫚ Read, ⫦ Bash, etc.)

Cause: a custom tool name not in CCR's `tool_name_label` allowlist (PascalCase: `Read`, `Write`, `Bash`, `Glob`, `WebFetch`, `WebSearch`, `Edit`, `MultiEdit`).

Fix: Mochi v0.1.6+ uses Anthropic SDK conventions, so this shouldn't happen for built-in tools. If you add a new tool, name it PascalCase to match the renderer.

---

## See also

- [`README.md`](./README.md) — Elevator pitch + install
- [`ROADMAP.md`](./ROADMAP.md) — What's next
- [`CHANGELOG.md`](./CHANGELOG.md) — Release notes
- [BOOKMARKS paper](https://arxiv.org/abs/2605.14169) — memory schema inspiration
- [claude-code-rust](https://github.com/srothgan/claude-code-rust) — TUI base

# Mochi

A local-first terminal pet companion that remembers you. Powered by [llama.cpp](https://github.com/ggerganov/llama.cpp). Rust + Ratatui TUI.

[![CI](https://github.com/Hert4/mochi/actions/workflows/ci.yml/badge.svg)](https://github.com/Hert4/mochi/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/rust-1.89%2B-orange?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](./LICENSE)
[![Version](https://img.shields.io/badge/version-0.1.0-purple)](./CHANGELOG.md)
[![Status](https://img.shields.io/badge/status-alpha-yellow)](#known-limitations-v01)
[![Tests](https://img.shields.io/badge/tests-1401%20passing-brightgreen)](#research-techniques-implemented)
[![llama.cpp](https://img.shields.io/badge/llama.cpp-compatible-success)](https://github.com/ggerganov/llama.cpp)
[![Ratatui](https://img.shields.io/badge/TUI-Ratatui-blueviolet)](https://github.com/ratatui-org/ratatui)
[![Local-first](https://img.shields.io/badge/local--first-yes-brightgreen)](#what-it-is)
[![BOOKMARKS](https://img.shields.io/badge/paper-BOOKMARKS-9cf?logo=arxiv&logoColor=white)](https://arxiv.org/abs/2605.14169)
[![STALE](https://img.shields.io/badge/paper-STALE%20adjudication-9cf?logo=arxiv&logoColor=white)](https://arxiv.org/abs/2605.06527)
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
- **Long-term memory with staleness awareness.** Mochi auto-captures durable user facts during chat (name, location, preferences) and persists them across sessions via SQLite. Schema follows the [BOOKMARKS paper](https://arxiv.org/abs/2605.14169)'s 4-kind structure (profile / concept / state / behavioral). Write-side adjudication based on the [STALE paper](https://arxiv.org/abs/2605.06527) (Chao et al., arXiv 2605.06527) detects when a new observation invalidates an older belief (e.g., user moves city, switches projects) and archives the stale fact so it stops feeding the model — no more "ghost facts" hanging around in your system prompt.
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

Quick reference (full docs in [`USAGE.md`](./USAGE.md)):

| Command | Purpose |
|---------|---------|
| `/help` | Show all commands |
| `/provider [show \| llamacpp PATH]` | Inspect or swap LLM provider / managed model |
| `/memory list [KIND]` | Show active facts (profile, concept, state, behavioral) |
| `/memory list all [KIND]` | Include STALE archives for audit (STALE-paper §F.3 readout debug) |
| `/memory remember KIND SLUG CONTENT` | Manually add a fact |
| `/memory forget SLUG` | Remove a fact (hard delete) |
| `/memory archive SLUG` | Mark fact STALE (won't inject into prompt, kept for audit) |
| `/memory revive SLUG` | Flip STALE/UNKNOWN fact back to ACTIVE (recovery from bad adjudication) |
| `/memory profile [TEXT]` | View or set your one-line user bio |
| `/memory consolidate` | LLM rewrites all facts into a narrative profile |
| `/memory query <text>` | Debug: see which memory facts the LLM proposes for a scene |
| `/memory mode active\|all` | Toggle per-turn query proposal vs full-dump injection |
| `/memory restate <slug>` | LLM rescans recent chat to update a state fact |
| `/memory observe <query>` | LLM summarizes a behavioral pattern from recent messages |
| `/skill list \| use NAME \| off \| show NAME` | Skill management |
| `/pet list \| show NAME` | Pet character roster |
| `/clear` | Clear chat view (keeps memory + active skill) |

For workflows, tool reference, skill authoring, memory model deep dive, and troubleshooting: see **[USAGE.md](./USAGE.md)**.

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

**Schema** (4 kinds, from BOOKMARKS):

- **profile** — user identity (name, role)
- **concept** — entities the user mentioned (city, language, project)
- **state** — current task or focus
- **behavioral** — preferences and communication patterns, scoped per active skill

**Staleness adjudication** (from STALE paper). Each fact carries a status — `ACTIVE` / `STALE` / `UNKNOWN` — and the write path runs two LLM judgments to keep the prompt clean:

```
                  user message
                       │
                       ▼
              ┌─────────────────┐
              │ capture_facts   │  extract durable facts
              └────────┬────────┘
                       │
                       ▼
              ┌─────────────────────────────────┐
              │ Stage 1 — same-slot judge       │
              │  Reuse / Derive / Replace / New │  4-way write-side
              └───────┬─────────────────────────┘  adjudication
                      │
              ┌───────┴────────┐
              │   Replace?     │  (only on profile/state)
              └───────┬────────┘
                      │ yes
                      ▼
              ┌─────────────────────────────────┐
              │ Stage 2 — belief propagation    │
              │  for each state fact (cap 5):    │
              │   Keep / Stale / Unknown         │
              └────────────────┬────────────────┘
                               │
                               ▼
                  ┌──────────────────────────┐
                  │ partial unique index:     │
                  │  ACTIVE rows ≤ 1/slot     │
                  │  STALE rows coexist for   │
                  │  audit (kept in DB)       │
                  └──────────────────────────┘
                               │
                               ▼
              system prompt injects ACTIVE only;
              UNKNOWN renders as "[UNRESOLVED — previous
              value `X` may no longer be current]"
```

**Narrative**: trước đây Mochi chỉ có `Reuse / Derive / New` — khi user đổi thành phố hay đổi project, fact cũ vẫn nằm trong prompt và LLM tự mâu thuẫn. Bây giờ:

1. **Stage 1 (same-slot)** quyết định 4-way trên slot trùng candidate. `Replace` archives old + writes new (only fires cho `profile`/`state` — `concept`/`behavioral` ổn định không cho replace).
2. **Stage 2 (cross-fact propagation)** chỉ fire sau khi Stage 1 trả `Replace`. Mỗi state fact (cap 5) được LLM hỏi "có còn valid sau quan sát mới không?" → `Keep` / `Stale` / `Unknown`. Bắt được Type II conflicts (đổi city → commute fact mention West Lake bị mark stale dù không có lexical overlap).
3. **Constrained readout**: `list()` mặc định lọc STALE; UNKNOWN render với marker `[UNRESOLVED]` thay vì raw content (assistant biết phải hỏi user).
4. **Audit trail**: STALE rows **không bị xóa** — partial unique index `WHERE status='active'` cho phép STALE + ACTIVE coexist trong same slot. `/memory list all` để xem.

You can always inspect, archive, or recover manually via `/memory list [all] [KIND]`, `/memory archive SLUG`, `/memory revive SLUG`, and `/memory remember`.

For the full research write-up + tradeoffs + open advisor questions, see **[docs/research/stale-memory-application.md](./docs/research/stale-memory-application.md)**.

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
               │           │
               │           ▼
               │      ┌──────────────────────────────────┐
               │      │ Stage 1: same-slot judge         │
               │      │  Reuse / Derive / Replace / New  │  ← STALE §F.2
               │      └──────────┬───────────────────────┘     (BOOKMARKS +
               │                 │                              STALE paper)
               │                 │ if Replace on profile/state
               │                 ▼
               │      ┌──────────────────────────────────┐
               │      │ Stage 2: belief propagation      │
               │      │  per state fact:                  │
               │      │  Keep / Stale / Unknown           │
               │      └──────────┬───────────────────────┘
               │                 ▼
               │      ┌──────────────────────────────────┐
               │      │ SQLite (partial unique index on  │
               │      │  status='active'; STALE rows     │
               │      │  archived for audit)             │
               │      └──────────────────────────────────┘
               │
               ├─ stream_chat → /v1/chat/completions
               │
               └─ slash side-channel: rebuild system prompt
                  on /memory and /skill activity
                  (injects ACTIVE facts only; UNKNOWN as
                   [UNRESOLVED] marker)
```

## License

Apache-2.0. Forked from [claude-code-rust](https://github.com/srothgan/claude-code-rust) (Simon Peter Rothgang, Apache-2.0). See `LICENSE` for the full text.

This project is not affiliated with Anthropic or the original `claude-code-rust` author.

## Inspiration & references

### Papers / research

- **BOOKMARKS — Efficient Active Storyline Memory for Role-playing** (Koishi's Day 2026, [arxiv 2605.14169](https://arxiv.org/abs/2605.14169))
  → Adopted: 4-kind memory schema (profile / concept / state / behavioral), per-character behavioral scoping, BOOKMARKS-style judge (reuse/derive), per-turn query proposal in `active` memory mode
  → Implementation: `src/memory.rs`, `src/memory_capture.rs`, `src/memory_judge.rs`, `src/memory_query.rs`
- **STALE — Can LLM Agents Know When Their Memories Are No Longer Valid?** (Chao, Bai, Sheng, Li, Sun, 2026, [arxiv 2605.06527](https://arxiv.org/abs/2605.06527)) — [code](https://github.com/icedreamc/STALE) · [dataset](https://huggingface.co/datasets/STALEproj/STALE)
  → Adopted: write-side adjudication outcome `Replace` for same-slot conflicts; cross-fact belief propagation (`Keep` / `Stale` / `Unknown`) for Type II cascades; `ACTIVE / STALE / UNKNOWN` status enum + partial unique index so archived facts coexist with active ones for audit; constrained readout filters STALE before injection; UNKNOWN renders as warning marker (companion variant of paper's strict block)
  → Deferred (acknowledged in research doc): full 8-domain × slot schema, R_global bounded fallback, query-time presupposition verifier (PR coverage is assistant-side only). See [docs/research/stale-memory-application.md](./docs/research/stale-memory-application.md) for tradeoffs + open advisor questions.
  → Implementation: `JudgeOutcome::Replace` + `PropagateOutcome` in `src/memory_judge.rs`; `run_belief_propagation` in `src/app/connect/llama_lifecycle.rs`; eval scenarios in `tests/memory_stale_scenarios.rs` + migration safety in `tests/memory_legacy_migration.rs`.
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

### Research techniques implemented

**Memory adjudication (STALE-paper based)**

- **Two-stage write-side adjudication** — every captured fact runs through a same-slot judge (`Reuse` / `Derive` / `Replace` / `New`) at 0.0 temperature. `Replace` is restricted by prompt to `profile` and `state` kinds — concept/behavioral are stable categories and never trigger replace. Implements paper §F.2 Stage b.1.
- **Belief propagation after Replace** — when a profile or state Replace fires, every state fact (cap 5) is fed to a separate propagation prompt: "given the new observation and the just-archived value, is this stored fact still valid?" → `Keep` / `Stale` / `Unknown`. Catches Type II cascades (location change invalidating commute fact) where lexical overlap alone misses the dependency. Implements paper §F.2 Stage b.2, simplified for Mochi's flat 4-kind schema.
- **Partial unique index for audit trail** — `CREATE UNIQUE INDEX facts_active_slot ON facts (kind, slug, COALESCE(skill_scope, '')) WHERE status = 'active'` allows a single ACTIVE row to coexist with arbitrarily many STALE rows for the same slot. Replaces the previous full unique index (preserves history after REPLACE adjudication, paper §F.2 archive semantics).
- **Constrained readout** — `MemoryStore::list()` filters `status != 'stale'` by default; `list_all()` for audit. UNKNOWN rows render with explicit marker `[UNRESOLVED — previous value `X` may no longer be current]` so the assistant treats the slot as unresolved rather than reusing the cached default (companion variant of paper §F.3 strict block).
- **Idempotent legacy DB migration** — on-disk DBs from pre-STALE versions detect missing `status` / `stale_at` columns via `PRAGMA table_info` and `ALTER TABLE ADD COLUMN ... DEFAULT 'active'` in stages: table creation → column backfill → partial index install. No data loss; existing facts default to ACTIVE. Regression-tested in `tests/memory_legacy_migration.rs`.
- **`Derive` slug disambiguator** — when the same-slot judge picks `Derive` but candidate slug collides with the matched existing slug, append `-2`, `-3`, ... until a free active slot is found. Prevents silent overwrites of distinct-facet facts that share a base slug.

**BOOKMARKS-paper based**

- **BOOKMARKS query proposer + matcher** — `active` memory mode proposes up to 3 typed search queries (TAG|QUERY format) per user turn, matches via token overlap on slug+content, behavioral facts filtered by active skill scope. Falls back to `all` mode (full dump) when proposer fails.
- **Profile consolidation** — `/memory consolidate` rewrites all stored facts into a single ~200-word narrative paragraph (0.2 temperature, paper's `profile_extract` / `profile_aggregate` pattern at session scope).
- **Recursive state update + behavioral observation** — `/memory restate <slug>` and `/memory observe <query>` slash commands scan recent user messages and LLM-rewrite state/behavioral facts.

**Mochi-specific harness engineering**

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

# Mochi roadmap

Living checklist of work between **v0.1** (shipped) and the ambitious full-vision Mochi. Sections are ordered by recommended priority for impact-per-effort. Mark `[x]` as items ship; update at the end of each session.

---

## v0.1 — shipped

- [x] Fork claude-code-rust → mochi (Sprint 0)
- [x] llama.cpp HTTP+SSE client + `mochi chat` REPL (Sprint 1)
- [x] Full Ratatui TUI on llama provider via synthetic BridgeEvents (Sprint 1b)
- [x] ASCII pet roster — 5 characters × 5 moods (Sprint 2)
- [x] Skills system — Markdown `SKILL.md` loader + `/skill use|off|show|list` (Sprint 3)
- [x] SQLite memory — 4-kind BOOKMARKS schema, auto-capture via background LLM call (Sprint 4)
- [x] Slash commands `/memory`, `/skill`, `/pet`, `/clear` in TUI (Sprint 4/5)
- [x] README + NOTICE + CHANGELOG, `cargo install --path .`, `--version`, default `--provider llamacpp` (Sprint 5)

---

## Sprint 6 — Tools (read-only MVP shipped)

Goal: Mochi can actually do work, not just chat. Verified on Luna-SRSA (Qwen3 base) — tool-calling survives the RP finetune.

- [x] Add `tools` + `tool_choice` to `LlamaConfig` and request body (`src/agent/llama_client.rs`)
- [x] Parse `delta.tool_calls[]` from SSE response → emit `BridgeEvent::SessionUpdate::ToolCall`
- [x] Tool loop in `llama_lifecycle.rs`: tool_call → execute → feed result back as `tool` role → continue stream (max 6 iterations)
- [x] `sdk_kind_for` mapper so CCR's `ui/theme.rs::tool_name_label` renders correct icon (Read/Write/Bash/…) instead of generic Tool fallback
- [x] **Tool 1: `read_file`** — 200KB cap, tilde expand, returns header + body
- [x] **Tool 2: `find_file`** — substring search via `ignore::WalkBuilder`, respects `.gitignore`, max depth 8, top 20 matches

### Sprint 6b — Permission flow + read-side web tools (shipped)

- [x] Refactored all tools to Anthropic SDK PascalCase names + arg conventions (`Read` with `file_path`, `Glob` with `pattern`, etc.) — dropped the `sdk_kind_for` mapper shim
- [x] **Tool: `WebFetch`** — `reqwest` GET, HTML→text stripper (scripts/styles/comments dropped, entities decoded, whitespace collapsed), 32KB output cap
- [x] **Tool: `WebSearch`** — DuckDuckGo HTML endpoint, no API key, parses `uddg` redirect wrapper to clean URLs, returns `title | url | snippet` per result
- [x] **Tool: `Bash`** — `tokio::process::Command` via `bash -lc`, 30s default timeout (max 5min), 100KB stdout cap, stderr inlined when present, exit code surfaced on non-zero
- [x] Permission flow: `request_permission` emits `BridgeEvent::PermissionRequest`, drains `cmd_rx` for matching `PermissionResponse`, branches AllowOnce / AllowSession / Reject. `allow_set: HashSet<String>` per-session so "Allow for session" doesn't re-prompt.
- [x] CCR's inline permission UI rendered free (zero new render code) — Ctrl+y / Ctrl+a / Ctrl+n shortcuts inherited

### Sprint 6c — Write tools (shipped)

- [x] `ToolResult` struct splitting model-facing `model_text` from UI `Vec<ToolCallContent>` blocks
- [x] **Tool: `Write`** — full file write, auto-creates parent dirs, emits `ToolCallContent::Diff { old, new, … }` so CCR renders inline diff in the permission overlay
- [x] **Tool: `Edit`** — string-replace within a file. Unique-match by default; `replace_all: true` overrides. Refuses empty `old_string` and identical-string no-ops with actionable error hints
- [x] **Tool: `MultiEdit`** — sequential edits applied in-memory and persisted atomically; rolls back the entire batch if any edit fails (no partial writes)
- [ ] Backup-on-write — copy `path.bak` before overwriting so `/undo` can recover the last edit
- [ ] `/undo` slash command — pop last write/edit from a session backup stack

Decision points still open:
- Permission UX cache: scope by tool name (current) vs. scope by `tool name + arg fingerprint` (safer — `Bash session` allows any command vs. only the approved one).
- Sandbox depth: `chroot`? Docker? `nsjail`? Currently CWD-rooted with the OS-level denylist (no `sudo`, no `rm -rf /`).

---

## Sprint 7 — Pet, but make it pixel

Goal: pixel-art animated pet sits in TUI corner, reacts to state. Closer to the original Petdex-style vision.

- [ ] Add `viuer` + `image` crate (already in deps) sprite renderer module `src/ui/pet_render.rs`
- [ ] Bundle 16×16 (or 32×32) pixel-art pets — either draw custom or use a CC0 sprite pack from itch.io. Audit Petdex license before reuse.
- [ ] Layout: carve `pet_corner: Rect` from `src/ui/layout.rs` (bottom-right, ~6×4 chars).
- [ ] Wire `PetMood` → frame selection. Use existing 16ms TUI tick for animation.
- [ ] Thread `--pet NAME` from CLI down into `WelcomeBlock` so welcome banner matches.
- [ ] ASCII fallback when terminal lacks kitty/iTerm2/sixel graphics protocol.
- [ ] `/pet pick NAME` to switch live without restart.

---

## Sprint 8 — Multi-provider polish

Goal: not just llama.cpp. Cover the realistic local + cloud spectrum.

- [ ] Generic OpenAI-compatible provider (`base_url` + `api_key` envvar) — covers OpenAI, OpenRouter, Together, DeepInfra, Groq with one client.
- [ ] Ollama provider — auto-detect at `localhost:11434`.
- [ ] vLLM provider — already OpenAI-compatible; just docs.
- [ ] `/provider switch NAME` slash command — runtime provider swap.
- [ ] `--list-models` flag — query `/v1/models` and print available IDs.
- [ ] Per-provider config in `~/.mochi/providers.toml`.

---

## Sprint 9 — Sub-agents (DeerFlow-inspired)

Goal: lead Mochi spawns scoped sub-Mochis in parallel for complex tasks.

- [ ] Sub-agent runtime: spawn task with its own `history` and `system_prompt`, isolated from lead context.
- [ ] `dispatch_subagent` tool the lead can call: `{name, instructions, tools_allowed}`.
- [ ] Parallel execution via `tokio::join!`; results returned to lead as structured tool output.
- [ ] Token budget accounting per sub-agent.
- [ ] Render in TUI: collapsed "Sub-agent: research vietnam-history" block, expandable.
- [ ] Aggregator skill: lead synthesizes sub-agent outputs into final response.

---

## Sprint 10 — MCP server support in llama mode

Goal: reuse the MCP ecosystem (filesystem, GitHub, Slack, Notion, etc.) without needing Anthropic auth.

- [ ] Lift CCR's MCP wire types (`src/agent/types.rs::McpServerConfig`) into llama path.
- [ ] Implement MCP client in pure Rust (HTTP-SSE + stdio transports).
- [ ] Expose registered MCP tools through the same tool-call loop as Sprint 6 tools.
- [ ] `/mcp add URL` / `/mcp list` slash commands in llama mode.
- [ ] Config in `~/.mochi/mcp.toml`.

---

## Sprint 11 — Memory advanced (full BOOKMARKS)

Goal: from "auto-capture flat facts" toward the paper's active reasoning.

- [ ] Reuse/derive dedup judge: when capture extracts a fact near-similar to an existing fact, LLM-classify reuse/derive/new, prevent bloat.
- [ ] `/memory consolidate` — manual or N-fact-threshold trigger: LLM merges raw facts into a 200-word narrative profile.
- [ ] Active query proposal — before each response, LLM proposes up to 3 search queries (concept/state/behavioral); only fetch matched bookmarks into the prompt. Replaces "inject everything" with "inject what matters."
- [ ] Recursive state update — when a state fact is updated, run reset/update/none judge against intervening conversation chunks.
- [ ] Optional behavioral classifier — small local model (or LLM call) flags evidence per past message.

---

## Sprint 12 — Vietnamese / IME polish

- [ ] Document EVKey as the canonical Vietnamese setup (already in README — extend with screenshots).
- [ ] Investigate kitty keyboard protocol opt-in (`PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES | REPORT_ALTERNATE_KEYS)`) — may unlock IME composition on kitty/foot/wezterm.
- [ ] Add diagnostic mode `mochi --log-keys` that dumps every KeyEvent received — for users to confirm what their IME actually sends.
- [ ] If IME compose pattern is recognizable (backspace burst + non-ASCII), reconstruct composed chars heuristically. Best-effort only.

---

## Sprint 13 — TUI polish

- [ ] Color theme: pastel palette as alternative to Rust orange.
- [ ] Footer mood indicator: pet face emoji + label ("thinking…").
- [ ] Inline pet beside assistant message header.
- [ ] Better empty-state visuals on first launch (animated mochi cat instead of static).
- [ ] Markdown rendering improvements for code blocks (syntax highlight already there via syntect).
- [ ] `/theme NAME` slash command.

---

## Sprint 14 — Quality of life

- [ ] `/export PATH` — save current conversation as Markdown.
- [ ] `/import URL` — install skills from a public Git URL or `.skill` archive.
- [ ] Session persistence: `mochi resume` already works for Anthropic; wire it for llama mode (store history in SQLite).
- [ ] Better error messages with actionable next steps (e.g. "llama-server not reachable at 127.0.0.1:8765 — start it with `llama-server -m model.gguf --port 8765`").
- [ ] `/history` slash command — searchable past sessions.
- [ ] `mochi doctor` — verify config, llama-server reachable, memory DB writable, skills loadable.

---

## Sprint 15 — Performance & lint polish

Items from the simplify-skill review that were deferred:

- [ ] Cache `MemoryStore` in `App` — currently opens new SQLite connection per `/memory` invocation.
- [ ] Move `handle_prompt`'s 9-arg signature into a small `LlamaTurnContext` struct.
- [ ] Profile the auto-capture call — if Luna handles 256-token prompts in <500ms, fine. If slower, move to parallel-not-blocking and apply on next turn (currently spawn_local, OK).
- [ ] Build with `RUSTFLAGS="-C target-cpu=native -C lto=fat"` for release; strip symbols.
- [ ] **Re-enable strict CI clippy.** Currently CI runs `cargo clippy -- -D clippy::correctness -D clippy::suspicious -A clippy::pedantic`. Cleanup work needed in Mochi-new files (`src/tools/*`, `src/memory.rs`, `src/memory_capture.rs`, `src/app/connect/llama_lifecycle.rs`, `src/app/slash/executors.rs`, `src/skills.rs`): `format!()` append → `write!()`, function-too-long splits, `map().unwrap_or()` → `map_or()`, unnecessary raw string hashes, doc backticks. Then drop the `-A clippy::pedantic` exception so CI matches CCR's original lint strictness.

---

## Sprint 16 — IM channels (DeerFlow-style)

Lower priority unless someone wants Mochi on phone. Pick one to start.

- [ ] Telegram bot via Bot API long-polling — easiest entry, no public IP needed.
- [ ] Slack via Socket Mode.
- [ ] Discord via gateway WebSocket.
- [ ] Generic webhook receiver.

---

## Sprint 17 — Polish & v0.2 release

- [ ] Demo GIF in README (asciicast or terminalizer).
- [ ] Cross-platform: test Linux + Windows (wsl).
- [ ] Pre-built release binaries via GitHub Actions (`cargo dist`).
- [ ] Public repo + first GitHub release.
- [ ] License audit on any added sprite/skill content (Petdex, itch.io packs, etc.).

---

## Open architectural questions

These don't fit a single sprint; resolve via plan-mode discussion before the relevant sprint:

- [ ] **Permission model for tools** (Sprint 6): per-call vs. session-trust vs. allowlist file. Bias toward Claude-Code-style per-call for safety; allow `--auto-approve` flag for power users.
- [ ] **Sandbox depth** (Sprint 6 + later): chroot? Docker? `nsjail`? Reuse DeerFlow's `AioSandboxProvider` model?
- [ ] **Sub-agent context isolation** (Sprint 9): can sub-agents see lead's memory? Per-skill scope? Inherits behavioral facts?
- [ ] **Profile narrative size** (Sprint 11): hard cap? Per-session vs. global?
- [ ] **MCP auth** (Sprint 10): OAuth flows already exist in CCR — reuse or simplify?

---

## What we are explicitly NOT doing

To keep scope honest:

- ❌ Fine-tuning / SFT / RLHF — Mochi is inference-time only.
- ❌ Becoming a coding agent like Claude Code or Aider — different lane; tools yes, but not deeply IDE-integrated.
- ❌ Mobile app — terminal-only.
- ❌ Voice — terminal-only.
- ❌ Custom model serving — delegate to llama.cpp / Ollama / vLLM.

---

## Stretch / interesting (no sprint allocated)

Capture for later, no commitment:

- [ ] Web UI mirror (port the TUI to a `/dev/tty`-via-WebSocket React thing).
- [ ] Multi-user session (Mochi as a household pet on a shared server).
- [ ] Pet plays with itself when idle (autonomous loop, low-temperature soliloquy).
- [ ] Skill marketplace ÷ central registry.
- [ ] Memory export/import (`.mochipack` portable bundle).

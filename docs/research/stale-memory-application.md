# STALE → Mochi: Áp dụng paper "Implicit Conflict" vào memory layer

**Status**: Draft cho advisor review. Chưa implement.
**Author**: Duc (Hert4)
**Date**: 2026-05-17
**Paper**: STALE (arXiv [2605.06527](https://arxiv.org/abs/2605.06527), 7 May 2026) — Chao, Bai, Sheng, Li, Sun (Wuhan U / CUHK / HKUST)
**Target**: Mochi's BOOKMARKS-style memory in `src/memory.rs`, `src/memory_judge.rs`, `src/memory_query.rs`

---

## 1. TL;DR

STALE chỉ ra một failure mode mà current memory systems (Mem0, A-mem, LightMem, Zep) đều fail: khi quan sát mới *implicitly* invalidate fact cũ (không có "I no longer X" rõ ràng), system tiếp tục dùng fact cũ → trả lời sai dù đã *retrieve* được fact mới. CUPMem (prototype của tác giả) đẩy GPT-4o-mini từ 8.7% → 68.0% bằng cách thêm **write-side adjudication** (KEEP/STALE/REPLACE/UNKNOWN) và **constrained readout** (lọc status trước khi inject vào prompt).

Mochi đang ở chỗ y hệt baseline trong paper — judge có Reuse/Derive/New, không có khái niệm "stale". Đề xuất: import 2 primitives nhẹ nhất (4-way adjudication + status column), bỏ phần schema-heavy (8 domain × slot). Cost = ~80 LOC, không thêm dependency, không thay đổi public API.

---

## 2. Paper summary

### 2.1 Problem: Implicit Conflict

**Formal definition** (paper §3.2):

Cho dialogue history `{m₁, ..., mₙ}` và world knowledge `K`. Implicit conflict xảy ra khi:

- **Axiom 1 (Belief Incompatibility)**: ∃ observation `mₒ` (o<n) và attribute `a` sao cho `mₒ` support belief `vₒ(a)`, nhưng `mₙ ⊨_K ¬vₒ(a)` (mₙ render vₒ(a) invalid dưới world knowledge).
- **Axiom 2 (Non-explicit Invalidation)**: Không có utterance nào trong `{m_{o+1}, ..., mₙ}` explicitly negate vₒ(a) (không có "I no longer...", "actually now...").

**Hai types**:

| Type | Cơ chế | Ví dụ |
|------|--------|-------|
| **I — Co-referential** | mₒ và mₙ nói về cùng attribute, value khác nhau | User nói sống ở Seattle → 6 tháng sau "đang setup utilities cho căn hộ mới ở Portland" (cùng attribute `current_location`, value đổi, không nói "no longer Seattle") |
| **II — Propagated** | mₙ update attribute B → invalidate attribute A qua dependency `B →_K A` | User quen Portland → sau đó "thấy bark scorpion trong giày, khí hậu khô quá" (climate đổi → ngụ ý location ≠ Portland, không nói gì về location) |

### 2.2 Evaluation protocol — 3 trục

Cho mỗi conflict scenario, paper hỏi LLM 3 query khác nhau:

1. **SR (State Resolution, explicit)**: "Dựa vào lịch sử chat, user còn đi xe đạp đi làm không?" → model phải nhận ra fact cũ đã invalid.
2. **PR (Premise Resistance, adversarial)**: "User đi xe đạp mỗi ngày, viết maintenance plan cho user." → model phải reject premise sai và ground vào belief mới.
3. **IPA (Implicit Policy Adaptation, implicit)**: "Đề xuất commute plan tuần này." → query không nhắc mₒ/mₙ nhưng đáp án đúng cần dùng updated state.

Đánh giá bằng LLM-as-judge (Gemini-3.1-flash-lite), 95.8% agreement với human.

### 2.3 Key results (Table 2)

| Model | Type I SR | Type I PR | Type I IPA | Type II SR | Type II PR | Type II IPA | Overall |
|-------|-----------|-----------|------------|------------|------------|-------------|---------|
| GPT-4o-mini | 30.0% | 0.0% | 11.0% | 9.5% | 0.0% | 1.5% | **8.7%** |
| GPT-5.4 | 35.0% | 2.0% | 29.0% | 9.0% | 2.0% | 17.0% | 15.7% |
| Gemini-3.1-pro | 92.0% | 30.0% | 71.0% | 69.0% | 14.0% | 55.0% | **55.2%** (best LLM) |
| Qwen3.5-27B | 76.0% | 4.0% | 39.0% | 42.0% | 3.5% | 23.0% | 31.3% |
| LightMem | 52.5% | 1.0% | 23.5% | 21.5% | 0.5% | 7.5% | 17.8% (best memory framework) |
| Mem0 | 17.0% | 1.0% | 22.0% | 3.5% | 0.0% | 6.5% | 8.3% |
| **CUPMem (theirs)** | **91.0%** | **78.0%** | 32.0% | **89.0%** | **75.0%** | 43.0% | **68.0%** |

**Findings**:
- F1: **Recognition ≠ Application** — model nhận ra fact cũ outdated (high SR) nhưng vẫn dùng nó khi answer (low IPA).
- F2: **Premise bias pervasive** — PR là weakest dimension cho mọi model. Gemini-pro: 92% SR → 30% PR.
- F3: **Type II khó hơn Type I** — propagation đòi commonsense reasoning over dependency.
- F4: **Adding memory framework không tự fix** — 4/5 memory frameworks tệ hơn plain GPT-4o-mini. Vấn đề không phải recall, là **current-state adjudication gap** — fact mới được retrieve nhưng không displace fact cũ.

### 2.4 CUPMem method (§5, Appendix F)

CUPMem là prototype, không phải general architecture. 3 components:

**(a) Memory representation — two-level schema Ω**

```
Ω = {(b, ℓ) : b ∈ B, ℓ ∈ T_b}
```

8 domains B (Table 8):
- `location_and_living`, `weather_and_environment`, `health_and_mobility`,
- `work_and_schedule`, `finance_and_resources`, `family_and_caregiving`,
- `routine_and_transport`, `current_focus_and_goals`

Mỗi domain có slots với cardinality `single` (current default duy nhất) hoặc `multi`. Schema fixed trước evaluation, không phụ thuộc benchmark.

Memory item: `mᵢ = (id, b, ℓ, value, status ∈ {ACTIVE, STALE}, timestamp, evidence)`.

Slot không có replacement chắc chắn → mark `UNKNOWN_CURRENT`.

**(b) Write-side belief updating — two stages (Adjudication)**

Đây là chỗ doc trước summarize sai (1 stage). Paper §F.2 thật ra có **2 stage tách biệt**:

*Stage b.1 — Local update* (same-slot only):
```
a_k = U(δ_k, R_same-slot(δ_k), R_same-domain(δ_k))
a_k ∈ {ADD, REFINE, REPLACE, NO_OP}
```
Mỗi candidate `δ_k = (b, ℓ, v̂, z, γ, τ, E)` quyết định 1 trong 4 thao tác trên slot trùng nó. **REPLACE ở đây là same-slot decision** (đổi value cùng attribute).

*Stage b.2 — Cross-region revision* (different slot via dependency):
```
R_τ = R_direct ∪ R_affected ∪ R_global
y_i = J_θ(i, Δ_t, x_t, Ω) ∈ {KEEP, STALE, REPLACE, UNKNOWN}  for each i in R_τ
```
Stage này mới là chỗ phát hiện Type II propagation: scan candidate set, mỗi item nhận decision STALE/UNKNOWN qua **separate LLM call**. R_global là bounded fallback (chỗ nhiều recall đến từ đây).

Đây là cốt lõi: trước đây memory frameworks chỉ append → mâu thuẫn coexist. CUPMem ép quyết định *trước query time* qua 2 LLM call/turn (1 same-slot + 1 cross-region; affected-region expand cũng có thể là call riêng).

**(c) Topology-triggered belief propagation (Search)**

Để handle Type II, search space mở rộng:
```
R_τ = R_direct  ∪  R_affected  ∪  R_global
```

- `R_direct`: same slot/domain với candidate mới.
- `R_affected`: neighboring domains có commonsense dependency. Ví dụ: `health_and_mobility` change → check `routine_and_transport` (limb injury invalidates cycling commute).
- `R_global`: bounded fallback bên ngoài hai trên.

Một LLM call expand "affected regions" từ schema topology.

**(d) Constrained readout at query time**

Query `q` được map về:
```
π(q) = (intent, presuppositions, current_state_basis_needed, action)
```

Verifier kiểm `V(q, M) ∈ {SUPPORTED, OUTDATED, UNRESOLVED}`. Nếu presupposition dùng STALE item → block, reconstruct current basis từ ACTIVE items.

**Cost**: $0.37/instance (paper §C). Backbone GPT-4o-mini.

### 2.5 Authors' stated limitations (§A)

1. **One-shot conflict** only. Real world có repeated updates, coupled propagation across many attrs, gradual drift — STALE không cover.
2. **LLM-generated dialogue** — distributional gap với organic interaction.
3. **Predefined schema** — brittle, không generalize. "Schema-free approaches that can generalize to arbitrary user attributes" = future work.
4. **LLM-as-judge** — 95.8% agreement nhưng có conservative bias.

---

## 3. Where Mochi sits (gap analysis)

### 3.1 Current state — Mochi Sprint 11 (shipped)

| Component | File | Behavior |
|-----------|------|----------|
| 4-kind schema | `src/memory.rs::FactKind` | Profile / Concept / State / Behavioral. One-level (no slot). |
| Capture | `src/memory_capture.rs` | LLM extracts facts từ user message, "would this fact still describe them next month?" filter. |
| Judge | `src/memory_judge.rs::JudgeOutcome` | `Reuse{id}` / `Derive{id}` / `New`. Bias toward reuse khi same attribute paraphrased. |
| Query proposal | `src/memory_query.rs::propose_queries` | LLM proposes up to 3 typed queries, match via token overlap. |
| State refresh | `src/memory_judge.rs::restate_from_history` | **Manual** via `/memory restate` — LLM scan recent messages, rewrite state fact. |
| Behavioral observe | `src/memory_judge.rs::observe_behavioral_pattern` | Manual via `/memory observe`. |
| Profile consolidate | `src/memory_judge.rs::consolidate_profile` | Manual via `/memory consolidate`. |
| Schema (SQL) | `facts(id, kind, slug, content, skill_scope, created_at, last_used)` | **Không có status column** — facts implicitly luôn ACTIVE. |

### 3.2 Mapping Mochi failure modes onto STALE's 3 axes

| STALE axis | Mochi behavior hiện tại | Verdict |
|------------|-------------------------|---------|
| **SR — State Resolution** | Khi user nói "actually now I work on crab-eval", capture pipeline lưu fact mới. Judge có thể Reuse hoặc New, không có Replace → cả fact cũ ("working on Mochi") và mới coexist. Inject cả 2 vào prompt → LLM thấy 2 thông tin mâu thuẫn. | **FAIL** — không có write-time adjudication. |
| **PR — Premise Resistance** | User hỏi "viết roadmap tiếp cho Mochi" sau khi đã chuyển sang crab-eval. Mochi sẽ comply vì fact cũ vẫn ACTIVE trong prompt. | **FAIL** — không có premise verifier. |
| **IPA — Implicit Policy Adaptation** | User hỏi "ngày mai schedule gì" sau khi đã đổi project. Đáp án đúng phải dùng new state. Mochi sẽ random — có thể trigger query proposal hit fact cũ. | **FAIL — partial** — `propose_queries` không biết fact nào "fresh". |

Mochi đang ở chỗ baseline (8-15%) của paper, không phải chỗ frameworks (8-18%) hay CUPMem (68%).

### 3.3 Where Mochi DIFFERS from paper setup

| Aspect | Paper | Mochi |
|--------|-------|-------|
| Context length | 150K tokens haystack | 32K (Qwen3 default) |
| Schema | 8 domains × N slots | 4 flat kinds |
| Trigger | One-shot session swap | Streaming turn-by-turn |
| User base | Synthetic personas | Single real user (long-running) |
| Eval | Offline benchmark | Live conversation |
| Compute budget | $0.37/instance cloud | Local llama.cpp, latency-bound |

→ **Implication**: Mochi không cần full CUPMem. Cần "minimal viable adjudication".

---

## 4. Application proposal

### 4.1 Scope — what to import, what to skip

✅ **Import**:
- Status enum on facts (ACTIVE / STALE / UNKNOWN) — drives constrained readout.
- Two adjudication surfaces, **NOT a single 4-way enum** (corrected from earlier draft):
  - Write-time `JudgeOutcome ∈ {Reuse, Derive, Replace, New}` — same-slot decisions (paper Stage b.1, restricted to profile/state for Replace).
  - Propagation `PropagateOutcome ∈ {Keep, Stale, Unknown}` — cross-fact validity check fired after a Replace on profile/state (paper Stage b.2 collapsed to single-pass).
- Constrained readout: filter STALE from `list()`; render UNKNOWN with explicit warning marker so the model treats slot as unresolved.

⚠️ **Adapt with reduction**:
- Cross-region search (Stage b.2). Paper splits into R_direct ∪ R_affected ∪ R_global. Mochi has no schema topology — collapse all three into "every state fact becomes a propagation candidate, capped at MAX_CANDIDATES=5, LLM judges each individually".
- Affected-region expander LLM call → dropped; we feed the new observation + archived value directly to the propagation prompt.

❌ **Skip** (with honest caveats):
- Full 8-domain × slot schema — over-engineering for single-user terminal pet.
- Query-time premise verifier (component d). **This is the load-bearing cut.** Paper Table 2 shows CUPMem's PR jump (0% → 78%) comes mostly from this component. Filtering STALE from prompt prevents *reinforcement* of bad premises by the assistant, but does **nothing** when the user themselves embeds the stale premise in the query (e.g., "write me the Mochi roadmap" after they switched to crab-eval). PR coverage in Mochi is therefore **partial — assistant-side only, not user-side**. Honest framing instead of pretending we cover PR.
- 400-scenario in-house benchmark — but see §5 Step 8 below: we now port ~50 instances from the released HF dataset for defensible numbers.

### 4.2 Concrete mapping

```
STALE concept                       → Mochi implementation
────────────────────────────────────────────────────────────────────
ACTIVE / STALE / UNKNOWN status     → facts.status TEXT column + partial unique index
                                      (only ACTIVE rows enforce slot uniqueness; STALE
                                      coexists for audit)
Stage b.1 KEEP outcome              → JudgeOutcome::Reuse (existing — overwrite slot content)
Stage b.1 REFINE                    → JudgeOutcome::Derive (existing — distinct facet, new row)
Stage b.1 REPLACE                   → JudgeOutcome::Replace { id } [NEW]
                                      → mark_stale(old) + upsert(new)
Stage b.1 NO_OP                     → JudgeOutcome::New (different slot)
Stage b.2 KEEP / STALE / UNKNOWN    → PropagateOutcome enum [NEW]
                                      Fires per state fact after a Replace on profile/state
Constrained readout                 → memory.list() filters status != STALE
                                      UNKNOWN renders with warning marker (not raw content)
R_direct ∪ R_affected ∪ R_global    → All state facts × MAX_CANDIDATES=5 cap
                                      (no schema-driven affected-region expander)
Two-level schema (b, ℓ)             → SKIP — keep FactKind flat
Query-time presupposition verifier  → SKIP — assistant-side STALE filter only
                                      (PR coverage is partial; see §4.1 cut justification)
```

### 4.3 Hệ quả về API

Public API thay đổi:
- `JudgeOutcome` enum thêm 3 variants. Pattern match callers cần handle.
- `MemoryStore::upsert` không đổi (vẫn tạo ACTIVE).
- New: `MemoryStore::mark_stale(id)`, `MemoryStore::mark_unknown(id, attribute)`.
- `MemoryStore::list()` mặc định filter status != STALE; thêm `list_all()` cho `/memory list` slash command.

Backward compat: facts cũ migrate với `status='active'`. SQLite migration thêm column với DEFAULT 'active'.

---

## 5. Implementation plan

### Step 1 — DB migration + status enum
**File**: `src/memory.rs`
**Change**:
```rust
pub enum FactStatus { Active, Stale, UnknownCurrent }

// Schema migration:
ALTER TABLE facts ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE facts ADD COLUMN stale_at INTEGER;  // timestamp khi archive
```
**Verify**: open existing `~/.mochi/memory.db`, column thêm, facts cũ vẫn list được.

### Step 2 — Extend JudgeOutcome
**File**: `src/memory_judge.rs`
**Change**:
```rust
pub enum JudgeOutcome {
    Reuse { existing_id: i64 },
    Derive { existing_id: i64 },
    Stale { existing_id: i64 },          // archive cũ, không tạo mới
    Replace { existing_id: i64 },         // archive cũ, tạo mới với content mới
    Unknown { existing_id: i64 },         // mark cũ unknown_current, không tạo mới
    New,
}
```
**Verify**: cargo build green.

### Step 3 — Adjudication prompt
**File**: `src/memory_judge.rs::JUDGE_SYSTEM`
**Change**: thêm 3 case mới vào prompt. Bias rule:
- `REUSE` — same attribute, same value (như cũ).
- `DERIVE` — same attribute, refines (như cũ).
- `REPLACE` — same attribute, *clearly* different value (e.g. city Hanoi → Saigon, current task X → Y). **Restrict** to `state` and `profile` kinds only.
- `STALE` — same attribute, old value contradicted by new context **without** explicit new value (e.g. broken leg implies cycling commute invalid, không nói commute mode mới).
- `UNKNOWN` — chỉ khi STALE but không có replacement candidate.
- `NEW` — different attribute (như cũ).
**Verify**: unit test với fixture scenarios:
- "I moved to Saigon" sau "I live in Hanoi" → REPLACE
- "I broke my leg" sau "I bike to work daily" → STALE on biking commute fact
- "Where do I live?" → returns active fact only

### Step 4 — Capture path branch
**File**: `src/memory_capture.rs` (caller của judge_capture)
**Change**: match thêm 3 outcomes:
```rust
match outcome {
    Replace { existing_id } => {
        store.mark_stale(existing_id)?;
        store.upsert(kind, slug, new_content, scope)?;
    }
    Stale { existing_id } => store.mark_stale(existing_id)?,
    Unknown { existing_id } => store.mark_unknown(existing_id)?,
    // ... existing arms
}
```
**Verify**: integration test — feed sequence ("I live in Hanoi", later "moved to Saigon, đang setup nội thất") → DB có 1 STALE Hanoi + 1 ACTIVE Saigon.

### Step 5 — Constrained readout
**File**: `src/memory.rs::list`, `src/app/connect/llama_lifecycle.rs::build_system_prompt`
**Change**: `list()` filter `status = 'active'` mặc định. UNKNOWN_CURRENT facts render thành `"user's current X is unclear (was: ..., needs reconfirmation)"` thay vì raw content. STALE không inject.
**Verify**: snapshot test trên rendered system prompt — STALE không xuất hiện; UNKNOWN_CURRENT có warning marker.

### Step 6 — Light propagation (Type II minimum)
**File**: `src/memory_judge.rs`
**Change**: khi REPLACE/STALE 1 fact `profile|location`, scan kind=state for facts có token overlap với location → run judge lần 2 cho từng cái với context "user just changed location, does this still hold?" → mark STALE/UNKNOWN cascading.
**Verify**: test fixture "I live in Hanoi" + state "commute by bike around West Lake" → after REPLACE location to Saigon, state fact marked STALE (West Lake không có ở Saigon → commute invalid).

### Step 7 — Slash command surface
**File**: `src/app/slash/executors.rs`
**Change**:
- `/memory list` — show ACTIVE only (default).
- `/memory list all` — show ACTIVE + STALE + UNKNOWN (with status badge).
- `/memory archive <slug>` — manual mark STALE.
- `/memory revive <slug>` — flip STALE → ACTIVE (mistake recovery).
**Verify**: type each command in REPL, check output.

### Step 8 — Eval harness (optional, advisor-relevant)
**File**: `tests/memory_stale_scenarios.rs`
**Change**: 10-20 hand-written scenarios mimic Type I + Type II từ paper, evaluate 3 axes (SR/PR/IPA) bằng simple keyword check + LLM judge. Baseline = no adjudication; treatment = with Step 1-7.
**Verify**: treatment > baseline trên SR + PR axes. Report numbers in PR description khi merge.

### Cumulative diff estimate
- ~80 LOC core (memory.rs + memory_judge.rs)
- ~30 LOC migration + tests
- ~40 LOC slash command updates
- 1 new function (`propagate_on_change`)
- 0 new dependencies
- 0 schema-breaking changes (column add với DEFAULT)

---

## 6. Risks và open questions cho advisor

### 6.1 Rủi ro implementation

| Risk | Mitigation |
|------|------------|
| False positive REPLACE (xóa fact đúng) | Restrict REPLACE to `state`/`profile` only. `concept`/`behavioral` không cho replace (stable categories). Manual `/memory revive` để recover. |
| LLM judge cost — extra call mỗi turn? | Không thêm call. Judge đã chạy hiện tại; chỉ mở rộng prompt + 3 outcome → cùng 1 call. |
| Propagation cascade quá rộng | Chỉ propagate khi `state`/`profile` đổi VÀ slug có token overlap. Cap 3 facts checked per cascade. |
| Migration backward compat | DEFAULT 'active' on column add → users hiện tại không bị mất facts. |

### 6.2 Câu hỏi advisor (chính)

1. **Schema-free direction (paper limitation #3)**: Mochi có 4 flat kinds vs paper 8 domain×slot. Implementation hiện tại feed *all state facts* (cap 5) vào LLM propagation prompt mỗi khi profile/state bị Replace. Đây thay R_direct ∪ R_affected ∪ R_global của paper. **Concrete fork question**: nên giữ "all-state-facts × 5 cap" này, hay add 1 LLM call expansion bước (mimic paper's affected-region expander) để enumerate likely-affected facts trước, rồi mới gọi judge per fact? Trade-off: cost (2 calls vs 5 calls) vs coverage.

2. **Streaming vs one-shot**: Paper assume session swap rõ ràng (sessionₒ vs sessionₙ). Mochi là streaming dialogue. **Default position**: tôi sẽ chạy adjudication mỗi turn tới khi đo được latency cost, sau đó batch theo cửa sổ messages. Advisor có lý do disagree không?

3. **Eval methodology**: ~~10-20 hand-written scenarios~~ — đã đổi. **Commit**: port ~50 instances từ HF dataset `STALEproj/STALE` (CC-BY-4.0), ingest mₒ → mₙ qua 2 turns, run SR+PR+IPA queries, judge với Qwen-as-judge local. Giữ 15 hand-written như Mochi-domain sanity check trên đó. Sanity check methodology giúp tôi không bias the eval?

4. **PR coverage gap (MOST IMPORTANT)**: Removing STALE từ prompt là **assistant-side** filter only. Khi *user* embed stale premise vào query ("viết roadmap Mochi tiếp" sau khi đã chuyển crab-eval), không có verifier → LLM vẫn comply. **Yes/no question**: Có acceptable để ship với PR partial coverage này, hay tôi nghĩa vụ add at least 1 lightweight presupposition check (e.g. extract `Pq` từ query → check if any Pq fact has status=STALE → inject warning) trước answer generation?

5. **UNKNOWN handling architecture**: CUPMem chặn UNKNOWN slot khỏi prompt entirely. Mochi (personal companion vibe) đang render với marker "[UNRESOLVED — previous value `X` may no longer be current; ask the user before relying on it]". Theo paper, đây là CUPMem làm sai (Stage b.2 block thẳng, không annotate). Nhưng companion UX argue dùng marker tốt hơn (assistant ask user thay vì hành xử như không biết). Pick: paper-faithful (block) hay companion-friendly (marker)?

### 6.3 Câu hỏi nếu có thời gian

- Cost measurement: paper $0.37/instance trên cloud. Mochi local llama.cpp, đo latency overhead/turn sau implement.
- Whether to expose `status` lên TUI (badge bên cạnh fact list)?
- Có nên log adjudication decisions vào audit file để debug?

---

## 7. Timeline đề xuất

| Phase | Days | Deliverable |
|-------|------|-------------|
| Advisor review | — | Feedback trên doc này |
| Step 1-2 (schema + enum) | 0.5 | PR #1 — migration |
| Step 3-4 (judge + capture) | 1 | PR #2 — adjudication logic |
| Step 5-6 (readout + propagation) | 1 | PR #3 — query-time |
| Step 7 (UX) | 0.5 | PR #4 — slash commands |
| Step 8 (eval) | 1 | PR #5 — test scenarios + numbers |
| **Total** | **~4 days** | Sprint 11b ship |

---

## 7b. Update log

Sau khi spawn 2 advisor agents (code-review + research-review) để check pass đầu tiên:

**Implementation đã ship** (1401 tests pass, 0 clippy errors):
- Steps 1-8 từ §5. Files modified: `src/memory.rs`, `src/memory_judge.rs`, `src/app/connect/llama_lifecycle.rs`, `src/app/slash/executors.rs`. New tests: `tests/memory_stale_scenarios.rs` (10), `tests/memory_legacy_migration.rs` (2).
- Partial unique index `WHERE status='active'` cho phép STALE/ACTIVE coexist trong same slot — preserves audit trail.

**Code-review fixes applied** (pre-merge):
- B1: migration order — `migrate_add_status_columns` chạy **trước** CREATE INDEX để legacy DBs không fail open.
- I5: belief propagation không gate trên token overlap nữa — feed all state facts (cap 5) vào LLM với context "user changed X from old to new".
- I3: Derive slug collision — disambiguator append `-2`, `-3` nếu base slug collides với existing active row.
- I1: `/memory revive` surface UNIQUE constraint conflicts thay vì silent.
- I4: legacy migration integration test added.

**Research-doc fixes applied**:
- §2.4 nay mô tả CUPMem 2-stage (local update vs cross-region revision) chứ không 1-stage.
- §4.1 honest về PR coverage gap (assistant-side only).
- §4.2 mapping rewritten: KEEP/STALE/REPLACE/UNKNOWN paper *không* map 1-1 vào single Mochi enum; thay vào đó split thành `JudgeOutcome` (write-time) + `PropagateOutcome` (cross-fact).
- §6.2 reframe: Q3 commit dataset port, Q4 promoted "most important", Q5 reframed architectural.

**Outstanding (defer / advisor input):**
- Q1: 1-call-vs-2-call propagation expansion.
- Q4: PR partial coverage — accept hay add presupposition verifier?
- Q5: UNKNOWN block-vs-marker.
- HF dataset port (~50 instances) — separate PR after implementation merge.

## 8. References

- STALE paper: <https://arxiv.org/abs/2605.06527>
- STALE code: <https://github.com/icedreamc/STALE>
- STALE dataset: <https://huggingface.co/datasets/STALEproj/STALE>
- BOOKMARKS (Mochi memory base, Sprint 4/11): arxiv 2605.14169
- Mochi current memory code: `src/memory.rs`, `src/memory_judge.rs`, `src/memory_query.rs`
- LightMem (best baseline framework in paper): cited as [4]
- Mem0 (popular OSS memory framework, scored 8.3% in paper): cited as [2]

// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Offline scenarios mirroring the STALE benchmark (arXiv 2605.06527) Type I
//! (co-referential) and Type II (propagated) implicit conflicts. These tests
//! exercise the *adjudication mechanics* — they do NOT call an LLM. They feed
//! synthetic judge/propagation outcomes that an oracle judge would return for
//! each scenario, then assert that the resulting memory store correctly
//! reflects the constrained-readout invariants (§F.3): no STALE fact appears
//! in the active list, UNKNOWN facts render with a warning marker, REPLACE
//! installs the new value while archiving the old.

use claude_code_rust::memory::{FactKind, FactStatus, MemoryStore, render_memory_section};

fn fresh_store() -> MemoryStore {
    MemoryStore::open_in_memory().unwrap()
}

// ---------------------------------------------------------------------------
// Type I — Co-referential conflict scenarios
// ---------------------------------------------------------------------------
//
// Both old and new observations address the same attribute, but imply
// incompatible values. The oracle outcome here is `Replace`: the existing
// fact is archived and the new value installed under the same slug. Active
// readout (system-prompt injection) should now reflect only the new value.

#[test]
fn type_i_co_referential_location_change_archives_old_and_installs_new() {
    let store = fresh_store();
    let old_id = store.upsert(FactKind::Profile, "city", "Hanoi", None).unwrap();
    // Oracle judge outcome: Replace { existing_id: old_id }.
    store.mark_stale(old_id).unwrap();
    store.upsert(FactKind::Profile, "city", "Saigon", None).unwrap();

    let active = store.list(None).unwrap();
    let cities: Vec<&str> =
        active.iter().filter(|f| f.slug == "city").map(|f| f.content.as_str()).collect();
    assert_eq!(cities, vec!["Saigon"], "active readout must show only the new value");

    let all = store.list_all(None).unwrap();
    let stale_count =
        all.iter().filter(|f| f.slug == "city" && f.status == FactStatus::Stale).count();
    assert_eq!(stale_count, 1, "old value must remain archived for audit");
}

#[test]
fn type_i_state_focus_change_does_not_leak_old_into_prompt() {
    let store = fresh_store();
    let old_id = store.upsert(FactKind::State, "focus", "writing Mochi", None).unwrap();
    store.mark_stale(old_id).unwrap();
    store.upsert(FactKind::State, "focus", "working on crab-eval", None).unwrap();

    let rendered = render_memory_section(&store.list(None).unwrap(), None);
    assert!(rendered.contains("crab-eval"));
    assert!(
        !rendered.contains("Mochi"),
        "STALE paper §F.3 invariant: archived state must not appear in prompt: {rendered}"
    );
}

#[test]
fn type_i_profile_role_paraphrase_uses_reuse_not_replace() {
    // Same value paraphrased — judge should choose `Reuse`, not `Replace`.
    // We simulate by upserting the same slug (no mark_stale).
    let store = fresh_store();
    store.upsert(FactKind::Profile, "role", "AI engineer", None).unwrap();
    store.upsert(FactKind::Profile, "role", "AI Engineer", None).unwrap();
    let active: Vec<_> = store
        .list(Some(FactKind::Profile))
        .unwrap()
        .into_iter()
        .filter(|f| f.slug == "role")
        .collect();
    assert_eq!(active.len(), 1, "reuse must not create duplicate rows");
    assert_eq!(active[0].status, FactStatus::Active);
}

// ---------------------------------------------------------------------------
// Type II — Propagated conflict scenarios
// ---------------------------------------------------------------------------
//
// The new observation updates attribute B; this cascades to invalidate
// attribute A via commonsense dependency. The oracle outcome on B is
// `Replace`; propagation should then run on A and return either `Stale` (if
// a replacement is implied) or `Unknown` (if no replacement).

#[test]
fn type_ii_location_change_propagates_stale_to_commute_routine() {
    // Setup: user lives in Hanoi, commutes by bike around West Lake.
    // New observation: user moved to Saigon. Commute around West Lake is
    // now invalidated (no West Lake in Saigon). Propagation should mark
    // commute STALE.
    let store = fresh_store();
    let _city = store.upsert(FactKind::Profile, "city", "Hanoi", None).unwrap();
    let commute_id =
        store.upsert(FactKind::State, "commute", "bikes around West Lake daily", None).unwrap();

    // Apply replace on city + propagation outcome=Stale on commute.
    let city_id =
        store.list(Some(FactKind::Profile)).unwrap().iter().find(|f| f.slug == "city").unwrap().id;
    store.mark_stale(city_id).unwrap();
    store.upsert(FactKind::Profile, "city", "Saigon", None).unwrap();
    store.mark_stale(commute_id).unwrap();

    let rendered = render_memory_section(&store.list(None).unwrap(), None);
    assert!(rendered.contains("Saigon"));
    assert!(
        !rendered.contains("West Lake"),
        "Type II propagation: commute fact must be archived after location change"
    );
    assert!(!rendered.contains("Hanoi"), "old location must not leak");
}

#[test]
fn type_ii_health_change_propagates_unknown_to_routine() {
    // Setup: user runs 5km every morning. New: user broke their leg.
    // Running invalidated, but no new routine implied. Propagation should
    // mark the routine UNKNOWN, surfacing a warning marker so the assistant
    // asks before reusing the old default.
    let store = fresh_store();
    let routine_id =
        store.upsert(FactKind::State, "morning-routine", "5km run at sunrise", None).unwrap();
    store.upsert(FactKind::Concept, "injury", "broken leg, recovering", None).unwrap();
    store.mark_unknown(routine_id).unwrap();

    let rendered = render_memory_section(&store.list(None).unwrap(), None);
    assert!(
        rendered.contains("UNRESOLVED"),
        "UNKNOWN-status state fact must surface warning marker: {rendered}"
    );
    assert!(
        rendered.contains("5km run"),
        "warning marker must cite the prior value so the assistant knows what was retired"
    );
}

#[test]
fn type_ii_orthogonal_observation_does_not_invalidate_unrelated_state() {
    // Setup: user works on crab-eval. New: user bought a new keyboard.
    // Propagation should output `keep` — unrelated. We simulate by NOT
    // marking the state fact stale.
    let store = fresh_store();
    let focus_id = store.upsert(FactKind::State, "focus", "crab-eval research", None).unwrap();
    store.upsert(FactKind::Concept, "hardware", "bought a new keyboard", None).unwrap();
    // Oracle: propagate_validity returns Keep → no mark_stale.
    let focus = store.get(focus_id).unwrap().unwrap();
    assert_eq!(focus.status, FactStatus::Active, "orthogonal observation must not cascade");
}

// ---------------------------------------------------------------------------
// Probing dimensions — sanity checks on what the system prompt would surface
// ---------------------------------------------------------------------------
//
// These tests pin invariants the STALE eval protocol (§3.5) probes:
// - SR (state resolution): query about the stale belief must NOT find it
//   active. The prompt should reflect only the new state.
// - PR (premise resistance): system prompt should not surface the old
//   premise. STALE facts being archived means the LLM never sees them.
// - IPA (implicit policy adaptation): the *current* state must be present
//   and visible for downstream planning to use it.

#[test]
fn sr_invariant_stale_fact_invisible_to_active_readout() {
    let store = fresh_store();
    let bike_id =
        store.upsert(FactKind::State, "commute", "cycles to work every day", None).unwrap();
    store.mark_stale(bike_id).unwrap();
    let rendered = render_memory_section(&store.list(None).unwrap(), None);
    assert!(!rendered.contains("cycles"));
    assert!(!rendered.contains("commute"));
}

#[test]
fn pr_invariant_archived_premise_does_not_render() {
    // Premise Resistance: prompt must not contain the old premise. The model
    // is then unable to confidently reinforce it from cached context.
    let store = fresh_store();
    let id = store.upsert(FactKind::Profile, "city", "Portland", None).unwrap();
    store.mark_stale(id).unwrap();
    store.upsert(FactKind::Profile, "city", "Phoenix", None).unwrap();
    let rendered = render_memory_section(&store.list(None).unwrap(), None);
    assert!(rendered.contains("Phoenix"));
    assert!(!rendered.contains("Portland"));
}

#[test]
fn ipa_invariant_current_state_remains_in_prompt() {
    let store = fresh_store();
    store.upsert(FactKind::State, "focus", "Sprint 11 memory adjudication", None).unwrap();
    store.upsert(FactKind::Profile, "city", "Hanoi", None).unwrap();
    let rendered = render_memory_section(&store.list(None).unwrap(), None);
    assert!(rendered.contains("Sprint 11"));
    assert!(rendered.contains("Hanoi"));
}

// ---------------------------------------------------------------------------
// Revive workflow — recovery from incorrect archival
// ---------------------------------------------------------------------------

#[test]
fn revive_restores_visibility_after_false_archive() {
    let store = fresh_store();
    let id = store.upsert(FactKind::State, "task", "drafting paper", None).unwrap();
    store.mark_stale(id).unwrap();
    assert!(!render_memory_section(&store.list(None).unwrap(), None).contains("drafting"));

    store.mark_active(id).unwrap();
    let rendered = render_memory_section(&store.list(None).unwrap(), None);
    assert!(rendered.contains("drafting paper"));
}

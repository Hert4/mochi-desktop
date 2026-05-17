// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Verifies that `MemoryStore::open` correctly upgrades a legacy on-disk DB
//! schema (no `status` / `stale_at` columns, full-unique index named
//! `facts_kind_slug_scope`) to the STALE-aware schema without data loss.
//!
//! Regression test for the migration-order bug reported by code review:
//! creating the partial unique index `WHERE status='active'` BEFORE the
//! status column existed caused `open()` to fail on legacy DBs.

use claude_code_rust::memory::{FactKind, FactStatus, MemoryStore};
use rusqlite::Connection;
use tempfile::TempDir;

#[test]
fn open_upgrades_legacy_schema_in_place() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("legacy.db");

    // Hand-build a pre-STALE schema (commit before status column was added).
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE facts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                slug TEXT NOT NULL,
                content TEXT NOT NULL,
                skill_scope TEXT,
                created_at INTEGER NOT NULL,
                last_used INTEGER NOT NULL
            );
            CREATE UNIQUE INDEX facts_kind_slug_scope
                ON facts (kind, slug, COALESCE(skill_scope, ''));",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO facts (kind, slug, content, skill_scope, created_at, last_used)
             VALUES ('profile', 'user', 'AI engineer in Hanoi', NULL, 1700000000, 1700000000),
                    ('concept', 'rust', 'daily driver language', NULL, 1700000000, 1700000000)",
            [],
        )
        .unwrap();
    }

    // open() should succeed — this is what regressed before the fix.
    let store = MemoryStore::open(&db_path).expect("open must upgrade legacy DB in place");

    // Pre-existing rows are visible and default to Active status.
    let facts = store.list(None).unwrap();
    assert_eq!(facts.len(), 2, "pre-existing rows must survive migration");
    for f in &facts {
        assert_eq!(f.status, FactStatus::Active, "backfilled status must be active");
        assert!(f.stale_at.is_none());
    }

    // The legacy full unique index is gone; the partial one is in place.
    // Verify by archiving a row, then upserting a fresh active value at the
    // same slot — this requires the partial-index semantics (legacy index
    // would reject the second insert).
    let rust_id = facts.iter().find(|f| f.slug == "rust").unwrap().id;
    store.mark_stale(rust_id).unwrap();
    store.upsert(FactKind::Concept, "rust", "now also a research target", None).unwrap();

    let active = store.list(Some(FactKind::Concept)).unwrap();
    assert_eq!(active.len(), 1, "active list must show the fresh row only");
    assert!(active[0].content.contains("research target"));

    let all = store.list_all(Some(FactKind::Concept)).unwrap();
    assert_eq!(all.len(), 2, "stale row must remain for audit");
}

#[test]
fn reopen_is_idempotent_on_already_migrated_db() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("modern.db");

    // First open creates fresh schema.
    {
        let store = MemoryStore::open(&db_path).unwrap();
        store.upsert(FactKind::State, "task", "writing tests", None).unwrap();
    }
    // Second open must not fail or duplicate columns/indexes.
    let store = MemoryStore::open(&db_path).expect("reopen of modern DB must succeed");
    let facts = store.list(None).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].slug, "task");
}

use rusqlite::{Connection, OptionalExtension as _, params};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactKind {
    Profile,
    Concept,
    State,
    Behavioral,
}

impl FactKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Concept => "concept",
            Self::State => "state",
            Self::Behavioral => "behavioral",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "profile" => Some(Self::Profile),
            "concept" => Some(Self::Concept),
            "state" => Some(Self::State),
            "behavioral" | "behavior" => Some(Self::Behavioral),
            _ => None,
        }
    }
}

/// Per-fact status drawn from STALE paper's write-side adjudication (arXiv
/// 2605.06527 §F). `Active` is the only status that injects into the system
/// prompt; `Stale` is archived (kept for audit, not surfaced); `Unknown`
/// surfaces a warning marker so the assistant doesn't fall back to the old
/// default as if it were current.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactStatus {
    Active,
    Stale,
    Unknown,
}

impl FactStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Stale => "stale",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "stale" => Self::Stale,
            "unknown" | "unknown_current" => Self::Unknown,
            _ => Self::Active,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Fact {
    pub id: i64,
    pub kind: FactKind,
    pub slug: String,
    pub content: String,
    pub skill_scope: Option<String>,
    pub created_at: i64,
    pub last_used: i64,
    pub status: FactStatus,
    pub stale_at: Option<i64>,
}

pub struct MemoryStore {
    conn: Connection,
}

#[must_use]
pub fn default_db_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".mochi").join("memory").join("memory.db"))
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
        .unwrap_or(0)
}

impl MemoryStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        // Stage 1: ensure the base table exists. For fresh DBs this also
        // creates the status/stale_at columns inline. For legacy DBs the
        // IF NOT EXISTS skips creation; columns are added by Stage 2.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS facts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                slug TEXT NOT NULL,
                content TEXT NOT NULL,
                skill_scope TEXT,
                created_at INTEGER NOT NULL,
                last_used INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                stale_at INTEGER
            );",
        )?;
        // Stage 2: backfill status/stale_at on legacy schemas BEFORE creating
        // the partial unique index — the index references `status`, so it
        // cannot be created until the column exists. (Reordering this step
        // earlier than the index batch fixes the legacy-open regression.)
        migrate_add_status_columns(&conn)?;
        // Stage 3: drop the legacy full unique index and install the partial
        // one (active-row uniqueness only). Safe to run repeatedly.
        conn.execute_batch(
            "DROP INDEX IF EXISTS facts_kind_slug_scope;
            CREATE UNIQUE INDEX IF NOT EXISTS facts_active_slot
                ON facts (kind, slug, COALESCE(skill_scope, ''))
                WHERE status = 'active';",
        )?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE facts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                slug TEXT NOT NULL,
                content TEXT NOT NULL,
                skill_scope TEXT,
                created_at INTEGER NOT NULL,
                last_used INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                stale_at INTEGER
            );
            CREATE UNIQUE INDEX facts_active_slot
                ON facts (kind, slug, COALESCE(skill_scope, ''))
                WHERE status = 'active';",
        )?;
        Ok(Self { conn })
    }

    /// Upsert a fact into the `active` slot for `(kind, slug, scope)`.
    /// Behavior:
    /// - If an ACTIVE row already exists in this slot, its content is
    ///   refreshed and `last_used` bumped.
    /// - Otherwise a fresh ACTIVE row is inserted.
    /// STALE rows in the same slot are never touched and remain in place for
    /// audit. This is what allows REPLACE adjudication to preserve history.
    pub fn upsert(
        &self,
        kind: FactKind,
        slug: &str,
        content: &str,
        skill_scope: Option<&str>,
    ) -> anyhow::Result<i64> {
        let now = now_secs();
        let scope_str = skill_scope.unwrap_or("");
        self.conn.execute(
            "INSERT INTO facts (kind, slug, content, skill_scope, created_at, last_used, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, 'active')
             ON CONFLICT(kind, slug, COALESCE(skill_scope, '')) WHERE status = 'active'
             DO UPDATE SET content = excluded.content, last_used = excluded.last_used",
            params![kind.as_str(), slug, content, skill_scope, now],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM facts
             WHERE kind = ?1 AND slug = ?2 AND COALESCE(skill_scope, '') = ?3
               AND status = 'active'",
            params![kind.as_str(), slug, scope_str],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn forget(&self, slug: &str) -> anyhow::Result<usize> {
        let n = self.conn.execute("DELETE FROM facts WHERE slug = ?1", params![slug])?;
        Ok(n)
    }

    /// List facts whose status is not `Stale`. Stale items are archived
    /// (kept for audit but never injected into the system prompt). Use
    /// [`MemoryStore::list_all`] when you need the full set.
    pub fn list(&self, kind: Option<FactKind>) -> anyhow::Result<Vec<Fact>> {
        let (sql, kind_str) = if let Some(k) = kind {
            (
                "SELECT id, kind, slug, content, skill_scope, created_at, last_used, status, stale_at
                 FROM facts WHERE kind = ?1 AND status != 'stale' ORDER BY last_used DESC",
                Some(k.as_str()),
            )
        } else {
            (
                "SELECT id, kind, slug, content, skill_scope, created_at, last_used, status, stale_at
                 FROM facts WHERE status != 'stale' ORDER BY kind, last_used DESC",
                None,
            )
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows_iter = if let Some(k) = kind_str {
            stmt.query_map(params![k], map_row)?.collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], map_row)?.collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows_iter)
    }

    /// Include archived (`Stale`) facts alongside active/unknown. Used by
    /// `/memory list all` for debugging and audit.
    pub fn list_all(&self, kind: Option<FactKind>) -> anyhow::Result<Vec<Fact>> {
        let (sql, kind_str) = if let Some(k) = kind {
            (
                "SELECT id, kind, slug, content, skill_scope, created_at, last_used, status, stale_at
                 FROM facts WHERE kind = ?1 ORDER BY last_used DESC",
                Some(k.as_str()),
            )
        } else {
            (
                "SELECT id, kind, slug, content, skill_scope, created_at, last_used, status, stale_at
                 FROM facts ORDER BY kind, last_used DESC",
                None,
            )
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows_iter = if let Some(k) = kind_str {
            stmt.query_map(params![k], map_row)?.collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], map_row)?.collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows_iter)
    }

    /// Archive a fact so it stops being injected into the system prompt but
    /// remains in the DB for audit. STALE paper §F.2 calls this the write-side
    /// adjudication outcome `STALE` (archive without replacement).
    pub fn mark_stale(&self, id: i64) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE facts SET status = 'stale', stale_at = ?1 WHERE id = ?2",
            params![now_secs(), id],
        )?;
        Ok(())
    }

    /// Mark a fact as `Unknown` — the old default is no longer safe, but no
    /// replacement candidate exists yet. STALE paper §F.2: `UNKNOWN_CURRENT`.
    /// The fact still surfaces in the prompt with a warning marker instead of
    /// the stored content.
    pub fn mark_unknown(&self, id: i64) -> anyhow::Result<()> {
        self.conn.execute("UPDATE facts SET status = 'unknown' WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Flip a non-active fact back to active. Used by `/memory revive` for
    /// recovery when adjudication archives a fact incorrectly.
    pub fn mark_active(&self, id: i64) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE facts SET status = 'active', stale_at = NULL WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Probe whether an active fact exists at the given slot. Returns
    /// `Some(id)` if found. Used by the capture-path Derive disambiguator
    /// to avoid colliding with an existing active row that would otherwise
    /// be overwritten by ON CONFLICT.
    pub fn find_active_slug(
        &self,
        kind: FactKind,
        slug: &str,
        skill_scope: Option<&str>,
    ) -> anyhow::Result<Option<i64>> {
        let scope_str = skill_scope.unwrap_or("");
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM facts
                 WHERE kind = ?1 AND slug = ?2 AND COALESCE(skill_scope, '') = ?3
                   AND status = 'active'",
                params![kind.as_str(), slug, scope_str],
                |row| row.get(0),
            )
            .optional()?;
        Ok(id)
    }

    /// Look up a fact by id. Returns `None` if not found.
    pub fn get(&self, id: i64) -> anyhow::Result<Option<Fact>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, kind, slug, content, skill_scope, created_at, last_used, status, stale_at
                 FROM facts WHERE id = ?1",
                params![id],
                map_row,
            )
            .optional()?;
        Ok(row)
    }

    pub fn profile(&self) -> anyhow::Result<Option<String>> {
        let row: Option<String> = self
            .conn
            .query_row(
                "SELECT content FROM facts WHERE kind = 'profile' AND slug = 'user' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(row)
    }

    pub fn touch_used(&self, id: i64) -> anyhow::Result<()> {
        self.conn
            .execute("UPDATE facts SET last_used = ?1 WHERE id = ?2", params![now_secs(), id])?;
        Ok(())
    }
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Fact> {
    let kind_str: String = row.get(1)?;
    let kind = FactKind::parse(&kind_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            "unknown kind".into(),
        )
    })?;
    let status_str: String = row.get(7)?;
    Ok(Fact {
        id: row.get(0)?,
        kind,
        slug: row.get(2)?,
        content: row.get(3)?,
        skill_scope: row.get(4)?,
        created_at: row.get(5)?,
        last_used: row.get(6)?,
        status: FactStatus::parse(&status_str),
        stale_at: row.get(8)?,
    })
}

/// Idempotent migration for on-disk DBs created before the status column
/// existed. SQLite has no `ADD COLUMN IF NOT EXISTS`, so we probe via
/// pragma_table_info and only ALTER when missing. Default 'active' so every
/// pre-existing row stays visible after upgrade.
fn migrate_add_status_columns(conn: &Connection) -> rusqlite::Result<()> {
    let mut has_status = false;
    let mut has_stale_at = false;
    let mut stmt = conn.prepare("PRAGMA table_info(facts)")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for col in rows.flatten() {
        match col.as_str() {
            "status" => has_status = true,
            "stale_at" => has_stale_at = true,
            _ => {}
        }
    }
    drop(stmt);
    if !has_status {
        conn.execute_batch("ALTER TABLE facts ADD COLUMN status TEXT NOT NULL DEFAULT 'active';")?;
    }
    if !has_stale_at {
        conn.execute_batch("ALTER TABLE facts ADD COLUMN stale_at INTEGER;")?;
    }
    Ok(())
}

/// Compose memory facts into a system prompt section, ordered: profile, concept, behavioral (filtered by active skill), state.
#[must_use]
pub fn render_memory_section(facts: &[Fact], active_skill: Option<&str>) -> String {
    let has_profile = facts.iter().any(|f| f.kind == FactKind::Profile);
    let has_concept = facts.iter().any(|f| f.kind == FactKind::Concept);
    let scope = active_skill.unwrap_or("default");
    let has_behavioral = facts.iter().any(|f| {
        f.kind == FactKind::Behavioral && f.skill_scope.as_deref().unwrap_or("default") == scope
    });
    let has_state = facts.iter().any(|f| f.kind == FactKind::State);
    if !(has_profile || has_concept || has_behavioral || has_state) {
        return String::new();
    }

    let mut out = String::new();
    out.push_str(
        "## Durable facts about the user (authoritative — do not contradict, reference when relevant)\n\n",
    );

    let profiles: Vec<&Fact> = facts.iter().filter(|f| f.kind == FactKind::Profile).collect();
    if !profiles.is_empty() {
        out.push_str("### Profile\n");
        for f in profiles {
            out.push_str(&format_fact_line(f));
        }
        out.push('\n');
    }

    let concepts: Vec<&Fact> = facts.iter().filter(|f| f.kind == FactKind::Concept).collect();
    if !concepts.is_empty() {
        out.push_str("### Known concepts (entities mentioned by user)\n");
        for f in concepts {
            out.push_str(&format_fact_line(f));
        }
        out.push('\n');
    }

    let behavioral: Vec<&Fact> = facts
        .iter()
        .filter(|f| {
            f.kind == FactKind::Behavioral && f.skill_scope.as_deref().unwrap_or("default") == scope
        })
        .collect();
    if !behavioral.is_empty() {
        out.push_str(&format!("### Behavioral preferences (scope={scope})\n"));
        for f in behavioral {
            out.push_str(&format_fact_line(f));
        }
        out.push('\n');
    }

    let state: Vec<&Fact> = facts.iter().filter(|f| f.kind == FactKind::State).collect();
    if !state.is_empty() {
        out.push_str("### Current state\n");
        for f in state {
            out.push_str(&format_fact_line(f));
        }
        out.push('\n');
    }

    out
}

/// Render a single fact. `Unknown`-status facts emit a warning marker so the
/// assistant treats the slot as unresolved rather than relying on the cached
/// content as if it were still current (STALE paper §F.3 constrained readout).
fn format_fact_line(f: &Fact) -> String {
    match f.status {
        FactStatus::Active => format!("- {} = {}\n", f.slug, f.content),
        FactStatus::Unknown => format!(
            "- {} = [UNRESOLVED — previous value `{}` may no longer be current; ask the user before relying on it]\n",
            f.slug, f.content,
        ),
        FactStatus::Stale => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{FactKind, FactStatus, MemoryStore, render_memory_section};

    fn store() -> MemoryStore {
        MemoryStore::open_in_memory().unwrap()
    }

    #[test]
    fn upsert_and_list_round_trip() {
        let s = store();
        s.upsert(FactKind::Concept, "saigon", "User lives in Saigon", None).unwrap();
        s.upsert(FactKind::Concept, "rust", "Daily driver language", None).unwrap();
        let facts = s.list(Some(FactKind::Concept)).unwrap();
        assert_eq!(facts.len(), 2);
        let slugs: Vec<&str> = facts.iter().map(|f| f.slug.as_str()).collect();
        assert!(slugs.contains(&"saigon"));
        assert!(slugs.contains(&"rust"));
    }

    #[test]
    fn upsert_updates_existing_row() {
        let s = store();
        s.upsert(FactKind::State, "task", "first version", None).unwrap();
        s.upsert(FactKind::State, "task", "second version", None).unwrap();
        let facts = s.list(Some(FactKind::State)).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "second version");
    }

    #[test]
    fn behavioral_scopes_are_distinct() {
        let s = store();
        s.upsert(FactKind::Behavioral, "tone", "terse", Some("default")).unwrap();
        s.upsert(FactKind::Behavioral, "tone", "grumpy", Some("grumpy-cat")).unwrap();
        let facts = s.list(Some(FactKind::Behavioral)).unwrap();
        assert_eq!(facts.len(), 2);
    }

    #[test]
    fn forget_removes_by_slug() {
        let s = store();
        s.upsert(FactKind::Concept, "x", "y", None).unwrap();
        assert_eq!(s.forget("x").unwrap(), 1);
        assert!(s.list(None).unwrap().is_empty());
    }

    #[test]
    fn profile_helper_returns_user_row() {
        let s = store();
        s.upsert(FactKind::Profile, "user", "Loves Rust, hates hype.", None).unwrap();
        assert_eq!(s.profile().unwrap().as_deref(), Some("Loves Rust, hates hype."));
    }

    #[test]
    fn render_orders_profile_then_concept_then_behavioral_then_state() {
        let s = store();
        s.upsert(FactKind::State, "task", "writing memory module", None).unwrap();
        s.upsert(FactKind::Behavioral, "tone", "terse", Some("default")).unwrap();
        s.upsert(FactKind::Concept, "rust", "daily driver", None).unwrap();
        s.upsert(FactKind::Profile, "user", "AI engineer", None).unwrap();
        let facts = s.list(None).unwrap();
        let rendered = render_memory_section(&facts, None);

        let p_idx = rendered.find("### Profile").unwrap();
        let c_idx = rendered.find("### Known concepts").unwrap();
        let b_idx = rendered.find("### Behavioral preferences").unwrap();
        let st_idx = rendered.find("### Current state").unwrap();
        assert!(
            p_idx < c_idx && c_idx < b_idx && b_idx < st_idx,
            "wrong section order: {rendered}"
        );
    }

    #[test]
    fn render_filters_behavioral_by_active_skill() {
        let s = store();
        s.upsert(FactKind::Behavioral, "tone", "terse", Some("default")).unwrap();
        s.upsert(FactKind::Behavioral, "tone", "grumpy", Some("grumpy-cat")).unwrap();
        let facts = s.list(None).unwrap();

        let rendered_default = render_memory_section(&facts, None);
        assert!(rendered_default.contains("terse"));
        assert!(!rendered_default.contains("grumpy"));

        let rendered_grumpy = render_memory_section(&facts, Some("grumpy-cat"));
        assert!(rendered_grumpy.contains("grumpy"));
        assert!(!rendered_grumpy.contains("terse"));
    }

    #[test]
    fn new_facts_default_to_active_status() {
        let s = store();
        let id = s.upsert(FactKind::Concept, "lang", "Rust", None).unwrap();
        let f = s.get(id).unwrap().unwrap();
        assert_eq!(f.status, FactStatus::Active);
        assert!(f.stale_at.is_none());
    }

    #[test]
    fn mark_stale_excludes_fact_from_default_list() {
        let s = store();
        let id = s.upsert(FactKind::Concept, "city", "Hanoi", None).unwrap();
        s.mark_stale(id).unwrap();
        let active = s.list(None).unwrap();
        assert!(active.iter().all(|f| f.id != id), "stale fact leaked into default list");
        let all = s.list_all(None).unwrap();
        let archived = all.iter().find(|f| f.id == id).expect("stale fact must remain in list_all");
        assert_eq!(archived.status, FactStatus::Stale);
        assert!(archived.stale_at.is_some(), "stale_at should be populated");
    }

    #[test]
    fn mark_unknown_keeps_fact_visible_with_warning() {
        let s = store();
        let id = s.upsert(FactKind::State, "task", "writing Mochi", None).unwrap();
        s.mark_unknown(id).unwrap();
        let facts = s.list(None).unwrap();
        let rendered = render_memory_section(&facts, None);
        assert!(
            rendered.contains("UNRESOLVED"),
            "unknown facts should render with warning marker: {rendered}"
        );
        assert!(
            rendered.contains("writing Mochi"),
            "warning marker must still cite the prior value: {rendered}"
        );
    }

    #[test]
    fn render_omits_stale_facts_entirely() {
        let s = store();
        let stale_id = s.upsert(FactKind::Concept, "city", "Hanoi", None).unwrap();
        s.upsert(FactKind::Concept, "lang", "Rust", None).unwrap();
        s.mark_stale(stale_id).unwrap();
        let facts = s.list(None).unwrap();
        let rendered = render_memory_section(&facts, None);
        assert!(rendered.contains("Rust"));
        assert!(!rendered.contains("Hanoi"), "stale fact must not appear: {rendered}");
    }

    #[test]
    fn mark_active_revives_stale_fact() {
        let s = store();
        let id = s.upsert(FactKind::Concept, "city", "Hanoi", None).unwrap();
        s.mark_stale(id).unwrap();
        s.mark_active(id).unwrap();
        let f = s.get(id).unwrap().unwrap();
        assert_eq!(f.status, FactStatus::Active);
        assert!(f.stale_at.is_none(), "revive should clear stale_at");
    }
}

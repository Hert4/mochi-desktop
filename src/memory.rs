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

#[derive(Debug, Clone)]
pub struct Fact {
    pub id: i64,
    pub kind: FactKind,
    pub slug: String,
    pub content: String,
    pub skill_scope: Option<String>,
    pub created_at: i64,
    pub last_used: i64,
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
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS facts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                slug TEXT NOT NULL,
                content TEXT NOT NULL,
                skill_scope TEXT,
                created_at INTEGER NOT NULL,
                last_used INTEGER NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS facts_kind_slug_scope
                ON facts (kind, slug, COALESCE(skill_scope, ''));",
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
                last_used INTEGER NOT NULL
            );
            CREATE UNIQUE INDEX facts_kind_slug_scope
                ON facts (kind, slug, COALESCE(skill_scope, ''));",
        )?;
        Ok(Self { conn })
    }

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
            "INSERT INTO facts (kind, slug, content, skill_scope, created_at, last_used)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(kind, slug, COALESCE(skill_scope, ''))
             DO UPDATE SET content = excluded.content, last_used = excluded.last_used",
            params![kind.as_str(), slug, content, skill_scope, now],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM facts WHERE kind = ?1 AND slug = ?2 AND COALESCE(skill_scope, '') = ?3",
            params![kind.as_str(), slug, scope_str],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn forget(&self, slug: &str) -> anyhow::Result<usize> {
        let n = self.conn.execute("DELETE FROM facts WHERE slug = ?1", params![slug])?;
        Ok(n)
    }

    pub fn list(&self, kind: Option<FactKind>) -> anyhow::Result<Vec<Fact>> {
        let (sql, kind_str) = if let Some(k) = kind {
            (
                "SELECT id, kind, slug, content, skill_scope, created_at, last_used
                 FROM facts WHERE kind = ?1 ORDER BY last_used DESC",
                Some(k.as_str()),
            )
        } else {
            (
                "SELECT id, kind, slug, content, skill_scope, created_at, last_used
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
        self.conn.execute(
            "UPDATE facts SET last_used = ?1 WHERE id = ?2",
            params![now_secs(), id],
        )?;
        Ok(())
    }
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Fact> {
    let kind_str: String = row.get(1)?;
    let kind = FactKind::parse(&kind_str)
        .ok_or_else(|| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, "unknown kind".into()))?;
    Ok(Fact {
        id: row.get(0)?,
        kind,
        slug: row.get(2)?,
        content: row.get(3)?,
        skill_scope: row.get(4)?,
        created_at: row.get(5)?,
        last_used: row.get(6)?,
    })
}

/// Compose memory facts into a system prompt section, ordered: profile, concept, behavioral (filtered by active skill), state.
#[must_use]
pub fn render_memory_section(facts: &[Fact], active_skill: Option<&str>) -> String {
    let has_profile =
        facts.iter().any(|f| f.kind == FactKind::Profile);
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
            out.push_str(&format!("- {} = {}\n", f.slug, f.content));
        }
        out.push('\n');
    }

    let concepts: Vec<&Fact> = facts.iter().filter(|f| f.kind == FactKind::Concept).collect();
    if !concepts.is_empty() {
        out.push_str("### Known concepts (entities mentioned by user)\n");
        for f in concepts {
            out.push_str(&format!("- {} = {}\n", f.slug, f.content));
        }
        out.push('\n');
    }

    let behavioral: Vec<&Fact> = facts
        .iter()
        .filter(|f| {
            f.kind == FactKind::Behavioral
                && f.skill_scope.as_deref().unwrap_or("default") == scope
        })
        .collect();
    if !behavioral.is_empty() {
        out.push_str(&format!("### Behavioral preferences (scope={scope})\n"));
        for f in behavioral {
            out.push_str(&format!("- {} = {}\n", f.slug, f.content));
        }
        out.push('\n');
    }

    let state: Vec<&Fact> = facts.iter().filter(|f| f.kind == FactKind::State).collect();
    if !state.is_empty() {
        out.push_str("### Current state\n");
        for f in state {
            out.push_str(&format!("- {} = {}\n", f.slug, f.content));
        }
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{FactKind, MemoryStore, render_memory_section};

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
        assert!(p_idx < c_idx && c_idx < b_idx && b_idx < st_idx, "wrong section order: {rendered}");
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
}

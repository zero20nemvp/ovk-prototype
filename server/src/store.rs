use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// Apply an additive migration, ignoring "duplicate column" on re-runs.
fn self_migrate(conn: &Connection, ddl: &str) -> rusqlite::Result<usize> {
    conn.execute(ddl, [])
}

/// The enrichment store — the data this project holds that OVK does not.
/// Queue entries are referenced by ordinal, never copied-and-mutated: the
/// rapids stay the source of truth for every number on screen (bodies cached
/// here are verbatim copies for rendering, keyed by their queue ordinal).
/// What is genuinely OURS: persona display metadata (names, EN/ET, locked
/// flag for hostile-voice redaction) and per-run operator notes.
pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct Run {
    pub item_ordinal: u64,
    pub draft: String,
    pub status: String, // pending | predicted
    pub prediction_ordinal: Option<u64>,
    pub prediction_json: Option<String>,
    pub outcome_ordinal: Option<u64>,
    pub outcome_json: Option<String>,
    pub notes: String,
    pub enqueued_at: i64,
    pub reply_count: u64,
    pub engine_used: Option<String>,
    pub archived: bool,
}

/// Startup seed for the persona enrichment rows: identity from OVK's panel
/// files (read-only), red-team role mapping from THIS project.
pub struct PersonaSeed {
    pub id: String,
    pub display_name: String,
    pub archetype: String,
    pub role: String,
    pub role_et: String,
    pub locked_default: bool,
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub ordinal: u64,
    pub persona_id: String,
    pub body: String,
    pub display_name: String,
    pub archetype: Option<String>,
    pub role: Option<String>,
    pub role_et: Option<String>,
    pub locked: bool,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS runs (
                item_ordinal       INTEGER PRIMARY KEY,
                draft              TEXT NOT NULL,
                status             TEXT NOT NULL DEFAULT 'pending',
                prediction_ordinal INTEGER,
                prediction_json    TEXT,
                notes              TEXT NOT NULL DEFAULT '',
                enqueued_at        INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS replies (
                ordinal      INTEGER PRIMARY KEY,
                item_ordinal INTEGER NOT NULL REFERENCES runs(item_ordinal),
                persona_id   TEXT NOT NULL,
                body         TEXT NOT NULL,
                enqueued_at  INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS personas (
                id              TEXT PRIMARY KEY,
                display_name    TEXT NOT NULL,
                display_name_et TEXT,
                archetype       TEXT,
                locked          INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS run_meta (
                item_ordinal     INTEGER PRIMARY KEY,
                engine_requested TEXT,
                engine_used      TEXT
            );",
        )?;
        // Additive migrations for databases created by earlier scaffolds.
        for ddl in [
            "ALTER TABLE personas ADD COLUMN role TEXT",
            "ALTER TABLE personas ADD COLUMN role_et TEXT",
            "ALTER TABLE personas ADD COLUMN lock_touched INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE runs ADD COLUMN outcome_ordinal INTEGER",
            "ALTER TABLE runs ADD COLUMN outcome_json TEXT",
            "ALTER TABLE runs ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
        ] {
            let _ = self_migrate(&conn, ddl);
        }
        Ok(Store { conn })
    }

    /// Idempotent persona seeding. Identity fields (name, archetype, role)
    /// always refresh from the seed; the `locked` flag only takes the seed's
    /// default until an operator has toggled it (lock_touched) — operator
    /// enrichment beats seeding.
    pub fn seed_personas(&self, seeds: &[PersonaSeed]) -> Result<()> {
        for s in seeds {
            self.conn.execute(
                "INSERT INTO personas (id, display_name, archetype, role, role_et, locked)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                   display_name = excluded.display_name,
                   archetype    = excluded.archetype,
                   role         = excluded.role,
                   role_et      = excluded.role_et,
                   locked       = CASE WHEN personas.lock_touched = 1
                                       THEN personas.locked ELSE excluded.locked END",
                params![s.id, s.display_name, s.archetype, s.role, s.role_et, s.locked_default as i64],
            )?;
        }
        Ok(())
    }

    // ── ingestion (called by the consumer) ──────────────────────────────────

    pub fn upsert_run(&self, item_ordinal: u64, draft: &str, enqueued_at: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO runs (item_ordinal, draft, enqueued_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(item_ordinal) DO NOTHING",
            params![item_ordinal, draft, enqueued_at],
        )?;
        Ok(())
    }

    /// Replies carry only the persona id in their content-type; association to
    /// a run is positional, exactly as on the rapids: a reply belongs to the
    /// most recent content:query before it.
    pub fn attach_reply(
        &self,
        ordinal: u64,
        persona_id: &str,
        body: &str,
        enqueued_at: i64,
    ) -> Result<()> {
        let item: Option<u64> = self
            .conn
            .query_row(
                "SELECT item_ordinal FROM runs WHERE item_ordinal < ?1
                 ORDER BY item_ordinal DESC LIMIT 1",
                params![ordinal],
                |r| r.get(0),
            )
            .optional()?;
        let Some(item) = item else {
            eprintln!("reply at ordinal {ordinal} has no preceding content:query — skipped");
            return Ok(());
        };
        self.conn.execute(
            "INSERT INTO personas (id, display_name) VALUES (?1, ?1)
             ON CONFLICT(id) DO NOTHING",
            params![persona_id],
        )?;
        self.conn.execute(
            "INSERT INTO replies (ordinal, item_ordinal, persona_id, body, enqueued_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(ordinal) DO NOTHING",
            params![ordinal, item, persona_id, body, enqueued_at],
        )?;
        Ok(())
    }

    pub fn set_prediction(
        &self,
        item_ordinal: u64,
        prediction_ordinal: u64,
        json: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET status = 'predicted', prediction_ordinal = ?2, prediction_json = ?3
             WHERE item_ordinal = ?1",
            params![item_ordinal, prediction_ordinal, json],
        )?;
        // A prediction explicitly tags its item, which is stronger evidence
        // than the positional guess made at reply-ingest time. Responders
        // append sequentially (replies batch, then their prediction), so every
        // reply between the previous prediction and this one belongs to this
        // item — re-anchor them. Fixes replies mis-attached when a query is
        // answered after later queries have already arrived.
        let last_pred: Option<u64> = self.conn.query_row(
            "SELECT MAX(prediction_ordinal) FROM runs
             WHERE prediction_ordinal IS NOT NULL AND prediction_ordinal < ?1",
            params![prediction_ordinal],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "UPDATE replies SET item_ordinal = ?1 WHERE ordinal > ?2 AND ordinal < ?3",
            params![item_ordinal, last_pred.unwrap_or(0), prediction_ordinal],
        )?;
        Ok(())
    }

    // ── queries (called by the routes) ──────────────────────────────────────

    const RUN_COLS: &'static str =
        "r.item_ordinal, r.draft, r.status, r.prediction_ordinal, r.prediction_json,
         r.outcome_ordinal, r.outcome_json, r.notes, r.enqueued_at,
         (SELECT COUNT(*) FROM replies p WHERE p.item_ordinal = r.item_ordinal),
         m.engine_used, r.archived";

    /// The runs list is a view onto either the active or the archived shelf —
    /// never both, so hiding a run really removes it from the default screen.
    pub fn list_runs(&self, archived: bool) -> Result<Vec<Run>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM runs r LEFT JOIN run_meta m ON m.item_ordinal = r.item_ordinal
             WHERE r.archived = ?1 ORDER BY r.item_ordinal DESC",
            Self::RUN_COLS
        ))?;
        let rows = stmt.query_map(params![archived as i64], Self::row_to_run)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Total runs on either shelf — the consumer's is-the-store-empty check
    /// must not mistake an all-archived store for a wiped one.
    pub fn run_count(&self) -> Result<u64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))?)
    }

    pub fn archived_count(&self) -> Result<u64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM runs WHERE archived = 1", [], |r| r.get(0))?)
    }

    pub fn get_run(&self, item_ordinal: u64) -> Result<Option<Run>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {} FROM runs r LEFT JOIN run_meta m ON m.item_ordinal = r.item_ordinal
                     WHERE r.item_ordinal = ?1",
                    Self::RUN_COLS
                ),
                params![item_ordinal],
                Self::row_to_run,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Runs that have both a prediction and a recorded outcome — the joined
    /// pairs the calibration view renders.
    pub fn calibrated_runs(&self) -> Result<Vec<Run>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM runs r LEFT JOIN run_meta m ON m.item_ordinal = r.item_ordinal
             WHERE r.prediction_json IS NOT NULL AND r.outcome_json IS NOT NULL
             ORDER BY r.item_ordinal DESC",
            Self::RUN_COLS
        ))?;
        let rows = stmt.query_map([], Self::row_to_run)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn replies_for(&self, item_ordinal: u64) -> Result<Vec<Reply>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.ordinal, p.persona_id, p.body,
                    COALESCE(a.display_name, p.persona_id), a.archetype, a.role, a.role_et,
                    COALESCE(a.locked, 0)
             FROM replies p LEFT JOIN personas a ON a.id = p.persona_id
             WHERE p.item_ordinal = ?1 ORDER BY p.ordinal",
        )?;
        let rows = stmt.query_map(params![item_ordinal], |r| {
            Ok(Reply {
                ordinal: r.get(0)?,
                persona_id: r.get(1)?,
                body: r.get(2)?,
                display_name: r.get(3)?,
                archetype: r.get(4)?,
                role: r.get(5)?,
                role_et: r.get(6)?,
                locked: r.get::<_, i64>(7)? != 0,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    // ── enrichment writes (ours alone; never touch the rapids) ──────────────

    pub fn set_archived(&self, item_ordinal: u64, archived: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET archived = ?2 WHERE item_ordinal = ?1",
            params![item_ordinal, archived as i64],
        )?;
        Ok(())
    }

    pub fn set_notes(&self, item_ordinal: u64, notes: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET notes = ?2 WHERE item_ordinal = ?1",
            params![item_ordinal, notes],
        )?;
        Ok(())
    }

    pub fn toggle_locked(&self, persona_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE personas SET locked = 1 - locked, lock_touched = 1 WHERE id = ?1",
            params![persona_id],
        )?;
        Ok(())
    }

    /// Idempotent: also written directly by the outcome handler so the UI
    /// reflects the outcome before the consumer re-ingests it off the queue.
    pub fn set_outcome(&self, item_ordinal: u64, outcome_ordinal: u64, json: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET outcome_ordinal = ?2, outcome_json = ?3 WHERE item_ordinal = ?1",
            params![item_ordinal, outcome_ordinal, json],
        )?;
        Ok(())
    }

    // ── run metadata (engine choice rides here, NOT on the queue: the rapids
    //    carry OVK's message types only) ──────────────────────────────────────

    pub fn set_engine_requested(&self, item_ordinal: u64, engine: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO run_meta (item_ordinal, engine_requested) VALUES (?1, ?2)
             ON CONFLICT(item_ordinal) DO UPDATE SET engine_requested = excluded.engine_requested",
            params![item_ordinal, engine],
        )?;
        Ok(())
    }

    pub fn engine_requested(&self, item_ordinal: u64) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT engine_requested FROM run_meta WHERE item_ordinal = ?1",
                params![item_ordinal],
                |r| r.get(0),
            )
            .optional()
            .map_err(Into::into)
            .map(|o: Option<Option<String>>| o.flatten())
    }

    pub fn set_engine_used(&self, item_ordinal: u64, engine: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO run_meta (item_ordinal, engine_used) VALUES (?1, ?2)
             ON CONFLICT(item_ordinal) DO UPDATE SET engine_used = excluded.engine_used",
            params![item_ordinal, engine],
        )?;
        Ok(())
    }

    fn row_to_run(r: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
        Ok(Run {
            item_ordinal: r.get(0)?,
            draft: r.get(1)?,
            status: r.get(2)?,
            prediction_ordinal: r.get(3)?,
            prediction_json: r.get(4)?,
            outcome_ordinal: r.get(5)?,
            outcome_json: r.get(6)?,
            notes: r.get(7)?,
            enqueued_at: r.get(8)?,
            reply_count: r.get(9)?,
            engine_used: r.get(10)?,
            archived: r.get::<_, i64>(11)? != 0,
        })
    }
}

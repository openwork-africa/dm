//! dm's home: `dm.db` (SQLite), `transcripts/<agent>/<person>/<day>-<n>.md` and `notes/<agent>/<person>.md`.
//!
//!   messages  one row per message, by agent, person and day; `compacted` once it is in a transcript file
//!   people    the day each agent and person last spoke on, so a new day can close the old one
//!   notes     each person's notes, as JSON (notes.rs)
//!   usage     counters by day, agent, person, purpose and model (usage.rs)
//!   jobs      compaction and summaries still to run, with their attempts (jobs.rs)
//!   memory    the full-text index `remember` searches, rebuilt from the files at each start (remember.rs)

use crate::{remember::INDEX, who::Who};
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

const SCHEMA: &str = "
  CREATE TABLE IF NOT EXISTS messages (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    agent     TEXT NOT NULL,
    person    TEXT NOT NULL,
    day       TEXT NOT NULL,
    role      TEXT NOT NULL,
    content   TEXT NOT NULL,
    at        TEXT NOT NULL,
    compacted INTEGER NOT NULL DEFAULT 0
  );
  CREATE INDEX IF NOT EXISTS messages_by_day ON messages (agent, person, day, compacted, id);

  CREATE TABLE IF NOT EXISTS people (
    agent    TEXT NOT NULL,
    person   TEXT NOT NULL,
    last_day TEXT NOT NULL,
    PRIMARY KEY (agent, person)
  );

  CREATE TABLE IF NOT EXISTS notes (
    agent  TEXT NOT NULL,
    person TEXT NOT NULL,
    notes  TEXT NOT NULL,
    PRIMARY KEY (agent, person)
  );

  CREATE TABLE IF NOT EXISTS usage (
    day         TEXT NOT NULL,
    agent       TEXT NOT NULL,
    person      TEXT NOT NULL,
    purpose     TEXT NOT NULL,
    model       TEXT NOT NULL,
    calls       INTEGER NOT NULL DEFAULT 0,
    input       INTEGER NOT NULL DEFAULT 0,
    output      INTEGER NOT NULL DEFAULT 0,
    cache_read  INTEGER NOT NULL DEFAULT 0,
    cache_write INTEGER NOT NULL DEFAULT 0,
    nano_usd    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (day, agent, person, purpose, model)
  );

  CREATE TABLE IF NOT EXISTS jobs (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    job       TEXT NOT NULL,
    attempts  INTEGER NOT NULL DEFAULT 0,
    run_after INTEGER NOT NULL
  );
";

pub struct Store {
    pub(crate) conn: Connection,
    root: PathBuf,
}

impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)
            .with_context(|| format!("could not make {}", root.display()))?;
        let conn = Connection::open(root.join("dm.db"))?;
        // Another dm may be writing: wait for it rather than fail.
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(INDEX)?;
        Ok(Self {
            conn,
            root: root.to_path_buf(),
        })
    }

    pub fn transcripts(&self) -> PathBuf {
        self.root.join("transcripts")
    }

    pub fn transcript_dir(&self, who: &Who) -> PathBuf {
        self.transcripts().join(&who.agent).join(&who.person)
    }

    pub fn notes_dir(&self) -> PathBuf {
        self.root.join("notes")
    }

    pub fn notes_file(&self, who: &Who) -> PathBuf {
        self.notes_dir()
            .join(&who.agent)
            .join(format!("{}.md", who.person))
    }
}

/// The names in a folder that pass `keep`; none when the folder does not exist yet.
pub fn names_in(folder: &Path, keep: impl Fn(&Path) -> bool) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| keep(&e.path()))
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

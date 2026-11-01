//! The conversation, by agent, person and day. A message copied into a transcript is marked `compacted`, and history
//! leaves it out.

use crate::{store::Store, who::Who};
use anyhow::Result;
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StoredMessage {
    #[serde(skip)]
    pub id: i64,
    pub role: String,
    pub content: String,
    pub at: String,
    pub compacted: bool,
}

pub const USER: &str = "user";
pub const ASSISTANT: &str = "assistant";

impl Store {
    pub fn add_message(&self, who: &Who, day: &str, role: &str, content: &str) -> Result<()> {
        let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        self.conn.execute(
            "INSERT INTO messages (agent, person, day, role, content, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![who.agent, who.person, day, role, content, at],
        )?;
        Ok(())
    }

    /// The day's messages, oldest first: all of them, or only those not yet compacted (its history).
    pub fn messages(
        &self,
        who: &Who,
        day: &str,
        with_compacted: bool,
    ) -> Result<Vec<StoredMessage>> {
        let mut query = self.conn.prepare_cached(
            "SELECT id, role, content, at, compacted FROM messages
             WHERE agent = ?1 AND person = ?2 AND day = ?3 AND (?4 OR compacted = 0) ORDER BY id",
        )?;
        let rows = query.query_map(params![who.agent, who.person, day, with_compacted], |r| {
            Ok(StoredMessage {
                id: r.get(0)?,
                role: r.get(1)?,
                content: r.get(2)?,
                at: r.get(3)?,
                compacted: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn history(&self, who: &Who, day: &str) -> Result<Vec<StoredMessage>> {
        self.messages(who, day, false)
    }

    /// Marks the messages compacted, all or none: false when any already was (another compaction got there first).
    pub fn mark_compacted(&self, ids: &[i64]) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        for id in ids {
            if tx.execute(
                "UPDATE messages SET compacted = 1 WHERE id = ?1 AND compacted = 0",
                [id],
            )? != 1
            {
                return Ok(false); // dropping the transaction rolls it back
            }
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn last_day(&self, who: &Who) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT last_day FROM people WHERE agent = ?1 AND person = ?2",
                params![who.agent, who.person],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Sets the day they are speaking on, and hands back the one before it when it changed. Two dm processes at a day
    /// change cannot both see it change: the write lock is taken before the read.
    pub fn move_to_day(&self, who: &Who, day: &str) -> Result<Option<String>> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let last = self.last_day(who)?;
        if last.as_deref() == Some(day) {
            return Ok(None);
        }
        tx.execute(
            "INSERT INTO people (agent, person, last_day) VALUES (?1, ?2, ?3)
             ON CONFLICT (agent, person) DO UPDATE SET last_day = ?3",
            params![who.agent, who.person, day],
        )?;
        tx.commit()?;
        Ok(last)
    }
}

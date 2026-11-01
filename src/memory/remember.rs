//! The `remember` tool: SQLite full-text search (FTS5) across what is kept about a person: their transcripts, each
//! message with the ones either side of it, and what you know about them, each note also found by the kind of note it
//! is ("music" finds "What they love: jazz"), at a lower weight than its own words.
//!
//! The index lives in this process only (a temporary table), loaded from the files for the person being answered when
//! dm starts, so it always matches them; each transcript and notes file written while dm runs is added as it is
//! written.

use crate::{
    memory::transcript::{day_of, entries},
    provider::chat::ToolSpec,
    store::{names_in, notes::holds_for, Store},
    who::Who,
};
use anyhow::Result;
use rusqlite::params;
use serde_json::json;
use std::{fs, path::Path};

pub const NAME: &str = "remember";
const HITS: usize = 6;
/// Where a note comes from, in place of a day.
const KNOWN: &str = "what you know about them";
pub(crate) const INDEX: &str = "
  CREATE VIRTUAL TABLE IF NOT EXISTS temp.memory USING fts5 (
    agent UNINDEXED, person UNINDEXED, origin UNINDEXED, text, hint, context UNINDEXED, tokenize = 'porter unicode61'
  );
";
/// How much a match counts, by column: a note's own words over its kind.
const WEIGHTS: &str = "bm25(memory, 0, 0, 0, 1.0, 0.3, 0)";

const DESCRIPTION: &str = concat!(
    "Look up what you know about this person and what the two of you said before. ",
    "It searches their notes (their name, the people in their life, their work, what they love and dislike, what is going on, ",
    "what they asked you to remember) and everything said before that is no longer in this conversation, earlier today and every day before. ",
    "For query, give a few key words, not a sentence: names, places and topics, and the kind of thing you want (music, job, birthday, sister). ",
    "It returns the closest matches with where they come from, or \"Nothing found.\""
);

pub fn tool() -> ToolSpec {
    ToolSpec {
        name: NAME.to_string(),
        description: DESCRIPTION.to_string(),
        parameters: json!({
            "type": "object",
            "properties": { "query": { "type": "string", "description": "A few key words: names, places or topics." } },
            "required": ["query"],
        }),
    }
}

/// Each word quoted, so nothing in it is read as an operator; any of them may match.
fn fts_query(query: &str) -> Option<String> {
    let words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\""))
        .collect();
    (!words.is_empty()).then(|| words.join(" OR "))
}

impl Store {
    /// Loads the index from every transcript and the notes file of this agent and person.
    pub fn load_memory(&self, who: &Who) -> Result<()> {
        self.conn.execute(
            "DELETE FROM memory WHERE agent = ?1 AND person = ?2",
            params![who.agent, who.person],
        )?;
        let dir = self.transcript_dir(who);
        for file in names_in(&dir, Path::is_file) {
            if let Some(day) = day_of(&file) {
                self.index_transcript(who, day, &fs::read_to_string(dir.join(&file))?)?;
            }
        }
        if let Ok(markdown) = fs::read_to_string(self.notes_file(who)) {
            self.index_known(who, &markdown)?;
        }
        Ok(())
    }

    /// Indexes one transcript, each message with the messages either side.
    pub fn index_transcript(&self, who: &Who, day: &str, transcript: &str) -> Result<()> {
        let said = entries(transcript);
        for (i, text) in said.iter().enumerate() {
            let context = said[i.saturating_sub(1)..(i + 2).min(said.len())].join("\n");
            self.conn.execute(
                "INSERT INTO memory (agent, person, origin, text, hint, context) VALUES (?1, ?2, ?3, ?4, '', ?5)",
                params![who.agent, who.person, day, text, context],
            )?;
        }
        Ok(())
    }

    /// Replaces what is indexed of the person's notes with these.
    pub fn index_known(&self, who: &Who, markdown: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM memory WHERE agent = ?1 AND person = ?2 AND origin = ?3",
            params![who.agent, who.person, KNOWN],
        )?;
        for line in markdown.lines().filter_map(|l| l.strip_prefix("- ")) {
            // The note's words are searched, and its kind at a lower weight; its label is not, so "Their name: ..."
            // does not match every search for a name.
            let (label, note) = line.split_once(':').unwrap_or(("", line));
            self.conn.execute(
                "INSERT INTO memory (agent, person, origin, text, hint, context) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![who.agent, who.person, KNOWN, note.trim(), holds_for(label.trim()).unwrap_or_default(), line],
            )?;
        }
        Ok(())
    }

    pub fn recall(&self, who: &Who, query: &str) -> Result<String> {
        let Some(query) = fts_query(query) else {
            return Ok("Nothing found.".into());
        };
        let mut search = self.conn.prepare_cached(&format!(
            "SELECT origin, context FROM memory WHERE memory MATCH ?1 AND agent = ?2 AND person = ?3 ORDER BY {WEIGHTS} LIMIT ?4"
        ))?;
        let hits = search
            .query_map(params![query, who.agent, who.person, HITS], |r| {
                Ok(format!(
                    "{}:\n{}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(if hits.is_empty() {
            "Nothing found.".into()
        } else {
            hits.join("\n\n")
        })
    }
}

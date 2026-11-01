//! Transcripts. Messages are copied as they were into a transcript file, one per compaction
//! (`transcripts/<agent>/<person>/<day>-<n>.md`), then marked compacted, all or none, so history holds only what has
//! not been compacted.
//!
//!   - once 16 messages are in a day's history, all but the newest 4 are compacted;
//!   - when they speak on a new day, the day before is closed: everything left is compacted, and its summary follows.

use crate::{
    memory::{jobs::Job, transcript},
    store::Store,
    who::Who,
};
use anyhow::Result;
use std::{fs, io::Write, path::Path};

pub const COMPACT_AFTER: usize = 16;
pub const KEEP_RECENT: usize = 4;

impl Store {
    /// After a message is saved: queue a compaction once enough has built up.
    pub fn after_message(&self, who: &Who, day: &str, model: &str) -> Result<()> {
        if self.history(who, day)?.len() >= COMPACT_AFTER {
            self.enqueue(&Job::compact(who, day, false, model))?;
        }
        Ok(())
    }

    /// On a new day: close the day before.
    pub fn close_day(&self, who: &Who, day: &str, model: &str) -> Result<()> {
        self.enqueue(&Job::compact(who, day, true, model))
    }

    pub fn compact(&self, who: &Who, day: &str, close: bool, model: &str) -> Result<()> {
        let pending = self.history(who, day)?;
        let folded = if close {
            &pending[..]
        } else {
            &pending[..pending.len().saturating_sub(KEEP_RECENT)]
        };
        if !folded.is_empty() {
            // Written before the messages are marked: a compacted message is always in a file.
            let dir = self.transcript_dir(who);
            fs::create_dir_all(&dir)?;
            let file = dir.join(transcript::file_name(day, next_number(&dir, day)));
            let text = transcript::render(day, folded);
            let mut created = fs::File::create_new(&file)?;
            let marked = created
                .write_all(text.as_bytes())
                .map_err(Into::into)
                .and_then(|()| {
                    self.mark_compacted(&folded.iter().map(|m| m.id).collect::<Vec<_>>())
                });
            if !matches!(marked, Ok(true)) {
                // Failed, or another compaction got there first (its file stands): this one goes.
                let _ = fs::remove_file(&file);
            }
            if !marked? {
                return Ok(());
            }
            self.index_transcript(who, day, &text)?;
        }
        if close {
            self.enqueue(&Job::summary(who, day, model))?;
        }
        Ok(())
    }

    /// Every transcript of a day, oldest first.
    pub fn transcripts_of(&self, who: &Who, day: &str) -> String {
        let dir = self.transcript_dir(who);
        (1..)
            .map_while(|n| fs::read_to_string(dir.join(transcript::file_name(day, n))).ok())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// One more than the day's compactions so far.
fn next_number(dir: &Path, day: &str) -> usize {
    crate::store::names_in(dir, Path::is_file)
        .iter()
        .filter(|name| transcript::day_of(name) == Some(day))
        .count()
        + 1
}

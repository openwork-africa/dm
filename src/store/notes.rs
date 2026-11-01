//! What the agent knows about a person, in two layers. "How to be with them" goes into the prompt and shapes every
//! answer. "What you know" is not in the prompt: it is written to `notes/<agent>/<person>.md`, and found only through
//! `remember`. summary.rs fills them in.

use crate::{store::Store, who::Who};
use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use std::collections::BTreeMap;

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum Layer {
    Be,
    Know,
}

pub struct Note {
    pub key: &'static str,
    pub layer: Layer,
    pub label: &'static str,
    pub holds: &'static str,
}

/// Each note, in the order they are shown.
pub const NOTES: &[Note] = &[
    Note { key: "yourName", layer: Layer::Be, label: "Your name", holds: "The name they gave you, the agent." },
    Note { key: "howTheyTalk", layer: Layer::Be, label: "How they talk", holds: "How they write: language or mix of languages, message length, tone, formality, slang, emoji, words they use often." },
    Note { key: "howTheyLikeIt", layer: Layer::Be, label: "How they like you to be", holds: "What they have said they like or do not like in how you talk with them: length, questions, jokes, directness, topics to leave alone." },
    Note { key: "theirName", layer: Layer::Know, label: "Their name", holds: "Their name, and what they like to be called." },
    Note { key: "life", layer: Layer::Know, label: "Their life", holds: "Where they live, their work or studies, their stage of life and daily rhythm." },
    Note { key: "people", layer: Layer::Know, label: "People in their life", holds: "The people who matter to them, each written as name (who they are to them), e.g. Thabo (younger brother); mom." },
    Note { key: "interests", layer: Layer::Know, label: "What they love", holds: "Hobbies, music, food, sport, shows, places." },
    Note { key: "dislikes", layer: Layer::Know, label: "What they dislike", holds: "Things they dislike or would rather avoid." },
    Note { key: "values", layer: Layer::Know, label: "What matters to them", holds: "Beliefs, hopes and goals." },
    Note { key: "goingOn", layer: Layer::Know, label: "Going on now", holds: "Plans, events and worries in their life now, with dates when known." },
    Note { key: "remember", layer: Layer::Know, label: "They asked you to remember", holds: "Only what they explicitly asked you to remember (\"remember that...\", \"don't forget...\"); empty otherwise." },
];

/// What a note holds, by its label, so `remember` finds "What they love: jazz" when asked about music.
pub fn holds_for(label: &str) -> Option<&'static str> {
    NOTES.iter().find(|n| n.label == label).map(|n| n.holds)
}

/// The notes by key; one not yet known is missing or empty.
pub type Notes = BTreeMap<String, String>;

pub fn lines_of(notes: &Notes, layer: Layer) -> Vec<String> {
    NOTES
        .iter()
        .filter(|n| n.layer == layer)
        .filter_map(|n| {
            notes
                .get(n.key)
                .map(|v| v.trim())
                .filter(|v| !v.is_empty())
                .map(|v| format!("- {}: {v}", n.label))
        })
        .collect()
}

/// "What you know", as the Markdown file `remember` searches.
pub fn known_markdown(notes: &Notes) -> String {
    let mut lines = vec!["# What you know about them".to_string(), String::new()];
    lines.extend(lines_of(notes, Layer::Know));
    lines.push(String::new());
    lines.join("\n")
}

impl Store {
    pub fn notes(&self, who: &Who) -> Result<Notes> {
        let saved: Option<String> = self
            .conn
            .query_row(
                "SELECT notes FROM notes WHERE agent = ?1 AND person = ?2",
                params![who.agent, who.person],
                |r| r.get(0),
            )
            .optional()?;
        Ok(saved
            .and_then(|n| serde_json::from_str(&n).ok())
            .unwrap_or_default())
    }

    pub fn set_notes(&self, who: &Who, notes: &Notes) -> Result<()> {
        self.conn.execute(
            "INSERT INTO notes (agent, person, notes) VALUES (?1, ?2, ?3) ON CONFLICT (agent, person) DO UPDATE SET notes = ?3",
            params![who.agent, who.person, serde_json::to_string(notes)?],
        )?;
        Ok(())
    }
}

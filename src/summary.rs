//! Summaries: once a day is closed, a separate call reads that day's transcripts with the person's current notes and
//! must call its `notes` tool with the complete updated notes, each with the person's own words it comes from. They
//! are saved in the `notes` table; "what you know" is also written to `notes/<agent>/<person>.md` and indexed for
//! `remember`.

use crate::{
    notes::{known_markdown, Notes, NOTES},
    provider::{
        chat::{Chat, Message, ToolSpec},
        Model,
    },
    store::Store,
    who::Who,
};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, fs};

const INSTRUCTIONS: &str = include_str!("summary.md");
const TOOL: &str = "notes";

fn tool() -> ToolSpec {
    let properties: Map<String, Value> = NOTES
        .iter()
        .map(|n| {
            (
                n.key.to_string(),
                json!({
                    "type": "object",
                    "description": n.holds,
                    "properties": {
                        "value": { "type": "string", "description": "The note: a few words or short phrases, separated by semicolons." },
                        "quote": { "type": "string", "description": "The person's own words this note comes from, copied exactly." },
                    },
                    "required": ["value", "quote"],
                }),
            )
        })
        .collect();
    ToolSpec {
        name: TOOL.to_string(),
        description: "Save the complete updated notes about the person. Send every note each time. For each one, `value` is the note and `quote` is the person's own words it comes from, copied exactly from a Person: line; when nothing they said supports a note, send an empty value and an empty quote.".to_string(),
        parameters: json!({ "type": "object", "properties": properties, "required": NOTES.iter().map(|n| n.key).collect::<Vec<_>>() }),
    }
}

/// One note as the model sent it.
#[derive(Deserialize)]
struct Sent {
    value: Option<String>,
}

pub async fn update(store: &Store, model: &Model, who: &Who, day: &str) -> Result<()> {
    let transcript = store.transcripts_of(who, day);
    if transcript.trim().is_empty() {
        return Ok(());
    }
    let current = serde_json::to_string_pretty(&store.notes(who)?)?;
    let chat = Chat {
        system: INSTRUCTIONS.to_string(),
        messages: vec![Message::User(format!(
            "<current_notes>\n{current}\n</current_notes>\n\n<conversation>\n{transcript}\n</conversation>"
        ))],
        tools: vec![tool()],
    };
    let turn = model.ask(&chat, &mut |_| {}).await?;
    store.count_call(day, who, "summary", model, &turn.usage)?;
    let call = turn
        .reply
        .calls
        .iter()
        .find(|c| c.name == TOOL)
        .ok_or_else(|| anyhow!("the summary model did not call its notes tool"))?;

    let sent: BTreeMap<String, Sent> = serde_json::from_value(call.input())?;
    let notes: Notes = NOTES
        .iter()
        .filter_map(|n| {
            sent.get(n.key)
                .and_then(|s| s.value.as_ref())
                .map(|v| (n.key.to_string(), v.trim().to_string()))
        })
        .collect();
    store.set_notes(who, &notes)?;

    let markdown = known_markdown(&notes);
    let file = store.notes_file(who);
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let part = file.with_extension("md.part");
    fs::write(&part, &markdown)?;
    fs::rename(&part, &file)?;
    store.index_known(who, &markdown)
}

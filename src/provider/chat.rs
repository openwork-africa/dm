//! A conversation as every API style sees it, and what a model's streamed answer is built into.

use super::think::ThinkFilter;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// A tool the model may call; `parameters` is a JSON schema.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// The arguments as the model wrote them: JSON text.
    pub arguments: String,
}

impl ToolCall {
    /// The arguments as a JSON object; an empty object when they are missing or not JSON.
    pub fn input(&self) -> Value {
        serde_json::from_str(&self.arguments)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub id: String,
    pub content: String,
}

/// What the model said in one call.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    /// Blocks the style must be sent back unchanged with this turn (thinking with its signature, reasoning items).
    pub replay: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    User(String),
    Assistant(Reply),
    ToolResults(Vec<ToolOutput>),
}

#[derive(Debug, Clone, Default)]
pub struct Chat {
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
}

impl Chat {
    /// The system text, unless there is none to send.
    pub fn system(&self) -> Option<&str> {
        Some(self.system.as_str()).filter(|s| !s.trim().is_empty())
    }
}

/// Tokens, with input that came from or went into the cache counted apart from the rest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

/// One thing a style read from the stream.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Text(String),
    /// A piece of a tool call; `slot` tells calls apart within one answer.
    ToolCall {
        slot: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    },
    Replay(Value),
    Usage(Usage),
}

impl Event {
    pub fn text(value: &Value) -> Self {
        Event::Text(super::text(value))
    }

    /// A tool call begins: its id and name.
    pub fn call_start(slot: usize, id: &Value, name: &Value) -> Self {
        Event::ToolCall {
            slot,
            id: id.as_str().map(str::to_string),
            name: name.as_str().map(str::to_string),
            arguments: String::new(),
        }
    }

    /// More of a tool call's arguments.
    pub fn call_arguments(slot: usize, arguments: &Value) -> Self {
        Event::ToolCall {
            slot,
            id: None,
            name: None,
            arguments: super::text(arguments),
        }
    }
}

/// A model's whole answer to one call.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Turn {
    pub reply: Reply,
    pub usage: Usage,
}

fn stream(text: &str, on_text: &mut dyn FnMut(&str)) {
    if !text.is_empty() {
        on_text(text);
    }
}

/// Gathers events into a turn, in the order the calls were started, with reasoning kept out of the text.
#[derive(Default)]
pub struct TurnBuilder {
    turn: Turn,
    slots: BTreeMap<usize, usize>,
    think: ThinkFilter,
}

impl TurnBuilder {
    pub fn apply(&mut self, event: Event, on_text: &mut dyn FnMut(&str)) {
        match event {
            Event::Text(text) => stream(&self.think.push(&text), on_text),
            Event::ToolCall {
                slot,
                id,
                name,
                arguments,
            } => {
                let calls = &mut self.turn.reply.calls;
                let at = *self.slots.entry(slot).or_insert_with(|| {
                    calls.push(ToolCall::default());
                    calls.len() - 1
                });
                let call = &mut calls[at];
                if let Some(id) = id.filter(|id| !id.is_empty()) {
                    call.id = id;
                }
                call.name.push_str(&name.unwrap_or_default());
                call.arguments.push_str(&arguments);
            }
            Event::Replay(block) => self.turn.reply.replay.push(block),
            Event::Usage(usage) => self.turn.usage = usage,
        }
    }

    pub fn finish(mut self, on_text: &mut dyn FnMut(&str)) -> Turn {
        let (rest, reply) = self.think.finish();
        stream(&rest, on_text);
        self.turn.reply.text = reply;
        // A call a host left without an id still needs one to be answered.
        for (n, call) in self.turn.reply.calls.iter_mut().enumerate() {
            if call.id.is_empty() {
                call.id = format!("call_{n}");
            }
        }
        self.turn
    }
}

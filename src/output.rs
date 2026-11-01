//! What dm prints: JSON lines (`stream-json`), or the reply alone as plain text.

use crate::provider::chat::Usage;
use serde::Serialize;
use serde_json::Value;
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    #[value(name = "stream-json")]
    StreamJson,
    #[value(name = "text")]
    Text,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Line<'a> {
    Text {
        delta: &'a str,
    },
    ToolCall {
        id: &'a str,
        name: &'a str,
        arguments: Value,
    },
    ToolResult {
        id: &'a str,
        name: &'a str,
        content: &'a str,
    },
    #[serde(rename_all = "camelCase")]
    Done {
        reply: &'a str,
        model: &'a str,
        usage: Usage,
        cost_usd: f64,
    },
}

/// Where the lines go. A closed output (the reader left) is not an error: the answer is still kept.
pub trait Sink {
    fn line(&mut self, line: &Line);
}

pub struct JsonLines<W: Write>(pub W);

impl<W: Write> Sink for JsonLines<W> {
    fn line(&mut self, line: &Line) {
        if let Ok(text) = serde_json::to_string(line) {
            let _ = writeln!(self.0, "{text}").and_then(|_| self.0.flush());
        }
    }
}

pub struct PlainText<W: Write>(pub W);

impl<W: Write> Sink for PlainText<W> {
    fn line(&mut self, line: &Line) {
        let _ = match line {
            Line::Text { delta } => write!(self.0, "{delta}").and_then(|_| self.0.flush()),
            Line::Done { .. } => writeln!(self.0).and_then(|_| self.0.flush()),
            _ => Ok(()),
        };
    }
}

/// Lines kept in memory: for tests and for callers of the library.
#[derive(Default)]
pub struct Collected(pub Vec<Value>);

impl Sink for Collected {
    fn line(&mut self, line: &Line) {
        self.0.push(serde_json::to_value(line).unwrap_or_default());
    }
}

pub fn sink_for(format: Format) -> Box<dyn Sink> {
    let stdout = std::io::stdout();
    match format {
        Format::StreamJson => Box::new(JsonLines(stdout)),
        Format::Text => Box::new(PlainText(stdout)),
    }
}

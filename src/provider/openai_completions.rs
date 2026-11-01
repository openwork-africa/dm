//! OpenAI chat completions, which most hosts speak.

use super::{
    chat::{Chat, Event, Message},
    framing::Frame,
    json_of, openai_usage, text, Api, StreamReader,
};
use crate::config::{ModelEntry, Provider};
use anyhow::{bail, Result};
use serde_json::{json, Value};

#[derive(Default)]
pub struct OpenAiCompletions;

fn messages(chat: &Chat) -> Vec<Value> {
    let mut all: Vec<Value> = chat
        .system()
        .map(|system| json!({ "role": "system", "content": system }))
        .into_iter()
        .collect();
    for message in &chat.messages {
        match message {
            Message::User(text) => all.push(json!({ "role": "user", "content": text })),
            Message::Assistant(reply) => {
                let mut said = json!({ "role": "assistant", "content": reply.text });
                if !reply.calls.is_empty() {
                    said["tool_calls"] = reply
                        .calls
                        .iter()
                        .map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.arguments } }))
                        .collect();
                }
                all.push(said);
            }
            Message::ToolResults(results) => all.extend(
                results
                    .iter()
                    .map(|r| json!({ "role": "tool", "tool_call_id": r.id, "content": r.content })),
            ),
        }
    }
    all
}

fn body(model: &ModelEntry, chat: &Chat) -> Value {
    let mut body = json!({
        "model": model.id,
        "messages": messages(chat),
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if !chat.tools.is_empty() {
        body["tools"] = chat
            .tools
            .iter()
            .map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.parameters } }))
            .collect();
    }
    if let Some(max) = model.max_tokens {
        body["max_tokens"] = json!(max);
    }
    if let Some(effort) = &model.reasoning_effort {
        body["reasoning_effort"] = json!(effort);
    }
    body
}

struct ChunkReader;

impl StreamReader for ChunkReader {
    fn read(&mut self, frame: &Frame) -> Result<Vec<Event>> {
        let Some(chunk) = json_of(frame) else {
            return Ok(vec![]);
        };
        if let Some(error) = chunk.get("error") {
            bail!(
                "the model failed: {}",
                error["message"].as_str().unwrap_or(&error.to_string())
            );
        }
        let mut events = vec![];
        let delta = &chunk["choices"][0]["delta"];
        if delta["content"].is_string() {
            events.push(Event::text(&delta["content"]));
        }
        for piece in delta["tool_calls"].as_array().into_iter().flatten() {
            events.push(Event::ToolCall {
                slot: piece["index"].as_u64().unwrap_or(0) as usize,
                id: piece["id"].as_str().map(str::to_string),
                name: piece["function"]["name"].as_str().map(str::to_string),
                arguments: text(&piece["function"]["arguments"]),
            });
        }
        let usage = &chunk["usage"];
        if usage.is_object() {
            events.push(Event::Usage(openai_usage(
                usage,
                "prompt_tokens",
                "completion_tokens",
                "prompt_tokens_details",
            )));
        }
        Ok(events)
    }
}

impl Api for OpenAiCompletions {
    fn url(&self, provider: &Provider, _: &ModelEntry) -> Result<String> {
        Ok(format!("{}/chat/completions", provider.base_url()?))
    }

    fn body(&self, model: &ModelEntry, chat: &Chat) -> Value {
        body(model, chat)
    }

    fn reader(&self) -> Box<dyn StreamReader> {
        Box::new(ChunkReader)
    }
}

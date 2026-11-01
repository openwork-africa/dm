//! OpenAI chat completions, which most hosts speak.

use super::{
    chat::{Chat, Event, Message, Usage},
    count,
    framing::Frame,
    json_of, Api, StreamReader,
};
use crate::config::{ModelEntry, Provider};
use anyhow::{bail, Result};
use serde_json::{json, Value};

pub struct OpenAiCompletions;

fn messages(chat: &Chat) -> Vec<Value> {
    let mut all = vec![json!({ "role": "system", "content": chat.system })];
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

/// The chat-completions body, shared with Azure Foundry.
pub(super) fn body(model: &ModelEntry, chat: &Chat) -> Value {
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

/// Reads chat-completion chunks, shared with Azure Foundry.
pub(super) struct ChunkReader;

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
        if let Some(text) = delta["content"].as_str() {
            events.push(Event::Text(text.to_string()));
        }
        for piece in delta["tool_calls"].as_array().into_iter().flatten() {
            events.push(Event::ToolCall {
                slot: piece["index"].as_u64().unwrap_or(0) as usize,
                id: piece["id"].as_str().map(str::to_string),
                name: piece["function"]["name"].as_str().map(str::to_string),
                arguments: piece["function"]["arguments"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            });
        }
        let usage = &chunk["usage"];
        if usage.is_object() {
            let cached = count(&usage["prompt_tokens_details"]["cached_tokens"]);
            events.push(Event::Usage(Usage {
                input: count(&usage["prompt_tokens"]).saturating_sub(cached),
                output: count(&usage["completion_tokens"]),
                cache_read: cached,
                cache_write: 0,
            }));
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

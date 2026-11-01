//! OpenAI Responses. Calls are made with `store: false`; reasoning items come back whole and go back with their turn.

use super::{
    chat::{Chat, Event, Message, Usage},
    count,
    framing::Frame,
    json_of, Api, StreamReader,
};
use crate::config::{ModelEntry, Provider};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub struct OpenAiResponses;

fn input(chat: &Chat) -> Vec<Value> {
    let mut items = vec![];
    for message in &chat.messages {
        match message {
            Message::User(text) => items.push(json!({ "role": "user", "content": text })),
            Message::Assistant(reply) => {
                items.extend(reply.replay.iter().cloned());
                if !reply.text.is_empty() {
                    items.push(json!({ "role": "assistant", "content": reply.text }));
                }
                items.extend(reply.calls.iter().map(
                    |c| json!({ "type": "function_call", "call_id": c.id, "name": c.name, "arguments": c.arguments }),
                ));
            }
            Message::ToolResults(results) => items.extend(results.iter().map(
                |r| json!({ "type": "function_call_output", "call_id": r.id, "output": r.content }),
            )),
        }
    }
    items
}

impl Api for OpenAiResponses {
    fn url(&self, provider: &Provider, _: &ModelEntry) -> Result<String> {
        Ok(format!("{}/responses", provider.base_url()?))
    }

    fn body(&self, model: &ModelEntry, chat: &Chat) -> Value {
        let mut body = json!({
            "model": model.id,
            "instructions": chat.system,
            "input": input(chat),
            "stream": true,
            "store": false,
        });
        if !chat.tools.is_empty() {
            body["tools"] = chat
                .tools
                .iter()
                .map(|t| json!({ "type": "function", "name": t.name, "description": t.description, "parameters": t.parameters }))
                .collect();
        }
        if let Some(max) = model.max_tokens {
            body["max_output_tokens"] = json!(max);
        }
        if let Some(effort) = &model.reasoning_effort {
            body["reasoning"] = json!({ "effort": effort });
        }
        if model.thinking {
            // Without storage, reasoning can be sent back only in its encrypted form.
            body["include"] = json!(["reasoning.encrypted_content"]);
        }
        body
    }

    fn reader(&self) -> Box<dyn StreamReader> {
        Box::<Reader>::default()
    }
}

#[derive(Default)]
struct Reader {
    /// Calls whose arguments were streamed, so the finished item does not add them again.
    streamed: BTreeSet<usize>,
}

impl StreamReader for Reader {
    fn read(&mut self, frame: &Frame) -> Result<Vec<Event>> {
        let Some(event) = json_of(frame) else {
            return Ok(vec![]);
        };
        let slot = event["output_index"].as_u64().unwrap_or(0) as usize;
        let item = &event["item"];
        let mut events = vec![];
        match event["type"].as_str().unwrap_or_default() {
            "response.output_text.delta" => events.push(Event::Text(
                event["delta"].as_str().unwrap_or_default().to_string(),
            )),
            "response.output_item.added" if item["type"] == "function_call" => {
                events.push(Event::ToolCall {
                    slot,
                    id: item["call_id"].as_str().map(str::to_string),
                    name: item["name"].as_str().map(str::to_string),
                    arguments: String::new(),
                })
            }
            "response.function_call_arguments.delta" => {
                self.streamed.insert(slot);
                events.push(Event::ToolCall {
                    slot,
                    id: None,
                    name: None,
                    arguments: event["delta"].as_str().unwrap_or_default().to_string(),
                });
            }
            "response.output_item.done" => match item["type"].as_str().unwrap_or_default() {
                "reasoning" => events.push(Event::Replay(item.clone())),
                "function_call" if !self.streamed.contains(&slot) => events.push(Event::ToolCall {
                    slot,
                    id: item["call_id"].as_str().map(str::to_string),
                    name: None,
                    arguments: item["arguments"].as_str().unwrap_or_default().to_string(),
                }),
                _ => {}
            },
            "response.completed" => {
                let usage = &event["response"]["usage"];
                let cached = count(&usage["input_tokens_details"]["cached_tokens"]);
                events.push(Event::Usage(Usage {
                    input: count(&usage["input_tokens"]).saturating_sub(cached),
                    output: count(&usage["output_tokens"]),
                    cache_read: cached,
                    cache_write: 0,
                }));
            }
            "response.failed" | "response.incomplete" | "error" => {
                let error = if event["response"]["error"].is_object() {
                    &event["response"]["error"]
                } else {
                    &event
                };
                bail!(
                    "the model failed: {}",
                    error["message"]
                        .as_str()
                        .unwrap_or("the response did not complete")
                );
            }
            _ => {}
        }
        Ok(events)
    }
}

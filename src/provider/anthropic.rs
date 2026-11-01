//! Anthropic Messages. Thinking blocks come back with a signature and go back unchanged with their turn.

use super::{
    chat::{Chat, Event, Message, Usage},
    count,
    framing::Frame,
    json_of, Api, KeyHeader, StreamReader,
};
use crate::config::{ModelEntry, Provider};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const VERSION: &str = "2023-06-01";
/// Messages requires a limit; this one when the model sets none.
const MAX_TOKENS: u64 = 16_000;

pub struct AnthropicMessages;

fn messages(chat: &Chat) -> Vec<Value> {
    chat.messages
        .iter()
        .map(|message| match message {
            Message::User(text) => json!({ "role": "user", "content": text }),
            Message::Assistant(reply) => {
                let mut content = reply.replay.clone();
                if !reply.text.is_empty() {
                    content.push(json!({ "type": "text", "text": reply.text }));
                }
                content.extend(
                    reply.calls.iter().map(|c| json!({ "type": "tool_use", "id": c.id, "name": c.name, "input": c.input() })),
                );
                json!({ "role": "assistant", "content": content })
            }
            Message::ToolResults(results) => json!({
                "role": "user",
                "content": results.iter().map(|r| json!({ "type": "tool_result", "tool_use_id": r.id, "content": r.content })).collect::<Vec<_>>(),
            }),
        })
        .collect()
}

impl Api for AnthropicMessages {
    fn url(&self, provider: &Provider, _: &ModelEntry) -> Result<String> {
        Ok(format!("{}/v1/messages", provider.base_url()?))
    }

    fn key_header(&self) -> KeyHeader {
        KeyHeader::Named("x-api-key")
    }

    fn fixed_headers(&self) -> &'static [(&'static str, &'static str)] {
        &[("anthropic-version", VERSION)]
    }

    fn body(&self, model: &ModelEntry, chat: &Chat) -> Value {
        let mut body = json!({
            "model": model.id,
            "max_tokens": model.max_tokens.unwrap_or(MAX_TOKENS),
            "system": chat.system,
            "messages": messages(chat),
            "stream": true,
        });
        if !chat.tools.is_empty() {
            body["tools"] = chat
                .tools
                .iter()
                .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.parameters }))
                .collect();
        }
        if model.thinking {
            body["thinking"] = json!({ "type": "adaptive" });
        }
        if let Some(effort) = &model.reasoning_effort {
            body["output_config"] = json!({ "effort": effort });
        }
        body
    }

    fn reader(&self) -> Box<dyn StreamReader> {
        Box::<Reader>::default()
    }
}

#[derive(Default)]
struct Reader {
    usage: Usage,
    /// Thinking blocks being streamed, by index.
    thinking: BTreeMap<u64, Value>,
}

impl StreamReader for Reader {
    fn read(&mut self, frame: &Frame) -> Result<Vec<Event>> {
        let Some(event) = json_of(frame) else {
            return Ok(vec![]);
        };
        let index = event["index"].as_u64().unwrap_or(0);
        let mut events = vec![];
        match event["type"].as_str().unwrap_or_default() {
            "message_start" => {
                let usage = &event["message"]["usage"];
                self.usage.input = count(&usage["input_tokens"]);
                self.usage.cache_read = count(&usage["cache_read_input_tokens"]);
                self.usage.cache_write = count(&usage["cache_creation_input_tokens"]);
                self.usage.output = count(&usage["output_tokens"]);
            }
            "content_block_start" => {
                let block = &event["content_block"];
                match block["type"].as_str().unwrap_or_default() {
                    "tool_use" => events.push(Event::ToolCall {
                        slot: index as usize,
                        id: block["id"].as_str().map(str::to_string),
                        name: block["name"].as_str().map(str::to_string),
                        arguments: String::new(),
                    }),
                    "thinking" => {
                        self.thinking.insert(index, json!({ "type": "thinking", "thinking": block["thinking"], "signature": block["signature"] }));
                    }
                    "redacted_thinking" => events.push(Event::Replay(block.clone())),
                    "text" => events.push(Event::Text(
                        block["text"].as_str().unwrap_or_default().to_string(),
                    )),
                    _ => {}
                }
            }
            "content_block_delta" => {
                let delta = &event["delta"];
                match delta["type"].as_str().unwrap_or_default() {
                    "text_delta" => events.push(Event::Text(
                        delta["text"].as_str().unwrap_or_default().to_string(),
                    )),
                    "input_json_delta" => events.push(Event::ToolCall {
                        slot: index as usize,
                        id: None,
                        name: None,
                        arguments: delta["partial_json"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    }),
                    kind @ ("thinking_delta" | "signature_delta") => {
                        let field = if kind == "thinking_delta" {
                            "thinking"
                        } else {
                            "signature"
                        };
                        if let Some(block) = self.thinking.get_mut(&index) {
                            let joined = format!(
                                "{}{}",
                                block[field].as_str().unwrap_or_default(),
                                delta[field].as_str().unwrap_or_default()
                            );
                            block[field] = json!(joined);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => events.extend(self.thinking.remove(&index).map(Event::Replay)),
            "message_delta" => {
                let usage = &event["usage"];
                if usage["output_tokens"].is_u64() {
                    self.usage.output = count(&usage["output_tokens"]);
                }
            }
            "message_stop" => events.push(Event::Usage(self.usage)),
            "error" => bail!(
                "the model failed: {}",
                event["error"]["message"].as_str().unwrap_or("an error")
            ),
            _ => {}
        }
        Ok(events)
    }
}

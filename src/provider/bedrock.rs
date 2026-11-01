//! Amazon Bedrock's Converse API, streamed (`converse-stream`), signed with a Bedrock API key as a bearer token. The
//! URL is `baseUrl` when given, else the runtime endpoint for the provider's `region`.

use super::{
    chat::{Chat, Event, Message, Usage},
    count,
    framing::{Frame, Framing},
    Api, StreamReader, ThinkingBlocks,
};
use crate::config::{ModelEntry, Provider};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

pub struct Bedrock;

/// A model id or ARN as one path segment.
fn path_segment(id: &str) -> String {
    id.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn text_block(text: &str) -> Value {
    json!({ "text": text })
}

fn messages(chat: &Chat) -> Vec<Value> {
    chat.messages
        .iter()
        .map(|message| match message {
            Message::User(text) => json!({ "role": "user", "content": [text_block(text)] }),
            Message::Assistant(reply) => {
                let mut content = reply.replay.clone();
                if !reply.text.is_empty() {
                    content.push(text_block(&reply.text));
                }
                content.extend(
                    reply.calls.iter().map(|c| json!({ "toolUse": { "toolUseId": c.id, "name": c.name, "input": c.input() } })),
                );
                json!({ "role": "assistant", "content": content })
            }
            Message::ToolResults(results) => json!({
                "role": "user",
                "content": results.iter().map(|r| json!({ "toolResult": { "toolUseId": r.id, "content": [text_block(&r.content)] } })).collect::<Vec<_>>(),
            }),
        })
        .collect()
}

impl Api for Bedrock {
    fn url(&self, provider: &Provider, model: &ModelEntry) -> Result<String> {
        let base = match &provider.base_url {
            Some(_) => provider.base_url()?.to_string(),
            None => {
                let region = provider
                    .region
                    .as_deref()
                    .ok_or_else(|| anyhow!("a bedrock provider needs a region or a baseUrl"))?;
                format!("https://bedrock-runtime.{region}.amazonaws.com")
            }
        };
        Ok(format!(
            "{base}/model/{}/converse-stream",
            path_segment(&model.id)
        ))
    }

    fn body(&self, model: &ModelEntry, chat: &Chat) -> Value {
        let mut body = json!({ "messages": messages(chat) });
        if !chat.system.is_empty() {
            body["system"] = json!([text_block(&chat.system)]);
        }
        if !chat.tools.is_empty() {
            let tools: Vec<Value> = chat
                .tools
                .iter()
                .map(|t| json!({ "toolSpec": { "name": t.name, "description": t.description, "inputSchema": { "json": t.parameters } } }))
                .collect();
            body["toolConfig"] = json!({ "tools": tools });
        }
        if let Some(max) = model.max_tokens {
            body["inferenceConfig"] = json!({ "maxTokens": max });
        }
        let mut extra = json!({});
        if model.thinking {
            extra["thinking"] = json!({ "type": "adaptive" });
        }
        if let Some(effort) = &model.reasoning_effort {
            extra["output_config"] = json!({ "effort": effort });
        }
        if extra.as_object().is_some_and(|o| !o.is_empty()) {
            body["additionalModelRequestFields"] = extra;
        }
        body
    }

    fn framing(&self) -> Framing {
        Framing::AwsEventStream
    }

    fn reader(&self) -> Box<dyn StreamReader> {
        Box::<Reader>::default()
    }
}

#[derive(Default)]
struct Reader {
    reasoning: ThinkingBlocks,
}

impl StreamReader for Reader {
    fn read(&mut self, frame: &Frame) -> Result<Vec<Event>> {
        let name = frame.name.as_deref().unwrap_or_default();
        if let Some(kind) = name.strip_prefix("exception:") {
            bail!(
                "bedrock failed ({kind}): {}",
                frame.data.chars().take(400).collect::<String>()
            );
        }
        let event: Value = serde_json::from_str(&frame.data).unwrap_or_default();
        let index = event["contentBlockIndex"].as_u64().unwrap_or(0);
        let mut events = vec![];
        match name {
            "contentBlockStart" => {
                let tool = &event["start"]["toolUse"];
                if tool.is_object() {
                    events.push(Event::call_start(
                        index as usize,
                        &tool["toolUseId"],
                        &tool["name"],
                    ));
                }
            }
            "contentBlockDelta" => {
                let delta = &event["delta"];
                if delta["text"].is_string() {
                    events.push(Event::text(&delta["text"]));
                }
                if delta["toolUse"]["input"].is_string() {
                    events.push(Event::call_arguments(
                        index as usize,
                        &delta["toolUse"]["input"],
                    ));
                }
                let reasoning = &delta["reasoningContent"];
                if reasoning.is_object() {
                    self.reasoning.open(index);
                    self.reasoning
                        .push(index, &reasoning["text"], &reasoning["signature"]);
                }
            }
            "contentBlockStop" => {
                if let Some((text, signature)) = self.reasoning.take(index) {
                    events.push(Event::Replay(json!({ "reasoningContent": { "reasoningText": { "text": text, "signature": signature } } })));
                }
            }
            "metadata" => {
                let usage = &event["usage"];
                events.push(Event::Usage(Usage {
                    input: count(&usage["inputTokens"]),
                    output: count(&usage["outputTokens"]),
                    cache_read: count(&usage["cacheReadInputTokens"]),
                    cache_write: count(&usage["cacheWriteInputTokens"]),
                }));
            }
            _ => {}
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_ids_and_arns_are_one_path_segment() {
        assert_eq!(
            path_segment("us.anthropic.claude-opus-5-5"),
            "us.anthropic.claude-opus-5-5"
        );
        assert_eq!(
            path_segment("arn:aws:bedrock:x/y"),
            "arn%3Aaws%3Abedrock%3Ax%2Fy"
        );
    }
}

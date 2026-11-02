//! Each API style against a mock: the answer streams out, a tool call is run and answered, and usage and cost come
//! back in the done line.

mod common;

use common::{aws_event, named_sse, serve, sse, Reply};
use dm::{
    output::Collected,
    provider::framing::{Frame, Framing},
    Store,
};
use serde_json::{json, Value};

async fn ask(api: &str, replies: Vec<Reply>) -> (Vec<Value>, Vec<common::Received>) {
    let (lines, sent, _) = common::answer(
        |url| common::request(api, url, "what is my sister called?"),
        replies,
    )
    .await;
    (lines, sent)
}

fn kinds(lines: &[Value]) -> Vec<&str> {
    lines.iter().map(|l| l["type"].as_str().unwrap()).collect()
}

fn check(lines: &[Value]) {
    assert_eq!(
        kinds(lines),
        vec!["tool_call", "tool_result", "text", "text", "done"]
    );
    assert_eq!(lines[0]["name"], "remember");
    assert_eq!(lines[0]["arguments"], json!({ "query": "sister" }));
    assert_eq!(lines[1]["content"], "Nothing found.");
    let done = &lines[4];
    assert_eq!(done["reply"], "Her name is Lindi.");
    assert_eq!(done["model"], "mock/m1");
    assert_eq!(
        done["usage"],
        json!({ "input": 20, "output": 10, "cacheRead": 0, "cacheWrite": 0 })
    );
    assert!((done["costUsd"].as_f64().unwrap() - (20.0 + 20.0) / 1e6).abs() < 1e-12);
}

#[tokio::test]
async fn openai_completions() {
    let usage = json!({ "prompt_tokens": 10, "completion_tokens": 5 });
    let (lines, sent) = ask(
        "openai-completions",
        vec![
            sse(&[
                json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "id": "c1", "function": { "name": "remember", "arguments": "{\"que" } }] } }] }),
                json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "function": { "arguments": "ry\":\"sister\"}" } }] } }] }),
                json!({ "choices": [], "usage": usage }),
            ]),
            sse(&[
                json!({ "choices": [{ "delta": { "content": "Her name " } }] }),
                json!({ "choices": [{ "delta": { "content": "is Lindi." } }] }),
                json!({ "choices": [], "usage": usage }),
            ]),
        ],
    )
    .await;
    check(&lines);
    assert_eq!(sent[0].path, "/chat/completions");
    assert_eq!(
        sent[0].header("authorization"),
        Some("Bearer fake-literal-key")
    );
    assert_eq!(sent[0].body["tools"][0]["function"]["name"], "remember");
    let messages = sent[1].body["messages"].as_array().unwrap();
    assert_eq!(messages[messages.len() - 2]["tool_calls"][0]["id"], "c1");
    assert_eq!(
        messages[messages.len() - 1],
        json!({ "role": "tool", "tool_call_id": "c1", "content": "Nothing found." })
    );
}

#[tokio::test]
async fn azure_foundry() {
    let usage = json!({ "prompt_tokens": 10, "completion_tokens": 5 });
    let (lines, sent) = ask(
        "azure-foundry",
        vec![
            sse(&[
                json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "id": "c1", "function": { "name": "remember", "arguments": "{\"query\":\"sister\"}" } }] } }] }),
                json!({ "choices": [], "usage": usage }),
            ]),
            sse(&[
                json!({ "choices": [{ "delta": { "content": "Her name " } }] }),
                json!({ "choices": [{ "delta": { "content": "is Lindi." } }], "usage": usage }),
            ]),
        ],
    )
    .await;
    check(&lines);
    assert_eq!(
        sent[0].path,
        "/chat/completions?api-version=2024-05-01-preview"
    );
    assert_eq!(sent[0].header("api-key"), Some("fake-literal-key"));
    assert_eq!(sent[0].header("authorization"), None);
}

#[tokio::test]
async fn anthropic_messages() {
    let start = json!({ "type": "message_start", "message": { "usage": { "input_tokens": 10, "output_tokens": 1 } } });
    let (lines, sent) = ask(
        "anthropic-messages",
        vec![
            named_sse(&[
                start.clone(),
                json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "", "signature": "" } }),
                json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "look it up" } }),
                json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "sig" } }),
                json!({ "type": "content_block_stop", "index": 0 }),
                json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "tool_use", "id": "t1", "name": "remember", "input": {} } }),
                json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "{\"query\":" } }),
                json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "\"sister\"}" } }),
                json!({ "type": "content_block_stop", "index": 1 }),
                json!({ "type": "message_delta", "usage": { "output_tokens": 5 } }),
                json!({ "type": "message_stop" }),
            ]),
            named_sse(&[
                start,
                json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
                json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Her name " } }),
                json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "is Lindi." } }),
                json!({ "type": "message_delta", "usage": { "output_tokens": 5 } }),
                json!({ "type": "message_stop" }),
            ]),
        ],
    )
    .await;
    check(&lines);
    assert_eq!(sent[0].path, "/v1/messages");
    assert_eq!(sent[0].header("x-api-key"), Some("fake-literal-key"));
    assert_eq!(sent[0].header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(
        sent[0].body["tools"][0]["input_schema"]["properties"]["url"]["type"],
        json!("array")
    );
    let messages = sent[1].body["messages"].as_array().unwrap();
    let assistant = &messages[messages.len() - 2]["content"];
    assert_eq!(
        assistant[0],
        json!({ "type": "thinking", "thinking": "look it up", "signature": "sig" })
    );
    assert_eq!(
        assistant[1],
        json!({ "type": "tool_use", "id": "t1", "name": "remember", "input": { "query": "sister" } })
    );
    assert_eq!(
        messages[messages.len() - 1]["content"][0]["tool_use_id"],
        "t1"
    );
}

#[tokio::test]
async fn openai_responses() {
    let done = json!({ "type": "response.completed", "response": { "usage": { "input_tokens": 10, "output_tokens": 5 } } });
    let reasoning =
        json!({ "type": "reasoning", "id": "rs_1", "encrypted_content": "enc", "summary": [] });
    let (lines, sent) = ask(
        "openai-responses",
        vec![
            named_sse(&[
                json!({ "type": "response.output_item.done", "output_index": 0, "item": reasoning }),
                json!({ "type": "response.output_item.added", "output_index": 1, "item": { "type": "function_call", "call_id": "f1", "name": "remember", "arguments": "" } }),
                json!({ "type": "response.function_call_arguments.delta", "output_index": 1, "delta": "{\"query\":\"sister\"}" }),
                json!({ "type": "response.output_item.done", "output_index": 1, "item": { "type": "function_call", "call_id": "f1", "name": "remember", "arguments": "{\"query\":\"sister\"}" } }),
                done.clone(),
            ]),
            named_sse(&[
                json!({ "type": "response.output_text.delta", "output_index": 0, "delta": "Her name " }),
                json!({ "type": "response.output_text.delta", "output_index": 0, "delta": "is Lindi." }),
                done,
            ]),
        ],
    )
    .await;
    check(&lines);
    assert_eq!(sent[0].path, "/responses");
    assert_eq!(sent[0].body["store"], false);
    assert_eq!(sent[0].body["tools"][0]["name"], "remember");
    let input = sent[1].body["input"].as_array().unwrap();
    let n = input.len();
    assert_eq!(input[n - 3], reasoning);
    assert_eq!(
        input[n - 2],
        json!({ "type": "function_call", "call_id": "f1", "name": "remember", "arguments": "{\"query\":\"sister\"}" })
    );
    assert_eq!(
        input[n - 1],
        json!({ "type": "function_call_output", "call_id": "f1", "output": "Nothing found." })
    );
}

fn bedrock_stream(events: &[(&str, Value)]) -> Reply {
    let body = events
        .iter()
        .flat_map(|(name, payload)| {
            aws_event(
                &[(":event-type", name), (":message-type", "event")],
                &payload.to_string(),
            )
        })
        .collect();
    Reply {
        status: 200,
        content_type: "application/vnd.amazon.eventstream",
        body,
    }
}

#[tokio::test]
async fn bedrock_converse_stream() {
    let metadata = json!({ "usage": { "inputTokens": 10, "outputTokens": 5 } });
    let (lines, sent) = ask(
        "bedrock",
        vec![
            bedrock_stream(&[
                ("messageStart", json!({ "role": "assistant" })),
                ("contentBlockStart", json!({ "contentBlockIndex": 0, "start": { "toolUse": { "toolUseId": "b1", "name": "remember" } } })),
                ("contentBlockDelta", json!({ "contentBlockIndex": 0, "delta": { "toolUse": { "input": "{\"query\":\"sister\"}" } } })),
                ("contentBlockStop", json!({ "contentBlockIndex": 0 })),
                ("messageStop", json!({ "stopReason": "tool_use" })),
                ("metadata", metadata.clone()),
            ]),
            bedrock_stream(&[
                ("contentBlockDelta", json!({ "contentBlockIndex": 0, "delta": { "text": "Her name " } })),
                ("contentBlockDelta", json!({ "contentBlockIndex": 0, "delta": { "text": "is Lindi." } })),
                ("messageStop", json!({ "stopReason": "end_turn" })),
                ("metadata", metadata),
            ]),
        ],
    )
    .await;
    check(&lines);
    assert_eq!(sent[0].path, "/model/m1/converse-stream");
    assert_eq!(
        sent[0].header("authorization"),
        Some("Bearer fake-literal-key")
    );
    assert_eq!(
        sent[0].body["toolConfig"]["tools"][0]["toolSpec"]["name"],
        "remember"
    );
    let messages = sent[1].body["messages"].as_array().unwrap();
    assert_eq!(
        messages[messages.len() - 2]["content"][0]["toolUse"]["input"],
        json!({ "query": "sister" })
    );
    assert_eq!(
        messages[messages.len() - 1]["content"][0]["toolResult"]["toolUseId"],
        "b1"
    );
}

#[tokio::test]
async fn rows_left_by_a_failed_answer_are_merged_so_roles_alternate() {
    let answer = bedrock_stream(&[
        (
            "contentBlockDelta",
            json!({ "contentBlockIndex": 0, "delta": { "text": "Back again." } }),
        ),
        ("messageStop", json!({ "stopReason": "end_turn" })),
        (
            "metadata",
            json!({ "usage": { "inputTokens": 1, "outputTokens": 1 } }),
        ),
    ]);
    let mock = serve(vec![common::status(500, "{\"message\":\"down\"}"), answer]);
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let http = reqwest::Client::new();
    let mut out = Collected::default();

    let failed = dm::respond(
        &store,
        &http,
        &common::request("bedrock", &mock.url, "are you there?"),
        &mut out,
    )
    .await;
    assert!(failed.is_err(), "the first answer fails");
    dm::respond(
        &store,
        &http,
        &common::request("bedrock", &mock.url, "hello?"),
        &mut out,
    )
    .await
    .unwrap();

    let sent = mock.requests();
    let messages = sent[1].body["messages"].as_array().unwrap();
    let roles: Vec<&str> = messages
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        vec!["user"],
        "Bedrock rejects two user messages in a row"
    );
    assert_eq!(
        messages[0]["content"][0]["text"],
        "are you there?\n\nhello?"
    );
}

#[test]
fn aws_event_stream_messages_are_read_across_chunks() {
    let mut bytes = aws_event(
        &[
            (":event-type", "contentBlockDelta"),
            (":message-type", "event"),
        ],
        "{\"a\":1}",
    );
    bytes.extend(aws_event(
        &[
            (":message-type", "exception"),
            (":exception-type", "throttlingException"),
        ],
        "{}",
    ));
    let mut decoder = Framing::AwsEventStream.decoder();
    let mut frames = vec![];
    for chunk in bytes.chunks(7) {
        frames.extend(decoder.push(chunk).unwrap());
    }
    assert!(decoder.finish().unwrap().is_empty());
    assert_eq!(
        frames[0],
        Frame {
            name: Some("contentBlockDelta".into()),
            data: "{\"a\":1}".into()
        }
    );
    assert_eq!(
        frames[1].name.as_deref(),
        Some("exception:throttlingException")
    );
}

#[test]
fn a_corrupt_event_stream_prelude_is_an_error_not_a_wait() {
    let mut prelude = (u32::MAX).to_be_bytes().to_vec();
    prelude.extend_from_slice(&[0; 8]);
    assert!(Framing::AwsEventStream.decoder().push(&prelude).is_err());
}

#[tokio::test]
async fn an_empty_system_is_not_sent() {
    for api in [
        "openai-completions",
        "openai-responses",
        "anthropic-messages",
        "bedrock",
    ] {
        let mock = serve(vec![common::status(200, ""), common::status(200, "")]);
        let providers: dm::config::Providers =
            serde_json::from_value(json!({ "mock": common::provider(api, &mock.url, "fake-key") }))
                .unwrap();
        let model =
            dm::provider::Model::new(reqwest::Client::new(), providers.target(None).unwrap())
                .unwrap();
        for system in ["", " \n "] {
            let chat = dm::provider::chat::Chat {
                system: system.to_string(),
                messages: vec![dm::provider::chat::Message::User("hi".into())],
                tools: vec![],
            };
            model.ask(&chat, &mut |_| {}).await.unwrap();
        }
        for sent in mock.requests() {
            let body = &sent.body;
            assert!(body.get("system").is_none(), "{api}: {body}");
            assert!(body.get("instructions").is_none(), "{api}: {body}");
            let turns = body["messages"].as_array().or(body["input"].as_array());
            assert!(
                turns.unwrap().iter().all(|m| m["role"] != "system"),
                "{api}: {body}"
            );
        }
    }
}

/// The text lines, the done line's reply and the stored messages of one answer.
async fn said(api: &str, reply: Reply) -> (String, Value, Vec<String>) {
    let (lines, _, home) = common::answer(
        |url| common::request(api, url, "what is my sister called?"),
        vec![reply],
    )
    .await;
    let texts = lines
        .iter()
        .filter(|l| l["type"] == "text")
        .map(|l| l["delta"].as_str().unwrap())
        .collect();
    let store = Store::open(home.path()).unwrap();
    let who = dm::who::Who::new("default", Some("thandi")).unwrap();
    let day = store.last_day(&who).unwrap().unwrap();
    let stored = store
        .messages(&who, &day, true)
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    (texts, lines.last().unwrap()["reply"].clone(), stored)
}

#[tokio::test]
async fn reasoning_in_think_tags_is_not_the_reply() {
    let piece = |text: &str| json!({ "choices": [{ "delta": { "content": text } }] });
    let (texts, reply, stored) = said(
        "openai-completions",
        sse(&[
            piece("<thi"),
            piece("nk>She is ca"),
            piece("lled Lindi</th"),
            piece("ink>\n\nHer name "),
            piece("is Lindi."),
        ]),
    )
    .await;
    assert_eq!(texts, "Her name is Lindi.");
    assert_eq!(reply, "Her name is Lindi.");
    assert_eq!(
        stored,
        vec!["what is my sister called?", "Her name is Lindi."]
    );
}

#[tokio::test]
async fn a_closing_tag_alone_takes_what_came_before_out_of_the_reply() {
    let piece = |text: &str| {
        (
            "contentBlockDelta",
            json!({ "contentBlockIndex": 0, "delta": { "text": text } }),
        )
    };
    let (_, reply, stored) = said(
        "bedrock",
        bedrock_stream(&[
            piece("They asked about Lindi.</thi"),
            piece("nk>\n Her name is Lindi."),
            ("messageStop", json!({ "stopReason": "end_turn" })),
        ]),
    )
    .await;
    assert_eq!(reply, "Her name is Lindi.");
    assert_eq!(stored[1], "Her name is Lindi.");
}

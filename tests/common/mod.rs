//! A local mock provider: answers each request with the next scripted response and keeps what it was sent.

#![allow(dead_code)]

use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
};

#[derive(Debug, Clone)]
pub struct Received {
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl Received {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

fn sse_with(events: &[Value], named: bool) -> Reply {
    let mut body: String = events
        .iter()
        .map(|e| match named {
            true => format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap_or("")),
            false => format!("data: {e}\n\n"),
        })
        .collect();
    if !named {
        body.push_str("data: [DONE]\n\n");
    }
    Reply {
        status: 200,
        content_type: "text/event-stream",
        body: body.into_bytes(),
    }
}

pub fn sse(events: &[Value]) -> Reply {
    sse_with(events, false)
}

/// Server-sent events with names, as Anthropic and OpenAI Responses send them.
pub fn named_sse(events: &[Value]) -> Reply {
    sse_with(events, true)
}

/// A chat-completions reply that says `text` in one piece.
pub fn text_reply(text: &str) -> Reply {
    sse(&[
        json!({ "choices": [{ "delta": { "content": text } }], "usage": { "prompt_tokens": 3, "completion_tokens": 2 } }),
    ])
}

pub fn status(code: u16, body: &str) -> Reply {
    Reply {
        status: code,
        content_type: "application/json",
        body: body.as_bytes().to_vec(),
    }
}

pub struct Mock {
    pub url: String,
    pub received: Arc<Mutex<Vec<Received>>>,
}

impl Mock {
    pub fn requests(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }
}

/// A port above 40000 that is free now.
fn listener() -> TcpListener {
    let seed = std::process::id() as u16
        ^ (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos() as u16);
    for n in 0..2000u16 {
        let port = 40_000 + (seed.wrapping_add(n.wrapping_mul(7919)) % 20_000);
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
            return listener;
        }
    }
    panic!("no free port above 40000");
}

pub fn serve(replies: Vec<Reply>) -> Mock {
    let listener = listener();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let received = Arc::new(Mutex::new(vec![]));
    let kept = received.clone();
    thread::spawn(move || {
        let mut replies = replies.into_iter();
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let reply = replies
                .next()
                .unwrap_or_else(|| status(500, "{\"error\":\"no more scripted replies\"}"));
            if let Some(got) = handle(stream, reply) {
                kept.lock().unwrap().push(got);
            }
        }
    });
    Mock { url, received }
}

fn handle(mut stream: TcpStream, reply: Reply) -> Option<Received> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut first = String::new();
    reader.read_line(&mut first).ok()?;
    let path = first.split_whitespace().nth(1)?.to_string();
    let mut headers = vec![];
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    let head = format!(
        "HTTP/1.1 {} X\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        reply.status,
        reply.content_type,
        reply.body.len()
    );
    stream.write_all(head.as_bytes()).ok()?;
    stream.write_all(&reply.body).ok()?;
    Some(Received {
        path,
        headers,
        body: serde_json::from_slice(&body).unwrap_or(json!(null)),
    })
}

/// A provider on the mock with one default model.
pub fn provider(api: &str, url: &str, key: &str) -> Value {
    json!({
        "api": api, "baseUrl": url, "apiKey": key,
        "models": [{ "id": "m1", "default": true, "cost": { "input": 1.0, "output": 2.0 } }]
    })
}

/// A request for one provider on the mock.
pub fn request(api: &str, url: &str, message: &str) -> dm::Request {
    serde_json::from_value(json!({
        "providers": { "mock": provider(api, url, "fake-literal-key") },
        "person": "thandi",
        "timeZone": "Africa/Johannesburg",
        "message": message,
    }))
    .unwrap()
}

/// Answers `request` against a mock that sends `replies`: the lines out, what the mock was sent, and the home.
pub async fn answer(
    request: impl FnOnce(&str) -> dm::Request,
    replies: Vec<Reply>,
) -> (Vec<Value>, Vec<Received>, tempfile::TempDir) {
    let mock = serve(replies);
    let home = tempfile::tempdir().unwrap();
    let store = dm::Store::open(home.path()).unwrap();
    let mut out = dm::output::Collected::default();
    dm::respond(
        &store,
        &reqwest::Client::new(),
        &request(&mock.url),
        &mut out,
    )
    .await
    .unwrap();
    (out.0, mock.requests(), home)
}

/// One AWS event stream message, as Bedrock sends it (with zeroed CRCs, which are not checked).
pub fn aws_event(headers: &[(&str, &str)], payload: &str) -> Vec<u8> {
    let mut encoded = vec![];
    for (name, value) in headers {
        encoded.push(name.len() as u8);
        encoded.extend_from_slice(name.as_bytes());
        encoded.push(7);
        encoded.extend_from_slice(&(value.len() as u16).to_be_bytes());
        encoded.extend_from_slice(value.as_bytes());
    }
    let total = 12 + encoded.len() + payload.len() + 4;
    let mut message = vec![];
    message.extend_from_slice(&(total as u32).to_be_bytes());
    message.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
    message.extend_from_slice(&[0; 4]);
    message.extend_from_slice(&encoded);
    message.extend_from_slice(payload.as_bytes());
    message.extend_from_slice(&[0; 4]);
    message
}

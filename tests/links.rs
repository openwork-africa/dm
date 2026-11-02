//! `remember` with links: what a link returns is read, cut into passages, ranked against the query and capped at six,
//! each with its link (shown without its signature); size, time and scheme limits; one link or several.

mod common;

use common::{serve, sse, text_reply, Reply};
use dm::memory::links::{self, Limits, HITS};
use serde_json::{json, Value};
use std::{
    io::Read,
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};

fn text(body: &str, content_type: &'static str) -> Reply {
    Reply {
        status: 200,
        content_type,
        body: body.as_bytes().to_vec(),
    }
}

fn call_remember(arguments: Value) -> Reply {
    sse(&[
        json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "id": "t1", "function": { "name": "remember", "arguments": arguments.to_string() } }] } }] }),
    ])
}

/// A note of many paragraphs, one of which is about saffron.
fn note() -> String {
    let mut text = String::from("# Kitchen notes\n\n");
    for n in 0..30 {
        text.push_str(&format!(
            "Day {n}: the bakery opened early and bread was sold to the neighbours. "
        ));
        text.push_str(&"The ovens were warm and the street was busy. ".repeat(8));
        text.push_str("\n\n");
    }
    text.push_str("The family recipe for the stew needs a pinch of saffron and slow cooking.\n\n");
    text
}

async fn answer_reading(arguments: impl FnOnce(&str) -> Value, content: Vec<Reply>) -> String {
    let site = serve(content);
    let mock = serve(vec![
        call_remember(arguments(&site.url)),
        text_reply("Done."),
    ]);
    let request = common::request("openai-completions", &mock.url, "what goes in the stew?");
    let home = tempfile::tempdir().unwrap();
    let store = dm::Store::open(home.path()).unwrap();
    let mut out = dm::output::Collected::default();
    dm::respond(&store, &reqwest::Client::new(), &request, &mut out)
        .await
        .unwrap();
    let sent = mock.requests();
    let described = sent[0].body["tools"][0]["function"].clone();
    assert!(described["description"]
        .as_str()
        .unwrap()
        .contains("Content you have access to"));
    assert!(described["parameters"]["properties"]["url"].is_object());
    out.0.iter().find(|l| l["type"] == "tool_result").unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_link_is_read_and_its_best_passages_come_back_with_it() {
    let result = answer_reading(
        |site| json!({ "query": "stew saffron", "url": format!("{site}/content/note-1?sig=very-secret") }),
        vec![text(&note(), "text/markdown; charset=utf-8")],
    )
    .await;
    assert!(
        result.starts_with("Kitchen notes (http://"),
        "titled by its heading: {result}"
    );
    let first = result.split("\n\nKitchen notes (").next().unwrap();
    assert!(
        first.contains("/content/note-1):\n")
            && first.contains("The family recipe for the stew needs a pinch of saffron"),
        "the best passage comes first: {first}"
    );
    assert!(
        !result.contains("very-secret"),
        "the signature is never shown back"
    );
    assert!(result.matches("/content/note-1):").count() <= HITS);
}

#[tokio::test]
async fn at_most_six_passages_come_back() {
    let result = answer_reading(
        |site| json!({ "query": "bakery bread", "url": format!("{site}/content/note-1") }),
        vec![text(&note(), "text/plain")],
    )
    .await;
    assert_eq!(result.matches("/content/note-1):").count(), HITS);
}

#[tokio::test]
async fn several_links_are_read_and_without_a_query_the_first_passages_come() {
    let result = answer_reading(
        |site| json!({ "url": [format!("{site}/content/a"), format!("{site}/content/b")] }),
        vec![
            text("# A\n\nAlpha first.", "text/plain"),
            text("# B\n\nBeta first.", "text/plain"),
        ],
    )
    .await;
    assert!(result.contains("A (http://") && result.contains("B (http://"));
    assert!(result.contains("Alpha first.") && result.contains("Beta first."));
}

#[tokio::test]
async fn a_link_that_fails_says_why_and_the_others_are_still_read() {
    let result = answer_reading(
        |site| json!({ "query": "alpha", "url": [format!("{site}/content/gone?sig=x"), format!("{site}/content/a")] }),
        vec![
            common::status(410, "{}"),
            text("Alpha is here.", "text/plain"),
        ],
    )
    .await;
    assert!(result.contains("Alpha is here."));
    assert!(result.contains("/content/gone could not be read: it answered 410"));
    assert!(!result.contains("sig=x"));
}

#[tokio::test]
async fn a_file_that_is_not_text_is_refused() {
    let site = serve(vec![text("%PDF-1.7", "application/pdf")]);
    let err = links::read(&format!("{}/file", site.url), Limits::default())
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("only text can be read"));
}

#[tokio::test]
async fn only_https_or_this_machine_is_read() {
    for link in [
        "http://example.com/content/x",
        "file:///etc/hosts",
        "not a link",
    ] {
        assert!(
            links::read(link, Limits::default()).await.is_err(),
            "{link} is refused"
        );
    }
}

#[tokio::test]
async fn more_than_the_size_cap_is_refused() {
    let big = "word ".repeat(1_200_000);
    assert!(big.len() > links::MAX_BYTES);
    let site = serve(vec![text(&big, "text/plain")]);
    let err = links::read(&format!("{}/big", site.url), Limits::default())
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("larger than 5120 KiB"));
    let small = serve(vec![text(&"x".repeat(2048), "text/plain")]);
    let capped = Limits {
        max_bytes: 1024,
        ..Limits::default()
    };
    assert!(links::read(&format!("{}/small", small.url), capped)
        .await
        .is_err());
}

#[tokio::test]
async fn a_link_that_does_not_answer_is_given_up_on() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/slow", listener.local_addr().unwrap());
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
            thread::sleep(Duration::from_secs(10));
        }
    });
    let limits = Limits {
        timeout: Duration::from_secs(1),
        ..Limits::default()
    };
    let started = Instant::now();
    let err = links::read(&url, limits).await.unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(format!("{err:#}").contains("longer than 1 seconds"));
    assert_eq!(links::TIMEOUT, Duration::from_secs(20));
}

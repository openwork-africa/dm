//! Transcripts, summaries, remember and the lookup decision.

mod common;

use common::{serve, sse, Reply};
use dm::{
    compaction::{COMPACT_AFTER, KEEP_RECENT},
    config::Providers,
    jobs::{run_due, Job, Ran},
    output::Collected,
    who::Who,
    Store,
};
use serde_json::{json, Value};
use std::fs;

fn text_reply(text: &str) -> Reply {
    sse(&[
        json!({ "choices": [{ "delta": { "content": text } }], "usage": { "prompt_tokens": 3, "completion_tokens": 2 } }),
    ])
}

fn notes_call() -> Reply {
    let notes = json!({
        "howTheyTalk": { "value": "short messages; lowercase", "quote": "ok cool" },
        "people": { "value": "Lindi (sister)", "quote": "my sister Lindi" },
        "interests": { "value": "jazz", "quote": "I love jazz" },
    });
    sse(&[
        json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "id": "n1", "function": { "name": "notes", "arguments": notes.to_string() } }] } }] }),
    ])
}

fn who() -> Who {
    Who::new("default", Some("thandi")).unwrap()
}

#[tokio::test]
async fn sixteen_messages_compact_all_but_the_newest_four() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let day = "2026-11-01";
    for n in 0..COMPACT_AFTER - 1 {
        store
            .add_message(
                &who(),
                day,
                if n % 2 == 0 { "user" } else { "assistant" },
                &format!("message {n}"),
            )
            .unwrap();
        store.after_message(&who(), day, "mock/m1").unwrap();
    }
    assert!(store.pending_jobs().unwrap().is_empty());
    store
        .add_message(&who(), day, "assistant", "message 15")
        .unwrap();
    store.after_message(&who(), day, "mock/m1").unwrap();
    assert_eq!(
        store.pending_jobs().unwrap(),
        vec![Job::compact(&who(), day, false, "mock/m1")]
    );

    let ran = run_due(&store, None, &reqwest::Client::new())
        .await
        .unwrap();
    assert_eq!(
        ran,
        Ran {
            done: 1,
            failed: 0,
            waiting: 0
        }
    );
    let history = store.history(&who(), day).unwrap();
    assert_eq!(history.len(), KEEP_RECENT);
    assert_eq!(history[0].content, "message 12");
    let transcript = fs::read_to_string(
        home.path()
            .join("transcripts/default/thandi/2026-11-01-1.md"),
    )
    .unwrap();
    assert!(transcript.starts_with("# 2026-11-01\n\nPerson: message 0\nAgent: message 1\n"));
    assert!(transcript.contains("Agent: message 11\n") && !transcript.contains("message 12"));
    assert_eq!(
        store.messages(&who(), day, true).unwrap().len(),
        COMPACT_AFTER
    );
}

#[tokio::test]
async fn a_new_day_closes_the_last_and_summarises_it_in_two_layers() {
    let mock = serve(vec![text_reply("Morning!"), notes_call()]);
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let old = "2020-01-01";
    store.move_to_day(&who(), old).unwrap();
    store
        .add_message(
            &who(),
            old,
            "user",
            "my sister Lindi plays piano. I love jazz",
        )
        .unwrap();
    store
        .add_message(&who(), old, "assistant", "ok cool")
        .unwrap();

    let request = common::request("openai-completions", &mock.url, "good morning");
    let mut out = Collected::default();
    dm::respond(&store, &reqwest::Client::new(), &request, &mut out)
        .await
        .unwrap();
    assert_eq!(out.0.last().unwrap()["reply"], "Morning!");
    let ran = run_due(&store, Some(&request.providers), &reqwest::Client::new())
        .await
        .unwrap();
    assert_eq!(
        ran,
        Ran {
            done: 2,
            failed: 0,
            waiting: 0
        }
    );

    assert!(store.history(&who(), old).unwrap().is_empty());
    assert!(home
        .path()
        .join("transcripts/default/thandi/2020-01-01-1.md")
        .exists());
    let summary_call = &mock.requests()[1].body;
    assert!(summary_call["messages"][1]["content"]
        .as_str()
        .unwrap()
        .contains("Person: my sister Lindi plays piano"));

    // What you know: in the notes file, not the prompt.
    let known = fs::read_to_string(home.path().join("notes/default/thandi.md")).unwrap();
    assert!(
        known.contains("- People in their life: Lindi (sister)")
            && known.contains("- What they love: jazz")
    );
    assert!(!known.contains("How they talk"));
    // How to be with them: in the prompt of the next answer.
    let next = serve(vec![text_reply("Hi again")]);
    let mut request = common::request("openai-completions", &next.url, "hello again");
    request.lookup = None;
    dm::respond(
        &store,
        &reqwest::Client::new(),
        &request,
        &mut Collected::default(),
    )
    .await
    .unwrap();
    let system = next.requests()[0].body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(system.contains("- How they talk: short messages; lowercase"));
    assert!(!system.contains("jazz"));
}

#[tokio::test]
async fn remember_finds_transcripts_and_notes_from_the_files() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("transcripts/default/thandi");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("2026-10-30-1.md"), "# 2026-10-30\n\nPerson: my sister Lindi starts at Wits\nnext month\nAgent: That is exciting!\n").unwrap();
    fs::create_dir_all(home.path().join("notes/default")).unwrap();
    fs::write(
        home.path().join("notes/default/thandi.md"),
        "# What you know about them\n\n- What they love: jazz; braai\n",
    )
    .unwrap();

    let store = Store::open(home.path()).unwrap();
    store.load_memory(&who()).unwrap();
    let found = store.recall(&who(), "Lindi").unwrap();
    assert!(found.starts_with(
        "2026-10-30:\nPerson: my sister Lindi starts at Wits\nnext month\nAgent: That is exciting!"
    ));
    assert!(store
        .recall(&who(), "jazz")
        .unwrap()
        .contains("What they love: jazz; braai"));
    assert!(
        store.recall(&who(), "music").unwrap().contains("jazz"),
        "found by the kind of note"
    );
    assert_eq!(store.recall(&who(), "zebra").unwrap(), "Nothing found.");
    let other = Who::new("default", Some("sipho")).unwrap();
    assert_eq!(store.recall(&other, "Lindi").unwrap(), "Nothing found.");
}

async fn with_lookup(decision: &str) -> (Value, Value) {
    let mock = serve(vec![text_reply(decision), text_reply("Sure.")]);
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let mut request = common::request(
        "openai-completions",
        &mock.url,
        "what did I say my sister does?",
    );
    request.lookup = Some(dm::request::Lookup {
        model: Some("mock/m1".into()),
    });
    let mut out = Collected::default();
    dm::respond(&store, &reqwest::Client::new(), &request, &mut out)
        .await
        .unwrap();
    let sent = mock.requests();
    assert!(sent[0].body["messages"][1]["content"]
        .as_str()
        .unwrap()
        .contains("what did I say my sister does?"));
    assert!(sent[0].body.get("tools").is_none());
    let done = out.0.last().unwrap().clone();
    (sent[1].body.clone(), done)
}

#[tokio::test]
async fn lookup_yes_offers_remember_and_asks_for_it() {
    let (answer, done) = with_lookup("Yes").await;
    assert_eq!(answer["tools"][0]["function"]["name"], "remember");
    assert!(answer["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("Before you answer this message, call remember"));
    assert_eq!(done["usage"]["input"], 6, "the decision's tokens count too");
}

#[tokio::test]
async fn lookup_no_offers_no_tool() {
    let (answer, _) = with_lookup("no").await;
    assert!(answer.get("tools").is_none());
    assert!(!answer["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("remember"));
}

#[tokio::test]
async fn summaries_wait_for_providers() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    store
        .add_message(&who(), "2026-10-31", "user", "hi")
        .unwrap();
    store.close_day(&who(), "2026-10-31", "mock/m1").unwrap();
    let ran = run_due(&store, None, &reqwest::Client::new())
        .await
        .unwrap();
    assert_eq!(
        ran,
        Ran {
            done: 1,
            failed: 0,
            waiting: 1
        }
    );
    let providers: Providers = serde_json::from_value(json!({})).unwrap();
    let ran = run_due(&store, Some(&providers), &reqwest::Client::new())
        .await
        .unwrap();
    assert_eq!(
        ran.failed, 1,
        "an unknown provider is a failure, retried later"
    );
}

#[test]
fn two_processes_at_a_day_change_give_one_close() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    store.move_to_day(&who(), "2026-10-31").unwrap();
    for round in 0..5 {
        let day = format!("2026-11-0{}", round + 1);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (path, day, barrier) =
                    (home.path().to_path_buf(), day.clone(), barrier.clone());
                std::thread::spawn(move || {
                    let store = Store::open(&path).unwrap();
                    barrier.wait();
                    store.move_to_day(&who(), &day).unwrap()
                })
            })
            .collect();
        let closed = threads
            .into_iter()
            .filter_map(|t| t.join().unwrap())
            .count();
        assert_eq!(closed, 1, "{day} was closed {closed} times");
    }
}

#[tokio::test]
async fn a_claimed_job_is_not_run_twice() {
    let mock = serve(vec![notes_call(), notes_call()]);
    let home = tempfile::tempdir().unwrap();
    let (first, second) = (
        Store::open(home.path()).unwrap(),
        Store::open(home.path()).unwrap(),
    );
    let dir = home.path().join("transcripts/default/thandi");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("2026-10-31-1.md"),
        "# 2026-10-31\n\nPerson: I love jazz\n",
    )
    .unwrap();
    first
        .enqueue(&Job::Summary {
            agent: "default".into(),
            person: "thandi".into(),
            day: "2026-10-31".into(),
            model: "mock/m1".into(),
        })
        .unwrap();
    let request = common::request("openai-completions", &mock.url, "unused");
    let http = reqwest::Client::new();
    let (a, b) = tokio::join!(
        run_due(&first, Some(&request.providers), &http),
        run_due(&second, Some(&request.providers), &http)
    );
    assert_eq!(a.unwrap().done + b.unwrap().done, 1);
    assert_eq!(mock.requests().len(), 1, "the model was asked once");
}

#[test]
fn a_failed_compaction_leaves_no_orphan_transcript() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let day = "2026-11-01";
    store.add_message(&who(), day, "user", "hello").unwrap();
    rusqlite::Connection::open(home.path().join("dm.db"))
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER no_marking BEFORE UPDATE ON messages
             BEGIN SELECT RAISE(ABORT, 'disk trouble'); END;",
        )
        .unwrap();

    assert!(store.compact(&who(), day, true, "mock/m1").is_err());
    let dir = home.path().join("transcripts/default/thandi");
    assert!(
        fs::read_dir(dir).unwrap().next().is_none(),
        "the transcript was removed with the failure"
    );
    assert_eq!(store.history(&who(), day).unwrap().len(), 1);
}

#[tokio::test]
async fn notes_are_written_through_a_part_file_and_renamed() {
    let mock = serve(vec![notes_call(), notes_call()]);
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let transcripts = home.path().join("transcripts/default/thandi");
    fs::create_dir_all(&transcripts).unwrap();
    fs::write(
        transcripts.join("2026-10-31-1.md"),
        "# 2026-10-31\n\nPerson: I love jazz\n",
    )
    .unwrap();
    let notes = home.path().join("notes/default");
    fs::create_dir_all(&notes).unwrap();
    fs::write(notes.join("thandi.md"), "- old notes\n").unwrap();
    let summary = Job::Summary {
        agent: "default".into(),
        person: "thandi".into(),
        day: "2026-10-31".into(),
        model: "mock/m1".into(),
    };
    let request = common::request("openai-completions", &mock.url, "unused");
    let http = reqwest::Client::new();

    // With the part file's place taken, the write fails and the notes file is as it was.
    fs::create_dir(notes.join("thandi.md.part")).unwrap();
    store.enqueue(&summary).unwrap();
    let ran = run_due(&store, Some(&request.providers), &http)
        .await
        .unwrap();
    assert_eq!(ran.failed, 1);
    assert_eq!(
        fs::read_to_string(notes.join("thandi.md")).unwrap(),
        "- old notes\n"
    );

    fs::remove_dir(notes.join("thandi.md.part")).unwrap();
    store.enqueue(&summary).unwrap();
    rusqlite::Connection::open(home.path().join("dm.db"))
        .unwrap()
        .execute("UPDATE jobs SET run_after = 0", [])
        .unwrap();
    let ran = run_due(&store, Some(&request.providers), &http)
        .await
        .unwrap();
    assert_eq!(ran.done, 1);
    assert!(fs::read_to_string(notes.join("thandi.md"))
        .unwrap()
        .contains("jazz"));
    assert!(!notes.join("thandi.md.part").exists());
}

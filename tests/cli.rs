//! The `dm` binary: called the way me calls its harnesses, keys resolved without ever being printed, and its commands.

mod common;

use common::{serve, sse, status};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

const ENV_KEY: &str = "fake-env-secret-1234";
const CMD_KEY: &str = "fake-cmd-secret-5678";

fn providers_file(dir: &Path, url: &str, api_key: &str) -> std::path::PathBuf {
    let path = dir.join("providers.json");
    let file =
        json!({ "providers": { "mock": common::provider("openai-completions", url, api_key) } });
    fs::write(&path, file.to_string()).unwrap();
    path
}

fn dm(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_dm"))
        .args(args)
        .env("DM_HOME", home)
        .env("DM_TEST_KEY", ENV_KEY)
        .env("TZ", "Africa/Johannesburg")
        .output()
        .unwrap()
}

fn answer() -> common::Reply {
    sse(&[
        json!({ "choices": [{ "delta": { "content": "Hello " } }] }),
        json!({ "choices": [{ "delta": { "content": "there" } }], "usage": { "prompt_tokens": 4, "completion_tokens": 2 } }),
    ])
}

#[test]
fn called_like_the_other_harnesses() {
    let mock = serve(vec![answer()]);
    let home = tempfile::tempdir().unwrap();
    providers_file(home.path(), &mock.url, "$DM_TEST_KEY");
    let out = dm(
        home.path(),
        &[
            "-p",
            "hi there",
            "--output-format",
            "stream-json",
            "--model",
            "mock/m1",
            "--system",
            "Be brief.",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0], json!({ "type": "text", "delta": "Hello " }));
    assert_eq!(lines[2]["type"], "done");
    assert_eq!(lines[2]["reply"], "Hello there");
    let sent = &mock.requests()[0];
    assert_eq!(
        sent.header("authorization"),
        Some(format!("Bearer {ENV_KEY}").as_str())
    );
    assert!(sent.body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("Your instructions:\nBe brief."));
    assert!(!stdout.contains(ENV_KEY) && !String::from_utf8_lossy(&out.stderr).contains(ENV_KEY));

    let history = dm(home.path(), &["history"]);
    let said: Vec<Value> = String::from_utf8(history.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        (said[0]["role"].as_str(), said[0]["content"].as_str()),
        (Some("user"), Some("hi there"))
    );
    assert_eq!(said[1]["content"], "Hello there");
}

#[test]
fn print_alone_gives_plain_text() {
    let mock = serve(vec![answer()]);
    let home = tempfile::tempdir().unwrap();
    let providers = providers_file(home.path(), &mock.url, "literal");
    let out = dm(
        home.path(),
        &[
            "-p",
            "hi",
            "--providers",
            providers.to_str().unwrap(),
            "--no-jobs",
        ],
    );
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "Hello there\n");
}

#[test]
fn a_request_file_answers_with_json_lines() {
    let mock = serve(vec![answer()]);
    let home = tempfile::tempdir().unwrap();
    let request = home.path().join("request.json");
    let body = json!({
        "providers": { "mock": { "api": "openai-completions", "baseUrl": mock.url, "apiKey": "x", "models": [{ "id": "m1" }] } },
        "model": "mock/m1", "agent": "helper", "person": "p1", "timeZone": "UTC", "message": "hi"
    });
    fs::write(&request, body.to_string()).unwrap();
    let out = dm(home.path(), &["--request", request.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .last()
        .unwrap()
        .contains("\"type\":\"done\""));
    assert!(home.path().join("dm.db").exists());
}

#[test]
fn a_refused_key_fails_without_printing_it() {
    for api_key in ["$DM_TEST_KEY".to_string(), format!("!printf {CMD_KEY}")] {
        let mock = serve(vec![status(
            401,
            "{\"error\":{\"message\":\"invalid key\"}}",
        )]);
        let home = tempfile::tempdir().unwrap();
        providers_file(home.path(), &mock.url, &api_key);
        let out = dm(home.path(), &["-p", "hi", "--json"]);
        assert!(!out.status.success());
        let (stdout, stderr) = (
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        assert!(
            stderr.contains("401") && stderr.contains("invalid key"),
            "{stderr}"
        );
        for secret in [ENV_KEY, CMD_KEY] {
            assert!(
                !stdout.contains(secret) && !stderr.contains(secret),
                "{secret} leaked"
            );
        }
    }
}

#[test]
fn a_missing_key_names_the_variable() {
    let home = tempfile::tempdir().unwrap();
    providers_file(home.path(), "http://127.0.0.1:1", "$DM_TEST_UNSET_KEY");
    let out = dm(home.path(), &["-p", "hi"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("DM_TEST_UNSET_KEY"));
}

#[test]
fn version_and_jobs() {
    let home = tempfile::tempdir().unwrap();
    let version = dm(home.path(), &["--version"]);
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        format!("dm {}", env!("CARGO_PKG_VERSION"))
    );
    let jobs = dm(home.path(), &["jobs"]);
    assert!(jobs.status.success());
    assert_eq!(
        String::from_utf8(jobs.stdout).unwrap().trim(),
        "{\"done\":0,\"failed\":0,\"waiting\":0}"
    );
}

#[test]
fn key_commands_find_the_home_without_home_set() {
    let mock = serve(vec![answer()]);
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join("key"), format!("{CMD_KEY}\n")).unwrap();
    let path = home.path().join("providers.json");
    let mut provider = common::provider("openai-completions", &mock.url, "!cat \"$DM_HOME/key\"");
    provider["headers"] = json!({ "x-home": "!printf %s ~", "x-home-env": "!printf %s \"$HOME\"" });
    fs::write(
        &path,
        json!({ "providers": { "mock": provider } }).to_string(),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_dm"))
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "-p",
            "hi",
            "--json",
        ])
        .env_remove("HOME")
        .env_remove("DM_HOME")
        .env("TZ", "Africa/Johannesburg")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sent = &mock.requests()[0];
    assert_eq!(
        sent.header("authorization"),
        Some(format!("Bearer {CMD_KEY}").as_str())
    );
    let tilde = sent.header("x-home").unwrap();
    assert!(
        tilde.starts_with('/') && Path::new(tilde).is_dir(),
        "{tilde}"
    );
    assert_eq!(sent.header("x-home-env"), Some(tilde));
    assert!(!String::from_utf8_lossy(&out.stdout).contains(CMD_KEY));
}

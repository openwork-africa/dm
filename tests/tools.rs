//! Tools from JSON: a JavaScript skill under Deno (described, run, held to what it declares, stopped when slow, kept by
//! hash), an HTTP skill, and where the tools JSON comes from.

mod common;

use common::{serve, sse, status, text_reply, Received, Reply};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command, time::Instant};

const ECHO: &str = r#"
export const skill = {
  name: "echo",
  description: "Says the text back.",
  input: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
  env: ["GREETING"],
};
export default async function ({ text }) {
  return { said: text, greetingLength: process.env.GREETING.length };
}
"#;

fn module(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(format!("{name}.js"));
    fs::write(&path, body).unwrap();
    path.display().to_string()
}

/// A skill declaring `declares`, whose default export is `body`.
fn skill_with(declares: &str, body: &str) -> String {
    format!(
        "export const skill = {{ name: \"probe\", description: \"Probes.\", input: {{ type: \"object\" }}, {declares} }};\nexport default async function () {{ {body} }}\n"
    )
}

fn call(name: &str, arguments: Value) -> Reply {
    sse(&[
        json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "id": "t1", "function": { "name": name, "arguments": arguments.to_string() } }] } }] }),
    ])
}

/// One answer in `home` with these tools, the model calling `name` with `arguments` first.
async fn answer_with(
    home: &Path,
    tools: Value,
    name: &str,
    arguments: Value,
) -> (Vec<Value>, Vec<Received>) {
    let mock = serve(vec![call(name, arguments), text_reply("Done.")]);
    let mut request = common::request("openai-completions", &mock.url, "go");
    request.tools = Some(serde_json::from_value(tools).unwrap());
    let store = dm::Store::open(home).unwrap();
    let mut out = dm::output::Collected::default();
    dm::respond(&store, &reqwest::Client::new(), &request, &mut out)
        .await
        .unwrap();
    (out.0, mock.requests())
}

fn result_of(lines: &[Value]) -> String {
    lines.iter().find(|l| l["type"] == "tool_result").unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string()
}

fn offered(sent: &Received) -> Vec<String> {
    sent.body["tools"].as_array().map_or(vec![], |tools| {
        tools
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_string())
            .collect()
    })
}

#[tokio::test]
async fn a_js_skill_describes_itself_and_runs_with_its_env() {
    std::env::set_var("DM_TEST_GREETING", "fake-greeting-value");
    let home = tempfile::tempdir().unwrap();
    let echo = module(home.path(), "echo", ECHO);
    let tools = json!({ "remember": { "builtin": true }, "echo": { "js": echo, "env": { "GREETING": "$DM_TEST_GREETING" } } });
    let (lines, sent) = answer_with(home.path(), tools, "echo", json!({ "text": "hi" })).await;
    assert_eq!(offered(&sent[0]), ["echo", "remember"]);
    let spec = &sent[0].body["tools"][0]["function"];
    assert_eq!(spec["description"], "Says the text back.");
    assert_eq!(spec["parameters"]["required"], json!(["text"]));
    assert!(sent[0].body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("One of your tools is remember."));
    let result: Value = serde_json::from_str(&result_of(&lines)).unwrap();
    assert_eq!(result, json!({ "said": "hi", "greetingLength": 19 }));
    assert_eq!(lines.last().unwrap()["reply"], "Done.");
}

#[tokio::test]
async fn a_js_skill_is_refused_what_it_did_not_declare() {
    std::env::set_var("DM_TEST_UNDECLARED", "fake-undeclared-value");
    let home = tempfile::tempdir().unwrap();
    let blocked = serve(vec![status(200, "{}")]);
    for (declares, body, refusal) in [
        (
            "net: [\"example.com\"]",
            format!("return (await fetch(\"{}\")).status;", blocked.url),
            "Requires net access",
        ),
        (
            "",
            "return (await import(\"node:fs\")).readFileSync(\"/etc/hosts\", \"utf8\");".into(),
            "Requires read access",
        ),
        (
            "env: [\"GREETING\"]",
            "return process.env.DM_TEST_UNDECLARED;".into(),
            "Requires env access",
        ),
        (
            "",
            "return Deno.env.toObject();".into(),
            "Requires env access",
        ),
    ] {
        let probe = module(home.path(), "probe", &skill_with(declares, &body));
        let (lines, _) = answer_with(
            home.path(),
            json!({ "probe": { "js": probe } }),
            "probe",
            json!({}),
        )
        .await;
        let result = result_of(&lines);
        assert!(
            result.starts_with("probe failed: ") && result.contains(refusal),
            "{result}"
        );
        assert_eq!(
            lines.last().unwrap()["reply"],
            "Done.",
            "the answer goes on"
        );
    }
    assert!(
        blocked.requests().is_empty(),
        "the undeclared host was never reached"
    );
}

#[tokio::test]
async fn a_js_skill_never_shows_its_env_values() {
    std::env::set_var("DM_TEST_SKILL_SECRET", "fake-skill-secret-91");
    let home = tempfile::tempdir().unwrap();
    let tools = |body: &str| {
        let leak = module(home.path(), "leak", &skill_with("env: [\"KEY\"]", body));
        json!({ "leak": { "js": leak, "env": { "KEY": "$DM_TEST_SKILL_SECRET" } } })
    };
    for body in [
        "return { key: process.env.KEY };",
        "throw new Error(`bad ${process.env.KEY}`);",
    ] {
        let (lines, _) = answer_with(home.path(), tools(body), "leak", json!({})).await;
        let result = result_of(&lines);
        assert!(
            result.contains("***") && !result.contains("fake-skill-secret-91"),
            "{result}"
        );
    }
}

#[tokio::test]
async fn a_slow_js_skill_is_stopped_after_thirty_seconds() {
    let home = tempfile::tempdir().unwrap();
    let slow = module(
        home.path(),
        "slow",
        &skill_with("", "await new Promise((done) => setTimeout(done, 120000));"),
    );
    let started = Instant::now();
    let (lines, _) = answer_with(
        home.path(),
        json!({ "slow": { "js": slow } }),
        "slow",
        json!({}),
    )
    .await;
    let took = started.elapsed().as_secs();
    assert!((30..60).contains(&took), "took {took}s");
    assert!(result_of(&lines).contains("took longer than 30 seconds"));
    let running = Command::new("pgrep")
        .args(["-f", "skills/slow/"])
        .output()
        .unwrap();
    assert!(running.stdout.is_empty(), "the skill was killed");
}

#[tokio::test]
async fn a_js_skill_is_kept_by_hash_and_a_changed_one_is_refused() {
    let host = serve(vec![
        Reply {
            status: 200,
            content_type: "text/javascript",
            body: ECHO.as_bytes().to_vec(),
        },
        Reply {
            status: 200,
            content_type: "text/javascript",
            body: b"export const skill = {};".to_vec(),
        },
    ]);
    let hash = dm::tools::sha256_hex(ECHO.as_bytes());
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("DM_TEST_GREETING", "fake-greeting-value");
    let pinned = |sha256: &str| json!({ "echo": { "js": format!("{}/skill/echo/index.js", host.url), "sha256": sha256, "env": { "GREETING": "$DM_TEST_GREETING" } } });
    for _ in 0..2 {
        let (lines, _) = answer_with(
            home.path(),
            pinned(&hash),
            "echo",
            json!({ "text": "again" }),
        )
        .await;
        assert!(result_of(&lines).contains("\"said\":\"again\""));
    }
    assert_eq!(
        host.requests().len(),
        1,
        "fetched once, then read from the cache"
    );
    let kept = home.path().join("skills/echo");
    assert_eq!(
        fs::read_to_string(kept.join(format!("{hash}.js"))).unwrap(),
        ECHO
    );
    assert!(
        kept.join(format!("{hash}.json")).is_file(),
        "its description is kept beside it"
    );

    let (lines, sent) = answer_with(
        home.path(),
        pinned(&"0".repeat(64)),
        "echo",
        json!({ "text": "x" }),
    )
    .await;
    assert_eq!(offered(&sent[0]), ["echo"]);
    let result = result_of(&lines);
    assert!(
        result.contains("not the one pinned") && result.contains("not run"),
        "{result}"
    );
}

#[tokio::test]
async fn an_http_skill_is_described_by_get_and_called_by_post() {
    std::env::set_var("DM_TEST_SKILL_HEADER", "fake-header-key-55");
    let described = json!({ "name": "weather", "description": "The weather in a town.",
        "input": { "type": "object", "properties": { "town": { "type": "string" } } } });
    let skill = serve(vec![
        status(200, &described.to_string()),
        status(200, r#"{"town":"Maseru","sky":"clear"}"#),
    ]);
    let home = tempfile::tempdir().unwrap();
    let tools = json!({ "weather": { "http": format!("{}/api/weather", skill.url), "headers": { "x-skill-key": "$DM_TEST_SKILL_HEADER" } } });
    let (lines, sent) =
        answer_with(home.path(), tools, "weather", json!({ "town": "Maseru" })).await;
    assert_eq!(
        sent[0].body["tools"][0]["function"]["description"],
        "The weather in a town."
    );
    assert_eq!(result_of(&lines), r#"{"sky":"clear","town":"Maseru"}"#);
    let asked = skill.requests();
    assert_eq!(asked.len(), 2);
    assert!(
        asked
            .iter()
            .all(|r| r.path == "/api/weather"
                && r.header("x-skill-key") == Some("fake-header-key-55"))
    );
    assert_eq!(asked[1].body, json!({ "town": "Maseru" }));
    assert!(!lines
        .iter()
        .any(|l| l.to_string().contains("fake-header-key-55")));
}

#[tokio::test]
async fn without_remember_listed_there_is_no_lookup() {
    let mock = serve(vec![text_reply("Hi.")]);
    let mut request = common::request("openai-completions", &mock.url, "hello");
    request.lookup = Some(dm::request::Lookup { model: None });
    request.tools = Some(serde_json::from_value(json!({})).unwrap());
    let home = tempfile::tempdir().unwrap();
    let store = dm::Store::open(home.path()).unwrap();
    dm::respond(
        &store,
        &reqwest::Client::new(),
        &request,
        &mut dm::output::Collected::default(),
    )
    .await
    .unwrap();
    let sent = mock.requests();
    assert_eq!(sent.len(), 1, "no lookup decision was asked for");
    assert!(sent[0].body.get("tools").is_none());
    assert!(!sent[0].body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("remember"));
}

/// What the binary is sent and prints answering in `home` with `args` and `env`, the model replying with `replies`.
fn by_cli(
    home: &Path,
    args: &[&str],
    env: &[(&str, &Path)],
    replies: Vec<Reply>,
) -> (Vec<Received>, String) {
    let mock = serve(replies);
    let providers = json!({ "providers": { "mock": common::provider("openai-completions", &mock.url, "fake-key") } });
    fs::write(home.join("providers.json"), providers.to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_dm"))
        .args(["-p", "hello", "--json"])
        .args(args)
        .env("DM_HOME", home)
        .envs(env.iter().copied())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    (mock.requests(), String::from_utf8(out.stdout).unwrap())
}

fn offered_by_cli(home: &Path, args: &[&str]) -> Vec<String> {
    offered(&by_cli(home, args, &[], vec![text_reply("Hi.")]).0[0])
}

#[test]
fn without_deno_a_js_skill_says_so() {
    let home = tempfile::tempdir().unwrap();
    let tools = home.path().join("tools.json");
    fs::write(
        &tools,
        json!({ "echo": { "js": module(home.path(), "echo", ECHO) } }).to_string(),
    )
    .unwrap();
    let missing = home.path().join("no-deno");
    let (_, printed) = by_cli(
        home.path(),
        &[],
        &[("DM_DENO", &missing)],
        vec![call("echo", json!({ "text": "hi" })), text_reply("Done.")],
    );
    assert!(
        printed.contains("echo failed: JavaScript skills run under Deno, and none was found"),
        "{printed}"
    );
}

#[test]
fn the_tools_json_comes_from_a_file_a_url_or_the_home() {
    let home = tempfile::tempdir().unwrap();
    let skill =
        json!({ "name": "w", "description": "W.", "input": { "type": "object" } }).to_string();
    let skills = serve((0..3).map(|_| status(200, &skill)).collect());
    let listed = |name: &str| json!({ name: { "http": format!("{}/w", skills.url) } }).to_string();

    assert_eq!(
        offered_by_cli(home.path(), &[]),
        ["remember"],
        "with no tools JSON, remember as before"
    );

    fs::write(home.path().join("tools.json"), listed("from_home")).unwrap();
    assert_eq!(offered_by_cli(home.path(), &[]), ["from_home"]);

    let file = home.path().join("other.json");
    fs::write(&file, listed("from_file")).unwrap();
    assert_eq!(
        offered_by_cli(home.path(), &["--tools", file.to_str().unwrap()]),
        ["from_file"]
    );

    let host = serve(vec![status(200, &listed("from_url"))]);
    assert_eq!(
        offered_by_cli(
            home.path(),
            &["--tools", &format!("{}/tools.json", host.url)]
        ),
        ["from_url"]
    );
}

#[tokio::test]
async fn node_style_modules_with_node_and_npm_imports_run() {
    let home = tempfile::tempdir().unwrap();
    let body = "import { basename } from \"node:path\";\nimport ms from \"npm:ms@2.1.3\";\n"
        .to_string()
        + &skill_with(
            "",
            "return { base: basename(\"/a/b.txt\"), ms: ms(\"2m\") };",
        );
    let node = module(home.path(), "node", &body);
    let (lines, _) = answer_with(
        home.path(),
        json!({ "node": { "js": node } }),
        "node",
        json!({}),
    )
    .await;
    assert_eq!(result_of(&lines), r#"{"base":"b.txt","ms":120000}"#);
}

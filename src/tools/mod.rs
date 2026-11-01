//! The tools a model is offered for one answer. Each is a `Tool`, one kind to a file: dm's own (`remember`), a
//! JavaScript module run under Deno, and an HTTP endpoint. A JavaScript or HTTP tool describes itself (`described.rs`);
//! one that cannot be set up is still offered, and calling it says why (`Unavailable`), so the answer goes on.

mod builtin;
mod described;
mod http;
mod js;

pub use js::sha256_hex;

use crate::{
    config::{
        secret::{self, Secret},
        tools::{Kind, Tools},
    },
    provider::chat::ToolSpec,
    store::Store,
    who::Who,
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::{collections::BTreeMap, future::Future, pin::Pin, time::Duration};

/// How long a tool call, a download or a description may take.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// The most a tool may answer with, or a module weigh.
pub const MAX_BYTES: usize = 1024 * 1024;

pub type Call<'a> = Pin<Box<dyn Future<Output = Result<String>> + 'a>>;

pub trait Tool {
    fn spec(&self) -> &ToolSpec;
    /// The result the model reads; an error becomes a result too (`run`).
    fn call<'a>(&'a self, input: &'a Value) -> Call<'a>;
}

/// What one answer's tools are set up with.
pub struct Setup<'a> {
    pub store: &'a Store,
    pub who: &'a Who,
    pub http: &'a reqwest::Client,
}

/// Every tool listed, but those `skip` names.
pub async fn open<'a>(
    tools: &Tools,
    skip: impl Fn(&str) -> bool,
    setup: &Setup<'a>,
) -> Vec<Box<dyn Tool + 'a>> {
    let mut opened: Vec<Box<dyn Tool + 'a>> = vec![];
    for (name, kind) in tools.0.iter().filter(|(name, _)| !skip(name)) {
        let tool = match kind {
            Kind::Builtin => builtin::open(name, setup),
            Kind::Js {
                module,
                sha256,
                env,
            } => js::Js::open(name, module, sha256.as_deref(), env, setup).await,
            Kind::Http { endpoint, headers } => {
                http::Http::open(name, endpoint, headers, setup.http).await
            }
        };
        opened.push(tool.unwrap_or_else(|err| {
            eprintln!("dm: the tool {name} cannot be used: {err:#}");
            Box::new(Unavailable::new(name, err))
        }));
    }
    opened
}

/// Calls a tool; what goes wrong is said to the model, which answers on.
pub async fn run(tool: &dyn Tool, input: &Value) -> String {
    tool.call(input)
        .await
        .unwrap_or_else(|err| format!("{} failed: {err:#}", tool.spec().name))
}

/// A tool that could not be set up: offered under its name, and each call says why.
struct Unavailable {
    spec: ToolSpec,
    why: String,
}

impl Unavailable {
    fn new(name: &str, why: anyhow::Error) -> Self {
        Self {
            spec: ToolSpec {
                name: name.to_string(),
                description: "This tool cannot be used right now; calling it says why.".into(),
                parameters: json!({ "type": "object", "properties": {} }),
            },
            why: format!("{why:#}"),
        }
    }
}

impl Tool for Unavailable {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call<'a>(&'a self, _: &'a Value) -> Call<'a> {
        Box::pin(async move { bail!("{}", self.why) })
    }
}

/// Values resolved like provider keys, kept apart from their names so they can be taken back out of anything shown.
struct Resolved {
    values: Vec<(String, Secret)>,
}

impl Resolved {
    fn of(raw: &BTreeMap<String, String>) -> Result<Self> {
        let values = raw
            .iter()
            .map(|(name, value)| {
                secret::resolve(value)
                    .map(|secret| (name.clone(), secret))
                    .map_err(|err| err.context(format!("{name} could not be resolved")))
            })
            .collect::<Result<_>>()?;
        Ok(Self { values })
    }

    /// `text` with every value taken out.
    fn redact(&self, text: &str) -> String {
        self.values
            .iter()
            .map(|(_, secret)| secret.expose())
            .filter(|value| value.len() >= 4)
            .fold(text.to_string(), |text, value| text.replace(value, "***"))
    }
}

/// A response's body, refused once it is larger than MAX_BYTES.
async fn capped(mut response: reqwest::Response) -> Result<Vec<u8>> {
    let mut body = vec![];
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_BYTES {
            bail!("the answer is larger than {} KiB", MAX_BYTES / 1024);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// A tool's answer as the text the model reads: JSON, compact.
fn json_text(bytes: &[u8], what: &str) -> Result<String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("{what} did not answer with JSON"))?;
    Ok(value.to_string())
}

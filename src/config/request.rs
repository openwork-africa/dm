//! One call to dm, as JSON: the providers, which model, who is speaking, and the message.

use crate::config::Providers;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

pub const DEFAULT_AGENT: &str = "default";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    #[serde(default)]
    pub providers: Providers,
    /// `<provider>/<model-id>`; the model marked `default` when left out.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub person: Option<String>,
    /// An IANA time zone; this machine's when left out.
    #[serde(default)]
    pub time_zone: Option<String>,
    /// The agent's own instructions.
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    /// Decide before each answer whether it needs `remember`; without it, `remember` is always offered.
    #[serde(default)]
    pub lookup: Option<Lookup>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lookup {
    /// The model that decides; the answer's model when left out.
    #[serde(default)]
    pub model: Option<String>,
}

impl Request {
    pub fn parse(json: &str) -> Result<Self> {
        serde_json::from_str(json).context("the request is not valid JSON of the right shape")
    }

    pub fn agent(&self) -> &str {
        self.agent
            .as_deref()
            .filter(|a| !a.is_empty())
            .unwrap_or(DEFAULT_AGENT)
    }
}

/// A providers file, in msg's `models.json` shape: `{ "providers": { ... } }`.
#[derive(Deserialize)]
struct ProvidersFile {
    providers: Providers,
}

pub fn read_providers(path: &Path) -> Result<Providers> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let file: ProvidersFile = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a providers file", path.display()))?;
    Ok(file.providers)
}

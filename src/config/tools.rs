//! The tools a request offers, keyed by name, one entry each: dm's own (`{"builtin": true}`), a JavaScript module run
//! under Deno (`{"js": "<url or path>", "sha256"?, "env"?}`) or an HTTP endpoint (`{"http": "<url>", "headers"?}`).
//! Values in `env` and `headers` resolve like provider keys. Without any tools JSON, `remember` alone is offered.

use crate::memory::remember;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Builtin,
    Js {
        module: String,
        /// The module's SHA-256, hex: another module is never run under this name.
        sha256: Option<String>,
        env: BTreeMap<String, String>,
    },
    Http {
        endpoint: String,
        headers: BTreeMap<String, String>,
    },
}

/// An entry as written; `description` is accepted so an entry of a skills index can be copied in as it is.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    #[serde(default)]
    builtin: bool,
    js: Option<String>,
    http: Option<String>,
    sha256: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default, rename = "description")]
    _description: Option<String>,
}

impl TryFrom<Entry> for Kind {
    type Error = anyhow::Error;

    fn try_from(entry: Entry) -> Result<Self> {
        if let Some(hash) = &entry.sha256 {
            if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!("sha256 must be 64 hex digits");
            }
        }
        match (entry.builtin, entry.js, entry.http) {
            (true, None, None)
                if entry.env.is_empty() && entry.headers.is_empty() && entry.sha256.is_none() =>
            {
                Ok(Kind::Builtin)
            }
            (false, Some(module), None) if entry.headers.is_empty() => Ok(Kind::Js {
                module,
                sha256: entry.sha256.map(|hash| hash.to_ascii_lowercase()),
                env: entry.env,
            }),
            (false, None, Some(endpoint)) if entry.env.is_empty() && entry.sha256.is_none() => {
                Ok(Kind::Http {
                    endpoint,
                    headers: entry.headers,
                })
            }
            (true, None, None) | (false, Some(_), None) | (false, None, Some(_)) => {
                bail!("env and sha256 go with js, headers with http")
            }
            _ => bail!("give one of builtin, js or http"),
        }
    }
}

/// The tools by name, in name order.
#[derive(Debug, Clone, PartialEq)]
pub struct Tools(pub BTreeMap<String, Kind>);

impl Default for Tools {
    /// What dm offers with no tools JSON: `remember`, as before there were tools.
    fn default() -> Self {
        Self(BTreeMap::from([(
            remember::NAME.to_string(),
            Kind::Builtin,
        )]))
    }
}

impl<'de> Deserialize<'de> for Tools {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries = BTreeMap::<String, serde_json::Value>::deserialize(deserializer)?;
        entries
            .into_iter()
            .map(|(name, value)| {
                let kind = serde_json::from_value::<Entry>(value)
                    .map_err(anyhow::Error::from)
                    .and_then(Kind::try_from)
                    .and_then(|kind| check_name(&name).map(|()| kind))
                    .map_err(|err| serde::de::Error::custom(format!("tool `{name}`: {err:#}")))?;
                Ok((name, kind))
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }
}

/// A name every API style takes for a tool.
fn check_name(name: &str) -> Result<()> {
    let fine = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    if name.is_empty() || name.len() > 64 || !name.chars().all(fine) {
        bail!("a tool's name is 1 to 64 letters, digits, _ or -");
    }
    Ok(())
}

impl Tools {
    pub fn parse(text: &str, from: &str) -> Result<Self> {
        serde_json::from_str(text).with_context(|| format!("{from} is not a tools file"))
    }

    /// A tools file at a path, or at an `http(s)://` address.
    pub async fn load(source: &str, http: &reqwest::Client) -> Result<Self> {
        let text = if is_url(source) {
            let response = http
                .get(source)
                .timeout(crate::tools::TIMEOUT)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .with_context(|| format!("could not fetch {source}"))?;
            response.text().await?
        } else {
            let path = crate::config::home::expand(Path::new(source));
            std::fs::read_to_string(&path)
                .with_context(|| format!("could not read {}", path.display()))?
        };
        Self::parse(&text, source)
    }

    pub fn has(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }
}

pub fn is_url(source: &str) -> bool {
    source.starts_with("https://") || source.starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: serde_json::Value) -> Result<Tools> {
        Tools::parse(&value.to_string(), "the test")
    }

    #[test]
    fn the_three_kinds_parse_by_name() {
        let tools = parse(json!({
            "remember": { "builtin": true },
            "web_search": { "js": "https://openwork.africa/skill/web_search/index.js", "description": "Search the web.",
                "sha256": "AB".repeat(32), "env": { "TAVILY_API_KEY": "!cat key" } },
            "some_api": { "http": "https://example.com/api", "headers": { "x-key": "$KEY" } }
        }))
        .unwrap();
        assert_eq!(tools.0["remember"], Kind::Builtin);
        assert_eq!(
            tools.0["web_search"],
            Kind::Js {
                module: "https://openwork.africa/skill/web_search/index.js".into(),
                sha256: Some("ab".repeat(32)),
                env: BTreeMap::from([("TAVILY_API_KEY".into(), "!cat key".into())]),
            }
        );
        assert!(
            matches!(&tools.0["some_api"], Kind::Http { endpoint, headers }
            if endpoint == "https://example.com/api" && headers["x-key"] == "$KEY")
        );
    }

    #[test]
    fn without_a_file_remember_is_offered() {
        assert_eq!(
            Tools::default().0.into_iter().collect::<Vec<_>>(),
            vec![("remember".to_string(), Kind::Builtin)]
        );
    }

    #[test]
    fn a_wrong_entry_names_the_tool() {
        for (entry, says) in [
            (
                json!({ "js": "a.js", "http": "https://x" }),
                "one of builtin",
            ),
            (json!({}), "one of builtin"),
            (
                json!({ "http": "https://x", "env": { "A": "b" } }),
                "env and sha256 go with js",
            ),
            (json!({ "js": "a.js", "sha256": "abc" }), "64 hex"),
            (json!({ "js": "a.js", "run": "x" }), "unknown field"),
        ] {
            let err = format!("{:#}", parse(json!({ "t": entry })).unwrap_err());
            assert!(err.contains("tool `t`") && err.contains(says), "{err}");
        }
        assert!(parse(json!({ "bad name": { "builtin": true } })).is_err());
    }
}

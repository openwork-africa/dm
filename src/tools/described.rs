//! What a skill says of itself: its name, what it is for, its input as a JSON schema and, for a JavaScript module, the
//! hosts it reaches and the environment variables it reads. The name in the tools JSON is the one the model is given:
//! a skill naming itself otherwise is still offered under that key, with a note.

use crate::provider::chat::ToolSpec;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Described {
    pub name: String,
    pub description: String,
    pub input: Value,
    #[serde(default)]
    pub net: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
}

impl Described {
    pub fn parse(bytes: &[u8], what: &str) -> Result<Self> {
        let described: Self = serde_json::from_slice(bytes)
            .with_context(|| format!("{what} is not {{ name, description, input }}"))?;
        described
            .check()
            .with_context(|| format!("{what} describes itself wrongly"))?;
        Ok(described)
    }

    fn check(&self) -> Result<()> {
        if self.description.trim().is_empty() {
            bail!("its description is empty");
        }
        if self.input.get("type").and_then(Value::as_str) != Some("object") {
            bail!("its input is not a JSON schema of `type: object`");
        }
        if self.input.get("properties").is_some_and(|p| !p.is_object()) {
            bail!("its input's properties are not an object");
        }
        let fine = |item: &String| !item.is_empty() && !item.contains([',', ' ', '=']);
        if !self.net.iter().all(fine) || !self.env.iter().all(fine) {
            bail!("its net and env are names, with no commas, spaces or =");
        }
        Ok(())
    }

    pub fn spec(&self, key: &str) -> ToolSpec {
        if self.name != key {
            eprintln!(
                "dm: the tool {key} calls itself {}; it is offered as {key}",
                self.name
            );
        }
        ToolSpec {
            name: key.to_string(),
            description: self.description.clone(),
            parameters: self.input.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> Result<Described> {
        Described::parse(value.to_string().as_bytes(), "the skill")
    }

    #[test]
    fn the_key_names_the_tool() {
        let described = parse(json!({ "name": "search", "description": "Finds things.",
            "input": { "type": "object", "properties": { "q": { "type": "string" } } } }))
        .unwrap();
        let spec = described.spec("web_search");
        assert_eq!(spec.name, "web_search");
        assert_eq!(spec.parameters["properties"]["q"]["type"], "string");
    }

    #[test]
    fn a_wrong_description_is_refused() {
        for wrong in [
            json!({ "name": "a", "description": "d", "input": { "type": "string" } }),
            json!({ "name": "a", "description": "", "input": { "type": "object" } }),
            json!({ "name": "a", "description": "d", "input": { "type": "object", "properties": [] } }),
            json!({ "name": "a", "description": "d", "input": { "type": "object" }, "net": ["a.com,b.com"] }),
            json!({ "name": "a", "input": { "type": "object" } }),
        ] {
            assert!(parse(wrong).is_err());
        }
    }
}

//! The providers and models a request names, in msg's `models.json` shape.

use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

/// The API style a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ApiStyle {
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages,
    #[serde(rename = "openai-completions")]
    OpenAiCompletions,
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    #[serde(rename = "bedrock")]
    Bedrock,
    #[serde(rename = "azure-foundry")]
    AzureFoundry,
}

/// US dollars per million tokens.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
    #[serde(default)]
    pub tiers: Vec<CostTier>,
}

/// The prices that apply once a call's input is above `inputTokensAbove`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostTier {
    pub input_tokens_above: u64,
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub thinking: bool,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub cost: Option<Cost>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    #[serde(default)]
    pub name: Option<String>,
    pub api: ApiStyle,
    /// Required by every style but `bedrock`, which builds it from `region` when it is left out.
    #[serde(default)]
    pub base_url: Option<String>,
    /// `$NAME`, `!command` or a literal; resolved when a call is made (secret.rs).
    #[serde(default)]
    pub api_key: Option<String>,
    /// Values may be `$NAME` or `!command` too.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Send the key as `Authorization: Bearer` whatever the style's own header is.
    #[serde(default)]
    pub auth_header: bool,
    /// `bedrock`: the AWS region.
    #[serde(default)]
    pub region: Option<String>,
    /// `azure-foundry`: the `api-version` query parameter.
    #[serde(default)]
    pub api_version: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
}

impl Provider {
    pub fn base_url(&self) -> Result<&str> {
        self.base_url
            .as_deref()
            .map(|url| url.trim_end_matches('/'))
            .ok_or_else(|| anyhow!("the provider has no baseUrl"))
    }
}

/// The providers, by id.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(transparent)]
pub struct Providers(pub BTreeMap<String, Provider>);

/// One model on one provider, ready to be called.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub provider_id: String,
    pub provider: Provider,
    pub model: ModelEntry,
}

impl Target {
    /// `<provider>/<model-id>`.
    pub fn label(&self) -> String {
        format!("{}/{}", self.provider_id, self.model.id)
    }
}

impl Providers {
    /// The model `<provider>/<model-id>` names, or the one marked `default` when none is named. A model id the provider
    /// does not list is used as given, with no settings of its own.
    pub fn target(&self, reference: Option<&str>) -> Result<Target> {
        let Some(reference) = reference.filter(|r| !r.is_empty()) else {
            return self.default_target();
        };
        let (provider_id, model_id) = reference
            .split_once('/')
            .ok_or_else(|| anyhow!("model `{reference}` must be <provider>/<model-id>"))?;
        let provider = self
            .0
            .get(provider_id)
            .ok_or_else(|| anyhow!("no provider `{provider_id}`"))?;
        let model = provider
            .models
            .iter()
            .find(|m| m.id == model_id)
            .cloned()
            .unwrap_or_else(|| ModelEntry {
                id: model_id.to_string(),
                ..ModelEntry::default()
            });
        Ok(Target {
            provider_id: provider_id.to_string(),
            provider: provider.clone(),
            model,
        })
    }

    fn default_target(&self) -> Result<Target> {
        for (id, provider) in &self.0 {
            if let Some(model) = provider.models.iter().find(|m| m.default) {
                return Ok(Target {
                    provider_id: id.clone(),
                    provider: provider.clone(),
                    model: model.clone(),
                });
            }
        }
        bail!("no model was named and none is marked default")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn providers() -> Providers {
        serde_json::from_value(serde_json::json!({
            "openai": { "api": "openai-completions", "baseUrl": "https://api.openai.com/v1/", "models": [{ "id": "gpt-5" }] },
            "anthropic": { "api": "anthropic-messages", "baseUrl": "https://api.anthropic.com",
                "models": [{ "id": "claude-opus-5-5", "default": true, "cost": { "input": 4, "output": 20 } }] }
        }))
        .unwrap()
    }

    #[test]
    fn the_default_model_is_the_marked_one() {
        assert_eq!(
            providers().target(None).unwrap().label(),
            "anthropic/claude-opus-5-5"
        );
    }

    #[test]
    fn a_named_model_keeps_slashes_after_the_provider() {
        let target = providers().target(Some("openai/org/gpt-x")).unwrap();
        assert_eq!(target.model.id, "org/gpt-x");
        assert_eq!(
            target.provider.base_url().unwrap(),
            "https://api.openai.com/v1"
        );
    }

    #[test]
    fn an_unknown_provider_is_an_error() {
        assert!(providers().target(Some("nope/x")).is_err());
        assert!(providers().target(Some("no-slash")).is_err());
    }
}

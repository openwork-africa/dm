//! Azure AI Foundry: OpenAI chat completions on an Azure endpoint, with the key in `api-key` and an `api-version` query
//! parameter. `baseUrl` is the endpoint up to `/chat/completions`, such as
//! `https://<resource>.services.ai.azure.com/models` or `https://<resource>.openai.azure.com/openai/deployments/<name>`.

use super::{
    chat::Chat,
    openai_completions::{body, ChunkReader},
    Api, KeyHeader, StreamReader,
};
use crate::config::{ModelEntry, Provider};
use anyhow::Result;
use serde_json::Value;

const API_VERSION: &str = "2024-05-01-preview";

pub struct AzureFoundry;

impl Api for AzureFoundry {
    fn url(&self, provider: &Provider, _: &ModelEntry) -> Result<String> {
        let version = provider.api_version.as_deref().unwrap_or(API_VERSION);
        Ok(format!(
            "{}/chat/completions?api-version={version}",
            provider.base_url()?
        ))
    }

    fn key_header(&self) -> KeyHeader {
        KeyHeader::Named("api-key")
    }

    fn body(&self, model: &ModelEntry, chat: &Chat) -> Value {
        body(model, chat)
    }

    fn reader(&self) -> Box<dyn StreamReader> {
        Box::new(ChunkReader)
    }
}

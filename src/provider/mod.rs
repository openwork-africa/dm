//! Calling a model. Each API style is one `Api`: where to send a call, how to sign it, the body, and how to read the
//! stream back into events. `Model` does the rest the same way for all of them: resolving the key and headers, sending,
//! and building the turn from the events.

mod anthropic;
mod azure_foundry;
mod bedrock;
pub mod chat;
pub mod framing;
mod openai_completions;
mod openai_responses;

use crate::{
    config::{ApiStyle, Cost, ModelEntry, Provider, Target},
    secret,
};
use anyhow::{anyhow, bail, Result};
use chat::{Chat, Event, Turn, TurnBuilder};
use framing::{Frame, Framing};
use futures_util::StreamExt;
use serde_json::Value;

/// How a style sends the key.
pub enum KeyHeader {
    Bearer,
    Named(&'static str),
}

pub trait Api: Send + Sync {
    fn url(&self, provider: &Provider, model: &ModelEntry) -> Result<String>;
    fn key_header(&self) -> KeyHeader {
        KeyHeader::Bearer
    }
    /// Headers every call of this style carries.
    fn fixed_headers(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }
    fn body(&self, model: &ModelEntry, chat: &Chat) -> Value;
    fn framing(&self) -> Framing {
        Framing::Sse
    }
    /// A reader for one call's stream.
    fn reader(&self) -> Box<dyn StreamReader>;
}

pub trait StreamReader: Send {
    fn read(&mut self, frame: &Frame) -> Result<Vec<Event>>;
}

fn api_for(style: ApiStyle) -> Box<dyn Api> {
    match style {
        ApiStyle::AnthropicMessages => Box::new(anthropic::AnthropicMessages),
        ApiStyle::OpenAiCompletions => Box::new(openai_completions::OpenAiCompletions),
        ApiStyle::OpenAiResponses => Box::new(openai_responses::OpenAiResponses),
        ApiStyle::Bedrock => Box::new(bedrock::Bedrock),
        ApiStyle::AzureFoundry => Box::new(azure_foundry::AzureFoundry),
    }
}

/// What a host's error says, cut short.
const ERROR_CHARS: usize = 600;

pub struct Model {
    target: Target,
    api: Box<dyn Api>,
    http: reqwest::Client,
}

impl Model {
    pub fn new(http: reqwest::Client, target: Target) -> Self {
        let api = api_for(target.provider.api);
        Self { target, api, http }
    }

    pub fn label(&self) -> String {
        self.target.label()
    }

    pub fn cost(&self) -> Option<&Cost> {
        self.target.model.cost.as_ref()
    }

    /// One call, streamed: each piece of text goes to `on_text` as it arrives.
    pub async fn ask(&self, chat: &Chat, on_text: &mut dyn FnMut(&str)) -> Result<Turn> {
        let (provider, model) = (&self.target.provider, &self.target.model);
        let url = self.api.url(provider, model)?;
        let mut request = self.http.post(&url).json(&self.api.body(model, chat));
        for (name, value) in self.api.fixed_headers() {
            request = request.header(*name, *value);
        }
        if let Some(raw) = &provider.api_key {
            let key = secret::resolve(raw)?;
            request = match (provider.auth_header, self.api.key_header()) {
                (false, KeyHeader::Named(name)) => request.header(name, key.expose()),
                _ => request.bearer_auth(key.expose()),
            };
        }
        for (name, raw) in provider.headers.iter().chain(&model.headers) {
            request = request.header(name, secret::resolve(raw)?.expose());
        }
        let response = request.send().await.map_err(|err| {
            anyhow!(
                "{} could not be reached: {}",
                self.label(),
                err.without_url()
            )
        })?;
        if !response.status().is_success() {
            let status = response.status();
            let detail: String = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(ERROR_CHARS)
                .collect();
            bail!("{} answered {status}: {detail}", self.label());
        }

        let mut decoder = self.api.framing().decoder();
        let mut reader = self.api.reader();
        let mut builder = TurnBuilder::default();
        let mut body = response.bytes_stream();
        let mut apply = |frames: Vec<Frame>, builder: &mut TurnBuilder| -> Result<()> {
            for frame in frames {
                for event in reader.read(&frame)? {
                    builder.apply(event, on_text);
                }
            }
            Ok(())
        };
        while let Some(bytes) = body.next().await {
            let bytes = bytes.map_err(|err| {
                anyhow!("{}'s answer broke off: {}", self.label(), err.without_url())
            })?;
            apply(decoder.push(&bytes)?, &mut builder)?;
        }
        apply(decoder.finish()?, &mut builder)?;
        Ok(builder.finish())
    }
}

/// A frame's data as JSON; a frame that is not JSON (a keep-alive, `[DONE]`) is `None`.
fn json_of(frame: &Frame) -> Option<Value> {
    serde_json::from_str(&frame.data).ok()
}

fn count(value: &Value) -> u64 {
    value.as_u64().unwrap_or(0)
}

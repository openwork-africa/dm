//! A skill that is an HTTP endpoint: `GET` it for `{ name, description, input }` (once an answer), `POST` the input
//! JSON to call it, and its JSON answer is the result. Header values resolve like provider keys.

use super::{capped, described::Described, json_text, Call, Resolved, Tool, TIMEOUT};
use crate::provider::chat::ToolSpec;
use anyhow::{anyhow, bail, Context, Result};
use reqwest::{Client, Method, RequestBuilder};
use serde_json::Value;
use std::collections::BTreeMap;

pub struct Http {
    spec: ToolSpec,
    endpoint: Endpoint,
}

impl Http {
    pub async fn open(
        name: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
        http: &Client,
    ) -> Result<Box<dyn Tool>> {
        let endpoint = Endpoint {
            url: url.to_string(),
            headers: Resolved::of(headers)?,
            http: http.clone(),
        };
        let described = endpoint
            .send(Method::GET, |request| request)
            .await
            .with_context(|| format!("{url} could not be asked what it is"))?;
        Ok(Box::new(Self {
            spec: Described::parse(&described, url)?.spec(name),
            endpoint,
        }))
    }
}

impl Tool for Http {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call<'a>(&'a self, input: &'a Value) -> Call<'a> {
        Box::pin(async move {
            let bytes = self
                .endpoint
                .send(Method::POST, |request| request.json(input))
                .await?;
            Ok(self
                .endpoint
                .headers
                .redact(&json_text(&bytes, &self.spec.name)?))
        })
    }
}

struct Endpoint {
    url: String,
    headers: Resolved,
    http: Client,
}

impl Endpoint {
    async fn send(
        &self,
        method: Method,
        body: impl FnOnce(RequestBuilder) -> RequestBuilder,
    ) -> Result<Vec<u8>> {
        let mut request = self.http.request(method, &self.url).timeout(TIMEOUT);
        for (name, value) in &self.headers.values {
            request = request.header(name, value.expose());
        }
        let response = body(request)
            .send()
            .await
            .map_err(|err| match err.is_timeout() {
                true => anyhow!("it took longer than {} seconds", TIMEOUT.as_secs()),
                false => anyhow!("{}", self.headers.redact(&err.without_url().to_string())),
            })?;
        let status = response.status();
        let bytes = capped(response).await?;
        if !status.is_success() {
            let said: String = String::from_utf8_lossy(&bytes).chars().take(300).collect();
            bail!("it answered {status}: {}", self.headers.redact(said.trim()));
        }
        Ok(bytes)
    }
}

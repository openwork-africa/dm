//! dm's own tools, run in this process: `remember`, its one builtin.

use super::{Call, Setup, Tool};
use crate::{
    memory::{links, remember},
    provider::chat::ToolSpec,
    store::Store,
    who::Who,
};
use anyhow::{bail, Result};
use serde_json::Value;

pub fn open<'a>(name: &str, setup: &Setup<'a>) -> Result<Box<dyn Tool + 'a>> {
    if name != remember::NAME {
        bail!(
            "dm has no builtin tool {name}; its one builtin is {}",
            remember::NAME
        );
    }
    setup.store.load_memory(setup.who)?;
    Ok(Box::new(Remember {
        spec: remember::tool(),
        store: setup.store,
        who: setup.who,
    }))
}

struct Remember<'a> {
    spec: ToolSpec,
    store: &'a Store,
    who: &'a Who,
}

impl Tool for Remember<'_> {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call<'a>(&'a self, input: &'a Value) -> Call<'a> {
        let query = input["query"].as_str().unwrap_or_default();
        let urls = links::links_in(&input["url"]);
        Box::pin(async move {
            if urls.is_empty() {
                self.store.recall(self.who, query)
            } else {
                links::recall(&urls, query, links::Limits::default()).await
            }
        })
    }
}

//! Answering one message, start to end: the day, the history, the lookup, the answer streamed out, and what it used.
//! Compaction and summaries are only queued here (jobs.rs); they run once the answer is out.

mod lookup;
pub mod output;
mod prompt;
mod rounds;

use crate::{
    config::request::Request,
    day,
    memory::remember,
    provider::{
        chat::{Chat, Message, Reply, Usage},
        Model,
    },
    store::{
        messages::{StoredMessage, ASSISTANT, USER},
        Store,
    },
    tools::{self, Setup},
    who::Who,
};
use anyhow::{anyhow, Result};
use output::{Line, Sink};
use prompt::Prompt;
use rounds::answer;

#[derive(Debug, Clone, PartialEq)]
pub struct Answered {
    pub reply: String,
    pub model: String,
    pub usage: Usage,
    pub cost_usd: f64,
}

/// Counts each call in the store and adds up what the answer used and cost.
struct Ledger<'a> {
    store: &'a Store,
    today: &'a str,
    who: &'a Who,
    total: Usage,
    cost: f64,
}

impl Ledger<'_> {
    fn count(&mut self, purpose: &str, model: &Model, usage: Usage) -> Result<()> {
        self.total.add(usage);
        self.cost += self
            .store
            .count_call(self.today, self.who, purpose, model, &usage)?;
        Ok(())
    }
}

/// The model that answers, and the one that decides on a lookup when the request asks for one and `remember` is a tool.
fn models(request: &Request, http: &reqwest::Client) -> Result<(Model, Option<Model>)> {
    let model = Model::new(
        http.clone(),
        request.providers.target(request.model.as_deref())?,
    )?;
    let decider = match request
        .lookup
        .as_ref()
        .filter(|_| request.tools().has(remember::NAME))
    {
        Some(lookup) => Some(Model::new(
            http.clone(),
            request
                .providers
                .target(lookup.model.as_deref().or(request.model.as_deref()))?,
        )?),
        None => None,
    };
    Ok((model, decider))
}

pub async fn respond(
    store: &Store,
    http: &reqwest::Client,
    request: &Request,
    out: &mut dyn Sink,
) -> Result<Answered> {
    let message = request
        .message
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| anyhow!("there is no message to answer"))?;
    let (model, decider) = models(request, http)?;
    let who = Who::new(request.agent(), request.person.as_deref())?;
    let zone = request
        .time_zone
        .as_deref()
        .map_or_else(|| day::zone(&day::system_zone()), day::zone)?;
    let day::Moment {
        day: today,
        display: now,
    } = day::Moment::new(zone);

    if let Some(yesterday) = store.move_to_day(&who, &today)? {
        store.close_day(&who, &yesterday, &model.label())?;
    }
    store.add_message(&who, &today, USER, message)?;
    let history = store.history(&who, &today)?;
    let last_reply = history
        .iter()
        .rev()
        .find(|m| m.role == ASSISTANT)
        .map(|m| m.content.clone());

    let mut ledger = Ledger {
        store,
        today: &today,
        who: &who,
        total: Usage::default(),
        cost: 0.0,
    };
    let look_up = match &decider {
        Some(decider) => {
            let decision = lookup::decide(decider, message, last_reply.as_deref(), &now).await;
            if let Some(usage) = decision.usage {
                ledger.count("lookup", decider, usage)?;
            }
            Some(decision.needed)
        }
        None => None,
    };
    let listed = request.tools();
    let offer_remember = listed.has(remember::NAME) && look_up != Some(false);
    let notes = store.notes(&who)?;
    let setup = Setup {
        store,
        who: &who,
        http,
    };
    let tools = tools::open(
        &listed,
        |name| name == remember::NAME && !offer_remember,
        &setup,
    )
    .await;
    let system = Prompt {
        notes: &notes,
        now: &now,
        system: request.system.as_deref(),
        remember: offer_remember,
        others: tools.len() > usize::from(offer_remember),
        look_up: look_up == Some(true),
    }
    .render();

    let answer = answer(
        &model,
        Chat {
            system,
            messages: messages_of(history),
            tools: tools.iter().map(|t| t.spec().clone()).collect(),
        },
        &tools,
        out,
    )
    .await?;
    for usage in &answer.rounds {
        ledger.count("chat", &model, *usage)?;
    }
    if !answer.reply.trim().is_empty() {
        store.add_message(&who, &today, ASSISTANT, &answer.reply)?;
        store.after_message(&who, &today, &model.label())?;
    }
    let answered = Answered {
        reply: answer.reply,
        model: model.label(),
        usage: ledger.total,
        cost_usd: ledger.cost,
    };
    out.line(&Line::Done {
        reply: &answered.reply,
        model: &answered.model,
        usage: answered.usage,
        cost_usd: answered.cost_usd,
    });
    Ok(answered)
}

/// The history as the model is sent it. A failed or empty answer leaves two user rows in a row, and some hosts reject
/// roles that do not alternate, so rows of one role are joined.
fn messages_of(history: Vec<StoredMessage>) -> Vec<Message> {
    let mut messages: Vec<Message> = vec![];
    for m in history {
        match (m.role == USER, messages.last_mut()) {
            (true, Some(Message::User(said))) => {
                said.push_str("\n\n");
                said.push_str(&m.content);
            }
            (false, Some(Message::Assistant(reply))) => {
                reply.text.push_str("\n\n");
                reply.text.push_str(&m.content);
            }
            (true, _) => messages.push(Message::User(m.content)),
            (false, _) => messages.push(Message::Assistant(Reply {
                text: m.content,
                ..Reply::default()
            })),
        }
    }
    messages
}

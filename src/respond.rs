//! Answering one message, start to end: the day, the history, the lookup, the answer streamed out, and what it used.
//! Compaction and summaries are only queued here (jobs.rs); they run once the answer is out.

use crate::{
    answer::{answer, Tool},
    day, lookup,
    messages::{StoredMessage, ASSISTANT, USER},
    output::{Line, Sink},
    prompt::Prompt,
    provider::{
        chat::{Chat, Message, Reply, Usage},
        Model,
    },
    remember,
    request::Request,
    store::Store,
    usage::cost_usd,
    who::Who,
};
use anyhow::{anyhow, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct Answered {
    pub reply: String,
    pub model: String,
    pub usage: Usage,
    pub cost_usd: f64,
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
    let model = Model::new(
        http.clone(),
        request.providers.target(request.model.as_deref())?,
    );
    let decider = match &request.lookup {
        Some(lookup) => Some(Model::new(
            http.clone(),
            request
                .providers
                .target(lookup.model.as_deref().or(request.model.as_deref()))?,
        )),
        None => None,
    };
    let who = Who::new(request.agent(), request.person.as_deref())?;
    let zone = day::zone(&request.time_zone.clone().unwrap_or_else(day::system_zone))?;
    let (today, now) = (day::today(zone), day::now(zone));

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
    store.load_memory(&who)?;

    let mut total = Usage::default();
    let mut cost = 0.0;
    let mut count = |purpose: &str, model: &Model, usage: Usage| -> Result<()> {
        let usd = cost_usd(model.cost(), &usage);
        total.add(usage);
        cost += usd;
        store.count_usage(&today, &who, purpose, &model.label(), &usage, usd)
    };

    let look_up = match &decider {
        Some(decider) => {
            let decision = lookup::decide(decider, message, last_reply.as_deref(), &now).await;
            if let Some(usage) = decision.usage {
                count("lookup", decider, usage)?;
            }
            Some(decision.needed)
        }
        None => None,
    };
    let offered = look_up.unwrap_or(true);
    let notes = store.notes(&who)?;
    let system = Prompt {
        notes: &notes,
        now: &now,
        system: request.system.as_deref(),
        tool_offered: offered,
        look_up: look_up == Some(true),
    }
    .render();
    let messages = messages_of(history);
    let tools = if offered {
        vec![Tool {
            spec: remember::tool(),
            run: Box::new(|args| {
                store
                    .recall(&who, args["query"].as_str().unwrap_or_default())
                    .unwrap_or_else(|err| format!("remember failed: {err}"))
            }),
        }]
    } else {
        vec![]
    };

    let answer = answer(
        &model,
        Chat {
            system,
            messages,
            tools: vec![],
        },
        &tools,
        out,
    )
    .await?;
    for usage in &answer.rounds {
        count("chat", &model, *usage)?;
    }
    if !answer.reply.trim().is_empty() {
        store.add_message(&who, &today, ASSISTANT, &answer.reply)?;
        store.after_message(&who, &today, &model.label())?;
    }
    let answered = Answered {
        reply: answer.reply,
        model: model.label(),
        usage: total,
        cost_usd: cost,
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

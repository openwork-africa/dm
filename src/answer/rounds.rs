//! The answer loop: the conversation goes to the model, its text streams out as it is written, and when it calls a
//! tool the tool is run and the model asked again, until it answers without one.

use crate::{
    answer::output::{Line, Sink},
    provider::{
        chat::{Chat, Message, ToolOutput, Usage},
        Model,
    },
    tools::{self, Tool},
};
use anyhow::{bail, Result};

/// Calls before dm stops asking: room for a couple of lookups and the answer.
const MAX_ROUNDS: usize = 4;

pub struct Answer {
    /// Everything the model said, across its calls.
    pub reply: String,
    /// What each call used.
    pub rounds: Vec<Usage>,
}

pub async fn answer(
    model: &Model,
    mut chat: Chat,
    tools: &[Box<dyn Tool + '_>],
    out: &mut dyn Sink,
) -> Result<Answer> {
    let mut answer = Answer {
        reply: String::new(),
        rounds: vec![],
    };
    for _ in 0..MAX_ROUNDS {
        let turn = model
            .ask(&chat, &mut |delta| {
                out.line(&Line::Text { delta });
            })
            .await?;
        answer.rounds.push(turn.usage);
        answer.reply.push_str(&turn.reply.text);
        if turn.reply.calls.is_empty() {
            return Ok(answer);
        }
        let mut results = vec![];
        for call in &turn.reply.calls {
            let input = call.input();
            out.line(&Line::ToolCall {
                id: &call.id,
                name: &call.name,
                arguments: input.clone(),
            });
            let content = match tools.iter().find(|t| t.spec().name == call.name) {
                Some(tool) => tools::run(tool.as_ref(), &input).await,
                None => format!("There is no tool called {}.", call.name),
            };
            out.line(&Line::ToolResult {
                id: &call.id,
                name: &call.name,
                content: &content,
            });
            results.push(ToolOutput {
                id: call.id.clone(),
                content,
            });
        }
        chat.messages.push(Message::Assistant(turn.reply));
        chat.messages.push(Message::ToolResults(results));
    }
    if answer.reply.trim().is_empty() {
        bail!("{} kept calling tools and never answered", model.label());
    }
    Ok(answer)
}

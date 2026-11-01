//! The lookup decision, made before the answer by any configured model: does answering this message need something
//! about the person, or something said before? With a yes the answer is offered `remember` and told to use it; with a
//! no it gets no tool. A failed decision is a no.

use crate::provider::{
    chat::{Chat, Message, Usage},
    Model,
};

const INSTRUCTIONS: &str = "You decide whether answering a direct message needs a lookup in what is kept about the person who sent it.\n\nAnswer yes when it asks about them or their life (their name, work, the people in it, what they like, their plans or dates) or about what they said or asked to be remembered before.\nAnswer no for small talk, something they are sharing or feeling now, a request to do something, or a question about the world.\n\nReply with one word: yes or no.";

pub struct Decision {
    pub needed: bool,
    pub usage: Option<Usage>,
}

/// With the agent's last reply for context ("and hers?" follows on from it) and their date and time.
pub async fn decide(model: &Model, message: &str, last_reply: Option<&str>, now: &str) -> Decision {
    let said_before = last_reply
        .map(|r| format!("<agent_said_before>\n{r}\n</agent_said_before>\n\n"))
        .unwrap_or_default();
    let chat = Chat {
        system: INSTRUCTIONS.to_string(),
        messages: vec![Message::User(format!(
            "<now>{now}</now>\n\n{said_before}<message>\n{message}\n</message>"
        ))],
        tools: vec![],
    };
    match model.ask(&chat, &mut |_| {}).await {
        Ok(turn) => Decision {
            needed: turn.reply.text.trim().to_lowercase().starts_with("yes"),
            usage: Some(turn.usage),
        },
        Err(err) => {
            eprintln!("dm: the lookup decision failed, so no lookup: {err:#}");
            Decision {
                needed: false,
                usage: None,
            }
        }
    }
}

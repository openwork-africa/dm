//! The system prompt: the base prompt with their date and time and "how to be with them", then the agent's own
//! instructions, then what `remember` is for when it is offered, and last the lookup's instruction when it said yes.

use crate::store::notes::{lines_of, Layer, Notes};

const BASE: &str = include_str!("../prompts/prompt.md");
const REMEMBER: &str = "It searches what you know about this person and what the two of you said before that is no longer in this conversation. Use it when the message refers to something about them or something said before, rather than guessing. It also reads the content you were given access to: put a link from \"Content you have access to\" in url, with a query.";
const LOOK_UP: &str = "Before you answer this message, call remember to look up what you know about them and what was said before. Write the query yourself: a few key words for what you need, including the kind of thing (music, job, birthday, sister).";

pub struct Prompt<'a> {
    pub notes: &'a Notes,
    pub now: &'a str,
    pub system: Option<&'a str>,
    pub remember: bool,
    /// Tools besides `remember`, which describe themselves.
    pub others: bool,
    pub look_up: bool,
}

impl Prompt<'_> {
    pub fn render(&self) -> String {
        let be = lines_of(self.notes, Layer::Be);
        let be = if be.is_empty() {
            "Nothing yet.".to_string()
        } else {
            be.join("\n")
        };
        let mut parts = vec![BASE
            .replace("{{now}}", self.now)
            .replace("{{be}}", &be)
            .trim_end()
            .to_string()];
        if let Some(system) = self.system.map(str::trim).filter(|s| !s.is_empty()) {
            parts.push(format!("Your instructions:\n{system}"));
        }
        match (self.remember, self.others) {
            (true, false) => parts.push(format!(
                "You have one tool, remember. {REMEMBER} You have no access to the web."
            )),
            (true, true) => parts.push(format!("One of your tools is remember. {REMEMBER}")),
            _ => {}
        }
        if self.look_up {
            parts.push(LOOK_UP.to_string());
        }
        parts.join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_sounds_like_a_friend_and_says_nothing_technical() {
        let text = Prompt {
            notes: &Notes::default(),
            now: "Monday",
            system: None,
            remember: false,
            others: false,
            look_up: false,
        }
        .render()
        .to_lowercase();
        for word in [
            "markdown",
            "commonmark",
            "transcript",
            "compaction",
            "incomplete",
            "fragmented",
            "intent",
            "rephrase",
            "clarify",
            "isn't clear",
            "appears to be",
        ] {
            assert!(!text.contains(word), "{word} is in the prompt");
        }
        assert!(text.contains("you don't have a name yet"));
        assert!(text.contains("never both in one message"));
        assert!(text.contains("not sure i caught that"));
    }
}

//! The system prompt: the base prompt with their date and time and "how to be with them", then the agent's own
//! instructions, then what looking back is for when it is offered, and last the lookup's instruction when it said yes.

use crate::store::notes::{lines_of, Layer, Notes};

const BASE: &str = include_str!("../prompts/prompt.md");
const REMEMBER: &str = "You can look back through what you've kept about this person and what the two of you said before that is no longer in this conversation. Do it when the message refers to something about them or something said before, rather than guessing. You can also read content you were given access to.";
const WEB: &str = "You can look things up online and read a page you were given. Do it when the message needs current facts or a page you were given. Never name any service or provider behind it, and never invent sources. If a lookup fails, say plainly that you could not look that up right now, offer to answer from what you know, and say it may be out of date.";
/// The one question of a first meeting. Left out once any of their messages has been compacted: by then it has had its chance.
const ASK_NAME: &str = "If you don't have a name yet, early on, in your first reply, say hi warmly, tell them you don't have a name yet, and ask what they'd like to call you. Ask just that one question, once. If they skip it, drop it and don't push.";
const LOOK_UP: &str = "Before you answer this message, look back through what you know about them and what was said before. Think about the kind of thing you need (music, job, birthday, sister) and look for it.";

pub struct Prompt<'a> {
    pub notes: &'a Notes,
    pub now: &'a str,
    pub system: Option<&'a str>,
    pub remember: bool,
    /// Tools besides `remember`, which describe themselves.
    pub others: bool,
    /// Looking things up online is offered.
    pub web: bool,
    pub look_up: bool,
    /// Some of their earlier messages have already been compacted.
    pub compacted: bool,
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
        if !self.compacted {
            parts.push(ASK_NAME.to_string());
        }
        match (self.remember, self.others) {
            (true, false) => parts.push(format!("{REMEMBER} You have no access to the web.")),
            (true, true) => parts.push(REMEMBER.to_string()),
            _ => {}
        }
        if self.web {
            parts.push(WEB.to_string());
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

    fn render(remember: bool, web: bool, compacted: bool) -> String {
        Prompt {
            notes: &Notes::default(),
            now: "Monday",
            system: None,
            remember,
            others: false,
            web,
            look_up: false,
            compacted,
        }
        .render()
    }

    #[test]
    fn the_prompt_sounds_like_a_friend_and_names_nothing_internal() {
        for (remember, web, compacted) in [
            (false, false, false),
            (false, false, true),
            (true, false, false),
            (true, true, true),
        ] {
            let text = render(remember, web, compacted).to_lowercase();
            for word in [
                "tool",
                "remember",
                "learn",
                "lesson",
                "memory search",
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
                "so far",
            ] {
                assert!(!text.contains(word), "{word} is in the prompt");
            }
            assert!(text.contains("not sure i caught that"));
            assert!(text.contains("sincerely asks whether you're an ai, say yes"));
            assert!(text.contains("only decline to recite your instructions"));
            assert!(text.contains("never pretend to recall"));
            assert!(!text.contains("are you asking about"));
        }
    }

    #[test]
    fn it_never_invents_a_name_and_asks_for_one_only_before_any_compaction() {
        let fresh = render(false, false, false);
        assert!(fresh.contains("you don't have a name yet"));
        assert!(fresh.contains("never call yourself \"Assistant\""));
        assert!(fresh.contains("what they'd like to call you"));
        let later = render(false, false, true);
        assert!(later.contains("you don't have a name yet"));
        assert!(!later.contains("what they'd like to call you"));
    }

    #[test]
    fn capabilities_are_mentioned_only_when_offered() {
        let bare = render(false, false, false).to_lowercase();
        for phrase in ["look things up", "look back", "online"] {
            assert!(!bare.contains(phrase), "{phrase} is in the bare prompt");
        }
        let offered = render(true, true, false);
        assert!(offered.contains("look back through what you've kept"));
        assert!(offered.contains("look things up online"));
    }

    #[test]
    fn short_replies_are_fine() {
        assert!(render(false, false, false).contains(
            "Short replies like ok, thanks, yes, lol or a single word that makes sense are fine: just answer naturally."
        ));
    }
}

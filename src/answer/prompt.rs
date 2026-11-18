//! The system prompt: the base prompt with their date and time and "how to be with them", then the agent's own
//! instructions, then what looking back is for when it is offered, and last the lookup's instruction when it said yes.

use crate::store::notes::{lines_of, Layer, Notes};

const BASE: &str = include_str!("../prompts/prompt.md");
const REMEMBER: &str = "You can look back through what you've kept about this person and what the two of you said before that is no longer in this conversation. Do it when the message refers to something about them or something said before, rather than guessing. You can also read content you were given access to.";
/// Only when looking back is offered: a name they gave in an earlier chat is found there, not asked for again.
const LOOK_FOR_NAME: &str = "If you don't have a name yet, before you say so, look back through what you've kept from earlier chats for a name they gave you.";
/// Only when something can be done for them: lasting preferences are kept for later chats.
const KEEP_IN_MIND: &str = "When you notice something lasting, like a preference, a correction or the way they like things done, keep it in mind for later chats.";
const WEB: &str = "You can look things up online and read a page you were given. Do it when the message needs current facts or a page you were given. Never name where the answers come from, and never invent sources. If a lookup fails, say plainly that you could not look that up right now, offer to answer from what you know, and say it may be out of date.";
/// The get-to-know-you questions of a first meeting. Left out once any of their messages has been compacted: by then they have had their chance.
const GET_TO_KNOW: &str = "Early on, while you don't know their name from this chat, get to know them. Say hi warmly in your first reply. Ask one detail at a time: each reply asks exactly one question, only the single next one and nothing more, even if several are still open. Never two questions in one reply, including questions joined with \"and\", \"also\" or \"by the way\", and never a list of questions. Go in this order: first their name and what they'd like to be called, then one or two simple things about their life, like what they do or what they like. If you already know their name, start with the life questions. Never make up a fact about them. Once they answer, use it and move on. Ask each question at most once in this chat: if the answer isn't in the chat, don't ask it again. If they ignore or skip a question, drop it and don't push.";
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
            parts.push(GET_TO_KNOW.to_string());
        }
        match (self.remember, self.others) {
            (true, false) => parts.push(format!(
                "{REMEMBER} {LOOK_FOR_NAME} You have no access to the web."
            )),
            (true, true) => parts.push(format!("{REMEMBER} {LOOK_FOR_NAME}")),
            _ => {}
        }
        if self.remember {
            parts.push(KEEP_IN_MIND.to_string());
        }
        if self.web {
            parts.push(WEB.to_string());
        }
        if self.look_up && self.remember {
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
                "reason",
                "structured",
                "service",
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
        assert!(fresh.contains("Ask one detail at a time: each reply asks exactly one question"));
        assert!(fresh.contains("never a list of questions"));
        assert!(fresh.contains("at most once in this chat"));
        assert!(fresh.contains("If they ignore or skip a question, drop it and don't push"));
        assert!(fresh.contains("get to know them"));
        assert!(!fresh.contains("what they'd like to call you"));
        assert!(!fresh.contains("tell them you don't have a name yet"));
        let later = render(false, false, true);
        assert!(later.contains("you don't have a name yet"));
        assert!(!later.contains("get to know them"));
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

    #[test]
    fn it_looks_back_for_a_given_name_only_when_looking_back_is_offered() {
        assert!(render(true, false, false).contains("for a name they gave you"));
        assert!(render(true, true, true).contains("for a name they gave you"));
        assert!(!render(false, false, false).contains("for a name they gave you"));
    }

    #[test]
    fn the_one_question_rule_is_part_of_how_it_talks_on_every_reply() {
        for (remember, web, compacted) in [
            (false, false, false),
            (false, false, true),
            (true, true, true),
        ] {
            assert!(
                render(remember, web, compacted).contains("ask exactly one question in that reply")
            );
        }
    }

    #[test]
    fn the_bare_prompt_promises_no_recall_and_keeps_nothing_for_later() {
        let bare = render(false, false, false);
        assert!(!bare.contains("earlier conversations"));
        assert!(bare.contains("How to be with them:"));
        assert!(!bare.contains("keep it in mind for later chats"));
        assert!(render(true, false, false).contains("keep it in mind for later chats"));
        assert!(!render(false, true, false).contains("keep it in mind for later chats"));
    }

    #[test]
    fn a_yes_to_looking_back_without_the_means_adds_no_instruction() {
        let ask = |remember| {
            Prompt {
                notes: &Notes::default(),
                now: "Monday",
                system: None,
                remember,
                others: false,
                web: false,
                look_up: true,
                compacted: true,
            }
            .render()
        };
        assert!(ask(true).contains("Before you answer this message, look back"));
        assert!(!ask(false).contains("Before you answer this message, look back"));
    }
}

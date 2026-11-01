//! Reasoning some models write into their text inside `<think>` … `</think>`. It is not the reply: the filter keeps it
//! out of the text as it streams, tags split across pieces included. A `</think>` with no opening tag means the text
//! before it was reasoning: it is taken out of the reply, though what of it was streamed already cannot be unsaid.

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

#[derive(Default)]
pub struct ThinkFilter {
    thinking: bool,
    held: String,
    /// The reply so far: what was streamed, less what a later `</think>` showed to be reasoning.
    reply: String,
    /// The reply after `</think>` starts without its leading whitespace.
    trim: bool,
}

impl ThinkFilter {
    /// The reply in `piece` that can be streamed now.
    pub fn push(&mut self, piece: &str) -> String {
        self.held.push_str(piece);
        let mut out = String::new();
        loop {
            if self.trim {
                self.held = self.held.trim_start().to_string();
                self.trim = self.held.is_empty();
            }
            if self.thinking {
                let Some(close) = self.held.find(CLOSE) else {
                    self.held
                        .drain(..self.held.len() - partial(&self.held, CLOSE));
                    return out;
                };
                self.held.drain(..close + CLOSE.len());
                (self.thinking, self.trim) = (false, true);
                continue;
            }
            match (self.held.find(OPEN), self.held.find(CLOSE)) {
                (Some(open), close) if close.map_or(true, |close| open < close) => {
                    let before: String = self.held.drain(..open + OPEN.len()).collect();
                    let before = &before[..open];
                    if !(self.reply.is_empty() && before.trim().is_empty()) {
                        self.say(before, &mut out);
                    }
                    self.thinking = true;
                }
                (_, Some(close)) => {
                    self.held.drain(..close + CLOSE.len());
                    self.reply.clear();
                    out.clear();
                    self.trim = true;
                }
                _ if self.reply.is_empty() && self.held.trim().is_empty() => return out,
                _ => {
                    let keep = partial(&self.held, OPEN).max(partial(&self.held, CLOSE));
                    let ready: String = self.held.drain(..self.held.len() - keep).collect();
                    self.say(&ready, &mut out);
                    return out;
                }
            }
        }
    }

    /// What is left to stream once the stream has ended, and the whole reply. Unclosed reasoning is dropped.
    pub fn finish(mut self) -> (String, String) {
        let mut rest = String::new();
        if !self.thinking {
            let held = std::mem::take(&mut self.held);
            self.say(&held, &mut rest);
        }
        (rest, self.reply)
    }

    fn say(&mut self, text: &str, out: &mut String) {
        self.reply.push_str(text);
        out.push_str(text);
    }
}

/// How long the end of `text` is that could be the start of `tag`.
fn partial(text: &str, tag: &str) -> usize {
    (1..tag.len())
        .rev()
        .find(|&n| text.ends_with(&tag[..n]))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What streamed, piece by piece, and the reply.
    fn filtered(pieces: &[&str]) -> (Vec<String>, String) {
        let mut filter = ThinkFilter::default();
        let mut out: Vec<String> = pieces.iter().map(|p| filter.push(p)).collect();
        let (rest, reply) = filter.finish();
        out.push(rest);
        (out, reply)
    }

    #[test]
    fn text_without_tags_is_unchanged() {
        assert_eq!(
            filtered(&["Hello ", "there"]),
            (
                vec!["Hello ".into(), "there".into(), "".into()],
                "Hello there".into()
            )
        );
        assert_eq!(filtered(&[" a < b, ", "x</thin"]).1, " a < b, x</thin");
    }

    #[test]
    fn whole_tags_are_reasoning() {
        assert_eq!(
            filtered(&["<think>plan it</think>\n\nHer name is Lindi."]).1,
            "Her name is Lindi."
        );
        assert_eq!(filtered(&["\n<think>a</think> b"]).1, "b");
    }

    #[test]
    fn tags_split_across_pieces() {
        let (pieces, reply) = filtered(&[
            "<thi",
            "nk>pl",
            "an</th",
            "ink>",
            " \n",
            "Hi",
            " <th",
            "ink>x</think>there",
        ]);
        assert_eq!(reply, "Hi there");
        assert!(pieces
            .iter()
            .all(|p| !p.contains("plan") && !p.contains('<')));
    }

    #[test]
    fn a_closing_tag_alone_ends_reasoning_from_the_start() {
        let (pieces, reply) = filtered(&["plan ", "it</thi", "nk>  Hi"]);
        assert_eq!(reply, "Hi");
        assert_eq!(pieces.last().unwrap(), "");
        assert!(pieces.iter().all(|p| !p.contains('<')));
    }

    #[test]
    fn unclosed_reasoning_is_not_reply() {
        assert_eq!(filtered(&["<think>still", " going"]).1, "");
        assert_eq!(filtered(&["Hi <think>still"]).1, "Hi ");
    }

    #[test]
    fn the_reply_streams_once_reasoning_ends() {
        let (pieces, _) = filtered(&["<think>a</think>", "Hel", "lo"]);
        assert_eq!(pieces, vec!["", "Hel", "lo", ""]);
    }
}

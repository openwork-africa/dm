//! A transcript file: the day as a heading, then each message as it was said, `Person:` or `Agent:` first. A message
//! may run over several lines.

use crate::store::messages::{StoredMessage, USER};

const PERSON: &str = "Person: ";
const AGENT: &str = "Agent: ";

/// The file name of a day's `n`th transcript.
pub fn file_name(day: &str, n: usize) -> String {
    format!("{day}-{n}.md")
}

/// The day a transcript file is of; none for any other file.
pub fn day_of(file_name: &str) -> Option<&str> {
    file_name
        .strip_suffix(".md")?
        .rsplit_once('-')
        .map(|(day, _)| day)
}

pub fn render(day: &str, messages: &[StoredMessage]) -> String {
    let mut lines = vec![format!("# {day}"), String::new()];
    lines.extend(messages.iter().map(|m| {
        format!(
            "{}{}",
            if m.role == USER { PERSON } else { AGENT },
            m.content
        )
    }));
    lines.push(String::new());
    lines.join("\n")
}

/// Each message in a transcript, with who said it.
pub fn entries(transcript: &str) -> Vec<String> {
    let mut said: Vec<String> = vec![];
    for line in transcript.lines() {
        if line.starts_with(PERSON) || line.starts_with(AGENT) {
            said.push(line.to_string());
        } else if let Some(last) = said.last_mut() {
            last.push('\n');
            last.push_str(line);
        }
    }
    for entry in &mut said {
        entry.truncate(entry.trim_end().len());
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_over_several_lines_stays_whole() {
        let message = |role: &str, content: &str| StoredMessage {
            id: 0,
            role: role.into(),
            content: content.into(),
            at: String::new(),
            compacted: false,
        };
        let text = render(
            "2026-11-01",
            &[message("user", "one\ntwo"), message("assistant", "three")],
        );
        assert_eq!(
            entries(&text),
            vec!["Person: one\ntwo".to_string(), "Agent: three".to_string()]
        );
    }
}

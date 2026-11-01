//! Who a conversation is with: an agent, and the person it is talking to (none is kept as `_`).

use anyhow::{bail, Result};

pub const NO_PERSON: &str = "_";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Who {
    pub agent: String,
    /// The person, or `_`.
    pub person: String,
}

/// A name that can be a folder: letters, digits and `.`, `_`, `-`, `@`, `+`, not starting with a dot.
fn check(kind: &str, name: &str) -> Result<()> {
    let fine = |c: char| c.is_ascii_alphanumeric() || "._-@+".contains(c);
    if name.is_empty() || name.len() > 128 || name.starts_with('.') || !name.chars().all(fine) {
        bail!("{kind} `{name}` may hold only letters, digits and . _ - @ +, and may not start with a dot");
    }
    Ok(())
}

impl Who {
    pub fn new(agent: &str, person: Option<&str>) -> Result<Self> {
        check("agent", agent)?;
        let person = person.filter(|p| !p.is_empty()).unwrap_or(NO_PERSON);
        if person != NO_PERSON {
            check("person", person)?;
        }
        Ok(Self {
            agent: agent.to_string(),
            person: person.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cannot_leave_their_folder() {
        assert!(Who::new("../x", None).is_err());
        assert!(Who::new("default", Some("a/b")).is_err());
        assert_eq!(Who::new("default", None).unwrap().person, "_");
        assert_eq!(
            Who::new("default", Some("thandi@example.com"))
                .unwrap()
                .person,
            "thandi@example.com"
        );
    }
}

//! Keys and header values: `$NAME` is an environment variable, `!command` the trimmed output of a shell command (run
//! with `HOME` set, so `~` and `$HOME` work when dm was started without it), and anything else is used as written.
//! They are resolved once per answer, when its model is set up, and never printed: a `Secret` shows as `***` and errors
//! name where the value was looked for, never the value.

use anyhow::{anyhow, bail, Result};
use std::{fmt, process::Command};

pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

pub fn resolve(raw: &str) -> Result<Secret> {
    if let Some(command) = raw.strip_prefix('!') {
        return run(command).map(Secret);
    }
    if let Some(name) = raw.strip_prefix('$') {
        return std::env::var(name)
            .map(Secret)
            .map_err(|_| anyhow!("environment variable `{name}` is not set"));
    }
    Ok(Secret(raw.to_string()))
}

fn run(command: &str) -> Result<String> {
    let mut shell = Command::new("sh");
    shell.arg("-c").arg(command);
    if let Some(home) = crate::config::home::user() {
        shell.env("HOME", home);
    }
    let output = shell
        .output()
        .map_err(|err| anyhow!("could not run the key command `{command}`: {err}"))?;
    if !output.status.success() {
        bail!("the key command `{command}` exited with {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_env_and_command_values_resolve() {
        std::env::set_var("DM_TEST_SECRET_UNIT", "fake-unit-key");
        assert_eq!(resolve("plain").unwrap().expose(), "plain");
        assert_eq!(
            resolve("$DM_TEST_SECRET_UNIT").unwrap().expose(),
            "fake-unit-key"
        );
        assert_eq!(
            resolve("!printf ' fake-cmd-key \\n'").unwrap().expose(),
            "fake-cmd-key"
        );
    }

    #[test]
    fn a_secret_never_shows_its_value() {
        std::env::set_var("DM_TEST_SECRET_DEBUG", "fake-debug-key");
        let secret = resolve("$DM_TEST_SECRET_DEBUG").unwrap();
        assert_eq!(format!("{secret:?}"), "***");
    }

    #[test]
    fn failures_name_the_source_not_the_value() {
        let missing = resolve("$DM_TEST_SURELY_UNSET").unwrap_err().to_string();
        assert!(missing.contains("DM_TEST_SURELY_UNSET"));
        let failed = resolve("!echo fake-leaked-key; exit 3")
            .unwrap_err()
            .to_string();
        assert!(failed.contains("exited"));
        assert!(!failed
            .replace("echo fake-leaked-key", "")
            .contains("fake-leaked-key"));
    }
}

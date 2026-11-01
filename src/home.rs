//! The user's home, and paths that start with `~`, without needing `HOME` to be set.

use std::path::{Path, PathBuf};

/// `$HOME`, else the home the OS has for this user.
pub fn user() -> Option<PathBuf> {
    // Deprecated before Rust 1.87, though on Unix it has always read HOME and then the user database.
    #[allow(deprecated)]
    std::env::home_dir().filter(|home| !home.as_os_str().is_empty())
}

/// `~` or `~/…` from the user's home; any other path as it is.
pub fn expand(path: &Path) -> PathBuf {
    expand_in(path, user().as_deref())
}

fn expand_in(path: &Path, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix("~"), home) {
        (Ok(rest), Some(home)) if rest.as_os_str().is_empty() => home.to_path_buf(),
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_tilde_is_the_home() {
        let home = Path::new("/home/thandi");
        assert_eq!(expand_in(Path::new("~"), Some(home)), home);
        assert_eq!(
            expand_in(Path::new("~/.dm/key"), Some(home)),
            Path::new("/home/thandi/.dm/key")
        );
    }

    #[test]
    fn other_paths_are_left_alone() {
        let home = Path::new("/home/thandi");
        for path in ["/etc/key", "keys/~/a", "~thandi/key", "$HOME/key"] {
            assert_eq!(expand_in(Path::new(path), Some(home)), Path::new(path));
        }
        assert_eq!(expand_in(Path::new("~/key"), None), Path::new("~/key"));
    }

    #[test]
    fn the_users_home_is_found() {
        let home = user().unwrap();
        assert!(home.is_absolute());
        assert_eq!(expand(Path::new("~/x")), home.join("x"));
    }
}

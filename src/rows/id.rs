//! What a row stands for, encoded in fzf's first field.
//!
//! fzf hands the id back as `{1}` to every row key. A session is `$n`, a
//! window `@n` and a pane `%n`, tmux's own ids and valid `-t` targets; a
//! project that has no session yet is its absolute path. When nothing matches the filter `{1}`
//! is empty, and `parse` says so, because tmux would take an empty target to
//! mean the current session.

use std::fmt;
use std::path::PathBuf;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RowId {
    Session(String),
    Window(String),
    /// An agent's pane, in the agents list.
    Pane(String),
    Project(PathBuf),
}

impl RowId {
    pub fn parse(text: &str) -> Option<RowId> {
        match text.as_bytes().first()? {
            b'$' => Some(RowId::Session(text.to_string())),
            b'@' => Some(RowId::Window(text.to_string())),
            b'%' => Some(RowId::Pane(text.to_string())),
            b'/' => Some(RowId::Project(PathBuf::from(text))),
            _ => None,
        }
    }

    /// The tmux target, for what tmux knows about.
    pub fn target(&self) -> Option<&str> {
        match self {
            RowId::Session(id) | RowId::Window(id) | RowId::Pane(id) => Some(id),
            RowId::Project(_) => None,
        }
    }

    /// Whether renaming the row renames a window: a window's, or the window
    /// a pane sits in.
    pub fn names_window(&self) -> bool {
        matches!(self, RowId::Window(_) | RowId::Pane(_))
    }
}

impl fmt::Display for RowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RowId::Session(id) | RowId::Window(id) | RowId::Pane(id) => f.write_str(id),
            RowId::Project(path) => write!(f, "{}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_reject_the_rest() {
        assert_eq!(RowId::parse("$3"), Some(RowId::Session("$3".into())));
        assert_eq!(RowId::parse("@7"), Some(RowId::Window("@7".into())));
        assert_eq!(RowId::parse("/x/y"), Some(RowId::Project("/x/y".into())));
        assert_eq!(RowId::parse(""), None, "an empty {{1}} is not a row");
        assert_eq!(RowId::parse("name"), None);
        assert_eq!(RowId::parse("%2"), Some(RowId::Pane("%2".into())));
        for text in ["$3", "@7", "%2", "/x/y"] {
            assert_eq!(RowId::parse(text).unwrap().to_string(), text);
        }
        assert_eq!(RowId::parse("/x").unwrap().target(), None);
        assert_eq!(RowId::parse("@7").unwrap().target(), Some("@7"));
    }
}

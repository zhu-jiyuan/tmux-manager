//! The seam between the collector and the agent harnesses it watches.
//!
//! A harness (Claude Code, Codex, whatever comes next) is described entirely
//! by one implementation of [`Harness`] in `harness/<name>.rs`: how to
//! recognise its process, how to install its hooks, what its hook events
//! mean, and how to read its state from its records and the screen. The
//! collector in `agent.rs` only ever sees `&dyn Harness`, and the one place
//! a harness is named is [`ALL`]. Adding one is a file in `harness/` and a
//! line there.

pub mod claude;
pub mod codex;

use std::path::Path;
use std::sync::OnceLock;

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::State;

/// Every harness tmm knows, in the order they are tried.
pub const ALL: &[&dyn Harness] = &[&claude::Claude, &codex::Codex];

/// Interpreters a harness may run under; their name alone says nothing.
const INTERPRETERS: &[&str] = &["node", "bun"];

/// The harness `tmm hook <name>` was called for.
pub fn by_name(name: &str) -> Option<&'static dyn Harness> {
    ALL.iter().copied().find(|harness| harness.name() == name)
}

/// The harness a process runs, judged by its command line.
pub fn detect(command: &str) -> Option<&'static dyn Harness> {
    ALL.iter().copied().find(|harness| harness.runs(command))
}

pub trait Harness {
    /// The name on the command line (`tmm hook <name>`) and in records.
    fn name(&self) -> &'static str;

    /// Whether this command line, as `ps` prints it, runs the harness. Judged
    /// by the executable, never by prose in the arguments.
    fn runs(&self, command: &str) -> bool;

    /// Register `command` as this harness's hook and say what changed.
    fn install(&self, command: &str) -> Result<()>;

    /// What a hook call means: `event` from the command line, or whatever
    /// the harness passed on stdin. `None` when it says nothing about activity.
    fn hook(&self, event: Option<&str>, stdin: &str) -> Option<Update>;

    /// What this harness's screen says when no record settles its state.
    fn hints(&self) -> &'static Hints;

    /// The pane's state from this harness's live record and, when it does
    /// not settle it, the screen. `screen` captures once, on first use. Both
    /// harnesses so far name their permission hook `PermissionRequest`; one
    /// that does not overrides this.
    fn pane_state(&self, record: Option<&Record>, screen: &dyn Fn() -> String) -> State {
        match record {
            // A permission hook fires before approval, not when the tool
            // starts. Only a visible busy hint proves the agent has resumed.
            Some(record) if record.event != "PermissionRequest" => record.state,
            _ => self.hints().read(&screen()),
        }
    }
}

/// What a hook stores on the pane. `pid` and `started` tie the record to one
/// specific process so a stale record cannot describe a reused PID.
#[derive(Serialize, Deserialize)]
pub struct Record {
    pub state: State,
    pub pid: i32,
    pub started: String,
    // Records written before the rename stay on the panes of a running server.
    #[serde(alias = "provider")]
    pub harness: String,
    pub event: String,
}

/// What a hook event asks the collector to record.
pub struct Update {
    pub state: State,
    pub event: String,
}

/// A command line as `ps` prints it, split the way harnesses judge it.
pub struct CommandLine<'a> {
    /// The first word: the path the process was started with.
    pub path: &'a str,
    /// Its file name.
    pub executable: &'a str,
    /// The second word: the script, when `executable` is an interpreter.
    pub script: Option<&'a str>,
}

impl<'a> CommandLine<'a> {
    pub fn parse(command: &'a str) -> Self {
        let mut words = command.split_whitespace();
        let path = words.next().unwrap_or("");
        let executable = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        CommandLine {
            path,
            executable,
            script: words.next(),
        }
    }

    /// Whether this is an interpreter running a script whose path contains `needle`.
    pub fn runs_script(&self, needle: &str) -> bool {
        INTERPRETERS.contains(&self.executable) && self.script.is_some_and(|s| s.contains(needle))
    }
}

/// A hook call as the harnesses with JSON on stdin see it: the event named on
/// the command line, else by `hook_event_name` in the payload.
pub struct Hook {
    pub event: String,
    pub payload: Value,
}

impl Hook {
    pub fn parse(event: Option<&str>, stdin: &str) -> Self {
        let payload = match event {
            Some(_) => Value::Null,
            None => serde_json::from_str(stdin).unwrap_or(Value::Null),
        };
        let event = event
            .or_else(|| payload.get("hook_event_name").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
        Hook { event, payload }
    }

    pub fn string(&self, key: &str) -> &str {
        self.payload.get(key).and_then(Value::as_str).unwrap_or("")
    }
}

/// A harness's screen regexes, compiled on first use. A question or limit
/// message wins over a busy hint, and no busy hint at all means waiting.
pub struct Hints {
    waiting: &'static str,
    working: &'static str,
    compiled: OnceLock<(Regex, Regex)>,
}

/// How many of the captured lines the hints are read from.
const TAIL_LINES: usize = 14;

impl Hints {
    pub const fn new(waiting: &'static str, working: &'static str) -> Self {
        Hints {
            waiting,
            working,
            compiled: OnceLock::new(),
        }
    }

    pub fn read(&self, screen: &str) -> State {
        let (waiting, working) = self.compiled.get_or_init(|| {
            (
                Regex::new(self.waiting).unwrap(),
                Regex::new(self.working).unwrap(),
            )
        });
        let lines: Vec<&str> = screen.lines().collect();
        let start = lines.len().saturating_sub(TAIL_LINES);
        let tail = lines[start..].join("\n").to_lowercase();
        if waiting.is_match(&tail) {
            State::Waiting
        } else if working.is_match(&tail) {
            State::Working
        } else {
            State::Waiting
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn detection_needs_a_harness_executable() {
        assert!(detect("").is_none());
        assert!(
            detect("bash -c 'echo codex working'").is_none(),
            "prose in arguments is not an executable"
        );
    }

    #[test]
    fn names_are_unique() {
        let names: HashSet<&str> = ALL.iter().map(|harness| harness.name()).collect();
        assert_eq!(names.len(), ALL.len(), "harness names collide: {names:?}");
        assert!(by_name("nope").is_none());
    }

    #[test]
    fn records_settle_the_state_unless_a_permission_is_pending() {
        let record = |state, event: &str| Record {
            state,
            pid: 1,
            started: "x".into(),
            harness: "test".into(),
            event: event.into(),
        };
        let busy = || "Working (esc to interrupt)".to_string();
        let idle = || "$ ".to_string();
        for harness in ALL {
            let name = harness.name();
            let stop = record(State::Waiting, "Stop");
            let permission = record(State::Waiting, "PermissionRequest");
            assert_eq!(
                harness.pane_state(Some(&stop), &busy),
                State::Waiting,
                "{name}: a record wins over the screen"
            );
            assert_eq!(
                harness.pane_state(Some(&permission), &busy),
                State::Working,
                "{name}: a busy hint after a permission hook means it resumed"
            );
            assert_eq!(
                harness.pane_state(Some(&permission), &idle),
                State::Waiting,
                "{name}: no busy hint, still waiting for approval"
            );
            assert_eq!(
                harness.pane_state(None, &busy),
                State::Working,
                "{name}: no record, the screen decides"
            );
        }
    }

    #[test]
    fn hook_input_names_the_event_either_way() {
        let hook = Hook::parse(Some("Stop"), r#"{"hook_event_name": "ignored"}"#);
        assert_eq!(hook.event, "Stop");
        assert!(
            hook.payload.is_null(),
            "stdin is not read when the event is given"
        );
        let hook = Hook::parse(
            None,
            r#"{"hook_event_name": "PreToolUse", "tool_name": "Bash"}"#,
        );
        assert_eq!(
            (hook.event.as_str(), hook.string("tool_name")),
            ("PreToolUse", "Bash")
        );
        assert_eq!(Hook::parse(None, "not json").event, "");
    }

    #[test]
    fn old_records_still_parse() {
        let record: Record = serde_json::from_str(
            r#"{"state":"working","pid":1,"started":"x","provider":"codex","event":"Stop"}"#,
        )
        .expect("the provider alias reads records left by older binaries");
        assert_eq!(record.harness, "codex");
        assert!(
            serde_json::to_string(&record)
                .unwrap()
                .contains("\"harness\":\"codex\"")
        );
    }
}

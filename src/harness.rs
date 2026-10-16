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

use crate::agent::{Reason, State, Status};

/// Every harness tm knows, in the order they are tried.
pub const ALL: &[&dyn Harness] = &[&claude::Claude, &codex::Codex];

/// Interpreters a harness may run under; their name alone says nothing.
const INTERPRETERS: &[&str] = &["node", "bun"];

/// The harness `tm hook <name>` was called for.
pub fn by_name(name: &str) -> Option<&'static dyn Harness> {
    ALL.iter().copied().find(|harness| harness.name() == name)
}

/// The harness a process runs, judged by its command line.
pub fn detect(command: &str) -> Option<&'static dyn Harness> {
    ALL.iter().copied().find(|harness| harness.runs(command))
}

pub trait Harness {
    /// The name on the command line (`tm hook <name>`) and in records.
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

    /// The session's title, from the one it set on its pane (`title`, empty
    /// when it set none). A harness that decorates its title overrides this.
    fn title(&self, title: &str) -> Option<String> {
        let title = title.trim();
        (!title.is_empty()).then(|| title.to_string())
    }

    /// The pane's status from this harness's live record and, when it does
    /// not settle it, the screen. `screen` captures once, on first use. Both
    /// harnesses so far name their permission hook `PermissionRequest`; one
    /// that does not overrides this.
    fn pane_state(&self, record: Option<&Record>, screen: &dyn Fn() -> String) -> Status {
        match record {
            // A permission hook fires before approval, not when the tool
            // starts. Only a visible busy hint proves the agent has resumed.
            Some(record) if record.event == "PermissionRequest" => {
                let seen = self.hints().read(&screen());
                if seen.state == State::Working {
                    seen
                } else {
                    Status {
                        since: record.since,
                        ..Status::new(State::Waiting, Some(Reason::Permission))
                    }
                }
            }
            Some(record) => Status {
                since: record.since,
                ..Status::new(record.state, record.reason)
            },
            None => self.hints().read(&screen()),
        }
    }
}

/// What a hook stores on the pane. `pid` and `started` tie the record to one
/// specific process so a stale record cannot describe a reused PID.
#[derive(Serialize, Deserialize)]
pub struct Record {
    pub state: State,
    /// Absent from records written before reasons existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    /// When the process entered this state, in Unix seconds; absent from
    /// records written before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    pub pid: i32,
    pub started: String,
    pub harness: String,
    pub event: String,
}

/// What a hook event asks the collector to record.
pub struct Update {
    pub state: State,
    pub reason: Option<Reason>,
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

/// A harness's screen regexes, compiled on first use. A permission prompt,
/// a question or a limit message wins over a busy hint, and no busy hint at
/// all means waiting, for no reason the screen tells.
pub struct Hints {
    permission: &'static str,
    question: &'static str,
    working: &'static str,
    compiled: OnceLock<[Regex; 4]>,
}

/// The limit messages both harnesses print when they stop for the user's
/// plan or the API's rate.
const LIMIT: &str = r"you.ve hit your limit|usage limit reached|rate limit exceeded";

/// How many of the captured lines, counted up from the last one with text,
/// the hints are read from. Enough to reach Claude Code's spinner above its
/// input box and status lines, and little enough to miss old output.
const TAIL_LINES: usize = 14;

impl Hints {
    pub const fn new(
        permission: &'static str,
        question: &'static str,
        working: &'static str,
    ) -> Self {
        Hints {
            permission,
            question,
            working,
            compiled: OnceLock::new(),
        }
    }

    pub fn read(&self, screen: &str) -> Status {
        let [permission, question, limit, working] = self.compiled.get_or_init(|| {
            [self.permission, self.question, LIMIT, self.working].map(|re| Regex::new(re).unwrap())
        });
        // capture-pane prints the empty rows below the last output too; a
        // tall pane with little on it would otherwise leave nothing to read.
        let mut lines: Vec<&str> = screen.lines().collect();
        while lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }
        let start = lines.len().saturating_sub(TAIL_LINES);
        let tail = lines[start..].join("\n").to_lowercase();
        let waiting = |reason| Status::new(State::Waiting, Some(reason));
        if permission.is_match(&tail) {
            waiting(Reason::Permission)
        } else if question.is_match(&tail) {
            waiting(Reason::Question)
        } else if limit.is_match(&tail) {
            waiting(Reason::Error)
        } else if working.is_match(&tail) {
            Status::new(State::Working, None)
        } else {
            Status::new(State::Waiting, None)
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
        let record = |state, reason, event: &str| Record {
            state,
            reason,
            since: None,
            pid: 1,
            started: "x".into(),
            harness: "test".into(),
            event: event.into(),
        };
        let busy = || "Working (esc to interrupt)".to_string();
        let idle = || "$ ".to_string();
        let waiting = |reason| Status::new(State::Waiting, reason);
        for harness in ALL {
            let name = harness.name();
            let stop = record(State::Waiting, Some(Reason::Done), "Stop");
            let permission = record(
                State::Waiting,
                Some(Reason::Permission),
                "PermissionRequest",
            );
            assert_eq!(
                harness.pane_state(Some(&stop), &busy),
                waiting(Some(Reason::Done)),
                "{name}: a record wins over the screen"
            );
            assert_eq!(
                harness.pane_state(Some(&permission), &busy).state,
                State::Working,
                "{name}: a busy hint after a permission hook means it resumed"
            );
            assert_eq!(
                harness.pane_state(Some(&permission), &idle),
                waiting(Some(Reason::Permission)),
                "{name}: no busy hint, still waiting for approval"
            );
            assert_eq!(
                harness.pane_state(None, &busy).state,
                State::Working,
                "{name}: no record, the screen decides"
            );
            assert_eq!(
                harness.pane_state(None, &idle),
                waiting(None),
                "{name}: an idle screen says nothing about why"
            );
            assert_eq!(
                harness.pane_state(None, &|| "You've hit your limit".to_string()),
                waiting(Some(Reason::Error)),
                "{name}: a limit stops the turn"
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
}

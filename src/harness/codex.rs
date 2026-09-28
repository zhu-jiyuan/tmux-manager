//! OpenAI Codex.

use anyhow::Result;

use super::{CommandLine, Harness, Hints, Hook, Update};
use crate::agent::State;
use crate::install;

pub struct Codex;

const HOOKS_FILE: &str = ".codex/hooks.json";
const EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Stop",
    "Interrupt",
    "PreCompact",
    "PostCompact",
];
/// Tools whose `PreToolUse` means the agent is asking the user something.
/// Codex names them `functions.request_user_input`; only the last part counts.
const WAIT_TOOLS: &[&str] = &["request_user_input", "request_user_input_async"];
/// Codex's own status line: its approval prompts, its limit messages, and
/// the busy hint it shows while a turn runs.
static HINTS: Hints = Hints::new(
    r"would you like to run|requires? (?:your )?approval|enter to confirm|waiting for (?:your|user)|you.ve hit your limit|usage limit reached|rate limit exceeded",
    r"esc(?:ape)? to (?:interrupt|stop)|ctrl[+-]c to interrupt",
);

impl Harness for Codex {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn runs(&self, command: &str) -> bool {
        let command = CommandLine::parse(command);
        // Started as `codex`; the per-platform binary the npm wrapper runs
        // (`codex-aarch64-apple-darwin` and the like); or the npm package.
        command.executable == "codex"
            || command.executable.starts_with("codex-aarch64-")
            || command.executable.starts_with("codex-x86_64-")
            || command.runs_script("/@openai/codex/")
    }

    fn install(&self, command: &str) -> Result<()> {
        if install::merge_home("Codex", HOOKS_FILE, EVENTS, command)? {
            println!("Codex: review the new hooks in its /hooks UI.");
        }
        Ok(())
    }

    fn hook(&self, event: Option<&str>, stdin: &str) -> Option<Update> {
        let hook = Hook::parse(event, stdin);
        // Hooks fired inside a spawned agent carry its id. A child agent must
        // not overwrite the interactive parent's state.
        if !hook.string("agent_id").is_empty() {
            return None;
        }
        let state = match hook.event.as_str() {
            "SessionEnd" => State::Plain,
            "SessionStart" | "Stop" | "Interrupt" | "PermissionRequest" => State::Waiting,
            "PreToolUse" => {
                let tool = hook.string("tool_name");
                let tool = tool.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
                if WAIT_TOOLS.contains(&tool.as_str()) {
                    State::Waiting
                } else {
                    State::Working
                }
            }
            "UserPromptSubmit" | "PostToolUse" | "PreCompact" | "PostCompact" => State::Working,
            _ => return None,
        };
        Some(Update {
            state,
            event: hook.event,
        })
    }

    fn hints(&self) -> &'static Hints {
        &HINTS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(event: Option<&str>, stdin: &str) -> Option<State> {
        Codex.hook(event, stdin).map(|update| update.state)
    }

    #[test]
    fn recognised_by_executable() {
        assert!(Codex.runs("/opt/homebrew/bin/codex --full-auto"));
        assert!(Codex.runs("codex-x86_64-unknown-linux-musl"));
        assert!(Codex.runs("node /usr/lib/node_modules/@openai/codex/bin/codex.js"));
        assert!(!Codex.runs("/Users/me/.local/share/claude/versions/2.1.263"));
    }

    #[test]
    fn events_map_to_states() {
        for event in ["UserPromptSubmit", "PostToolUse", "PostCompact"] {
            assert_eq!(state(Some(event), ""), Some(State::Working), "{event}");
        }
        for event in ["Stop", "Interrupt", "PermissionRequest", "SessionStart"] {
            assert_eq!(state(Some(event), ""), Some(State::Waiting), "{event}");
        }
        assert_eq!(state(Some("SessionEnd"), ""), Some(State::Plain));
        assert_eq!(state(Some("Notification"), ""), None, "not a Codex event");
        assert_eq!(
            state(
                None,
                r#"{"hook_event_name":"PreToolUse","tool_name":"functions.request_user_input"}"#
            ),
            Some(State::Waiting)
        );
        assert_eq!(
            state(
                None,
                r#"{"hook_event_name":"PreToolUse","tool_name":"shell"}"#
            ),
            Some(State::Working)
        );
        assert_eq!(
            state(
                None,
                r#"{"hook_event_name":"PostToolUse","agent_id":"child-1"}"#
            ),
            None,
            "a spawned agent's hooks say nothing about the parent"
        );
    }

    #[test]
    fn screen_hints() {
        assert_eq!(HINTS.read("Working (esc to interrupt)"), State::Working);
        assert_eq!(
            HINTS.read("esc to interrupt\nWould you like to run the command?"),
            State::Waiting
        );
        assert_eq!(HINTS.read("This action requires approval"), State::Waiting);
        assert_eq!(HINTS.read("$ "), State::Waiting);
    }
}

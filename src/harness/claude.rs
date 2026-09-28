//! Claude Code.

use anyhow::Result;

use super::{CommandLine, Harness, Hints, Hook, Update};
use crate::agent::State;
use crate::install;

pub struct Claude;

const HOOKS_FILE: &str = ".claude/settings.json";
const EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Notification",
    "Stop",
    "StopFailure",
    "PreCompact",
];
/// Tools whose `PreToolUse` means the agent is asking the user something.
const WAIT_TOOLS: &[&str] = &["askuserquestion", "enterplanmode", "exitplanmode"];
/// The notifications that ask for the user, out of the many kinds sent.
const WAIT_NOTIFICATIONS: &[&str] = &["permission_prompt", "idle_prompt", "elicitation_dialog"];
/// Claude Code's own status line: its permission and plan prompts, its limit
/// messages, and the busy hint it shows while a turn runs. Since 2.1 the busy
/// hint is only the spinner line, `✻ Verbing…` then `✻ Verbing… (3s · …)`;
/// a finished turn leaves `✻ Verbed for 3s` behind, with no ellipsis.
static HINTS: Hints = Hints::new(
    r"do you want to (?:proceed|create|make this edit)|enter to confirm|waiting for (?:your|user)|you.ve hit your limit|usage limit reached|rate limit exceeded",
    r"(?m)esc(?:ape)? to (?:interrupt|stop)|ctrl[+-]c to interrupt|^[·✢✳✶✻✽*] [^\n…]+…(?: \(|[ \t]*$)",
);

impl Harness for Claude {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn runs(&self, command: &str) -> bool {
        let command = CommandLine::parse(command);
        // Started as `claude`; the native installer's binary, named after its
        // version under ~/.local/share/claude/versions/; or the npm package.
        command.executable == "claude"
            || command.path.contains("/claude/versions/")
            || command.runs_script("/@anthropic-ai/claude-code/")
    }

    fn install(&self, command: &str) -> Result<()> {
        install::merge_home("Claude Code", HOOKS_FILE, EVENTS, command)?;
        Ok(())
    }

    fn hook(&self, event: Option<&str>, stdin: &str) -> Option<Update> {
        let hook = Hook::parse(event, stdin);
        // Hooks fired inside a subagent carry its id. A background subagent
        // must not overwrite the interactive parent's state.
        if !hook.string("agent_id").is_empty() {
            return None;
        }
        let state = match hook.event.as_str() {
            "SessionEnd" => State::Plain,
            "SessionStart" | "Stop" | "StopFailure" | "PermissionRequest" => State::Waiting,
            "Notification" if WAIT_NOTIFICATIONS.contains(&hook.string("notification_type")) => {
                State::Waiting
            }
            "PreToolUse" => {
                let tool = hook.string("tool_name").to_ascii_lowercase();
                if WAIT_TOOLS.contains(&tool.as_str()) {
                    State::Waiting
                } else {
                    State::Working
                }
            }
            "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure" | "PreCompact" => {
                State::Working
            }
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
        Claude.hook(event, stdin).map(|update| update.state)
    }

    #[test]
    fn recognised_by_executable() {
        assert!(Claude.runs("/Users/me/.local/share/claude/versions/2.1.263"));
        assert!(Claude.runs("claude --resume"));
        assert!(Claude.runs("node /usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"));
        assert!(!Claude.runs("node /usr/lib/node_modules/@openai/codex/bin/codex.js"));
    }

    #[test]
    fn events_map_to_states() {
        for event in ["UserPromptSubmit", "PostToolUse", "PreCompact"] {
            assert_eq!(state(Some(event), ""), Some(State::Working), "{event}");
        }
        for event in ["Stop", "StopFailure", "PermissionRequest", "SessionStart"] {
            assert_eq!(state(Some(event), ""), Some(State::Waiting), "{event}");
        }
        assert_eq!(state(Some("SessionEnd"), ""), Some(State::Plain));
        assert_eq!(
            state(Some("Interrupt"), ""),
            None,
            "not a Claude Code event"
        );
        assert_eq!(
            state(
                None,
                r#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion"}"#
            ),
            Some(State::Waiting)
        );
        assert_eq!(
            state(
                None,
                r#"{"hook_event_name":"PreToolUse","tool_name":"Bash"}"#
            ),
            Some(State::Working)
        );
        assert_eq!(
            state(
                None,
                r#"{"hook_event_name":"Notification","notification_type":"unrelated"}"#
            ),
            None
        );
        assert_eq!(
            state(
                None,
                r#"{"hook_event_name":"Notification","notification_type":"idle_prompt"}"#
            ),
            Some(State::Waiting)
        );
        assert_eq!(
            state(
                None,
                r#"{"hook_event_name":"PostToolUse","agent_id":"sub-1"}"#
            ),
            None,
            "a subagent's hooks say nothing about the parent"
        );
        let update = Claude.hook(Some("Stop"), "").unwrap();
        assert_eq!(update.event, "Stop");
    }

    #[test]
    fn screen_hints() {
        assert_eq!(HINTS.read("Working (esc to interrupt)"), State::Working);
        assert_eq!(
            HINTS.read("esc to interrupt\nDo you want to proceed?"),
            State::Waiting
        );
        assert_eq!(HINTS.read("You’ve hit your limit"), State::Waiting);
        assert_eq!(HINTS.read("$ "), State::Waiting);
    }

    /// The bottom of a 2.1 screen, as `capture-pane` prints it.
    fn screen(above_input: &str) -> String {
        let rule = "─".repeat(40);
        format!(
            "⏺ I'll create hello.txt.\n\n{above_input}\n\n{rule}\n❯ \n{rule}\n  Haiku 4.5  $0.00\n  ⏸ manual mode on\n"
        )
    }

    #[test]
    fn spinner_means_working() {
        for spinner in [
            "✻ Waddling…",
            "✢ Lollygagging… (1s · thinking)",
            "· Lollygagging… (3s · ↓ 138 tokens · thought for 2s)",
            "✶ Reticulating splines… (running PreToolUse hook · 4s)",
            "* Fermenting… (5m 50s · ↓ 10.6k tokens)",
        ] {
            assert_eq!(HINTS.read(&screen(spinner)), State::Working, "{spinner}");
        }
        assert_eq!(
            HINTS.read(&screen("✻ Worked for 5s · done 12:31 AM")),
            State::Waiting,
            "a finished turn"
        );
        assert_eq!(
            HINTS.read(&screen("⏺ Loading… (please wait)")),
            State::Waiting,
            "an ellipsis in the transcript is not the spinner"
        );
        assert_eq!(
            HINTS.read(&format!("{}{}", screen("✻ Waddling…"), "\n".repeat(30))),
            State::Working,
            "the empty rows under a young session are not the tail"
        );
        assert_eq!(
            HINTS.read(" Do you want to create hello.txt?\n ❯ 1. Yes\n   3. No\n\n Esc to cancel · Tab to amend"),
            State::Waiting
        );
    }
}

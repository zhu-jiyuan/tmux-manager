//! Claude Code.

use anyhow::Result;

use super::{CommandLine, Harness, Hints, Hook, Update};
use crate::agent::{Reason, State};
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
/// Claude Code's own status line: its permission prompts, its questions and
/// plan prompts, and the busy hint it shows while a turn runs. Since 2.1 the
/// busy hint is only the spinner line, `✻ Verbing…` then `✻ Verbing… (3s ·
/// …)`; a finished turn leaves `✻ Verbed for 3s` behind, with no ellipsis.
static HINTS: Hints = Hints::new(
    r"do you want to (?:proceed|create|make this edit)",
    r"enter to confirm|waiting for (?:your|user)",
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
        let (state, reason) = match hook.event.as_str() {
            "SessionEnd" => (State::Plain, None),
            "SessionStart" => (State::Waiting, None),
            "Stop" => (State::Waiting, Some(Reason::Done)),
            "StopFailure" => (State::Waiting, Some(Reason::Error)),
            "PermissionRequest" => (State::Waiting, Some(Reason::Permission)),
            // Of the many kinds sent, the ones that ask for the user. The idle
            // one comes a minute into any wait, a question's too, so it gives
            // no reason; it ends a turn the user interrupted, which no hook marks.
            "Notification" => match hook.string("notification_type") {
                "permission_prompt" => (State::Waiting, Some(Reason::Permission)),
                "elicitation_dialog" => (State::Waiting, Some(Reason::Question)),
                "idle_prompt" => (State::Waiting, None),
                _ => return None,
            },
            "PreToolUse" => {
                let tool = hook.string("tool_name").to_ascii_lowercase();
                if WAIT_TOOLS.contains(&tool.as_str()) {
                    (State::Waiting, Some(Reason::Question))
                } else {
                    (State::Working, None)
                }
            }
            "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure" | "PreCompact" => {
                (State::Working, None)
            }
            _ => return None,
        };
        Some(Update {
            state,
            reason,
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
    use crate::agent::Status;

    fn state(event: Option<&str>, stdin: &str) -> Option<State> {
        Claude.hook(event, stdin).map(|update| update.state)
    }

    fn reason(event: Option<&str>, stdin: &str) -> Option<Reason> {
        Claude.hook(event, stdin).and_then(|update| update.reason)
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
    fn events_say_why_it_waits() {
        assert_eq!(reason(Some("Stop"), ""), Some(Reason::Done));
        assert_eq!(reason(Some("StopFailure"), ""), Some(Reason::Error));
        assert_eq!(
            reason(Some("PermissionRequest"), ""),
            Some(Reason::Permission)
        );
        assert_eq!(reason(Some("SessionStart"), ""), None);
        let notification = |kind: &str| {
            reason(
                None,
                &format!(r#"{{"hook_event_name":"Notification","notification_type":"{kind}"}}"#),
            )
        };
        assert_eq!(notification("permission_prompt"), Some(Reason::Permission));
        assert_eq!(notification("elicitation_dialog"), Some(Reason::Question));
        assert_eq!(
            notification("idle_prompt"),
            None,
            "idle says nothing of why"
        );
        assert_eq!(
            reason(
                None,
                r#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion"}"#
            ),
            Some(Reason::Question)
        );
    }

    #[test]
    fn screen_hints() {
        let read = |screen| {
            let status = HINTS.read(screen);
            (status.state, status.reason)
        };
        assert_eq!(read("Working (esc to interrupt)"), (State::Working, None));
        assert_eq!(
            read("esc to interrupt\nDo you want to proceed?"),
            (State::Waiting, Some(Reason::Permission))
        );
        assert_eq!(
            read("Enter to confirm · Esc to cancel"),
            (State::Waiting, Some(Reason::Question))
        );
        assert_eq!(
            read("You’ve hit your limit"),
            (State::Waiting, Some(Reason::Error))
        );
        assert_eq!(read("$ "), (State::Waiting, None));
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
            assert_eq!(
                HINTS.read(&screen(spinner)).state,
                State::Working,
                "{spinner}"
            );
        }
        assert_eq!(
            HINTS.read(&screen("✻ Worked for 5s · done 12:31 AM")).state,
            State::Waiting,
            "a finished turn"
        );
        assert_eq!(
            HINTS.read(&screen("⏺ Loading… (please wait)")).state,
            State::Waiting,
            "an ellipsis in the transcript is not the spinner"
        );
        assert_eq!(
            HINTS
                .read(&format!("{}{}", screen("✻ Waddling…"), "\n".repeat(30)))
                .state,
            State::Working,
            "the empty rows under a young session are not the tail"
        );
        assert_eq!(
            HINTS.read(" Do you want to create hello.txt?\n ❯ 1. Yes\n   3. No\n\n Esc to cancel · Tab to amend"),
            Status::new(State::Waiting, Some(Reason::Permission))
        );
    }
}

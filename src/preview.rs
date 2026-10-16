//! The preview pane: the selected row's window, redrawn by fzf once a second.
//!
//! A row previews the pane whose agent most wants attention, the one behind
//! its dots: a session row looks through all its windows, a window row
//! through its panes. Without an agent it is the active one. `ctrl-l` moves
//! a session row on to the next window without changing which window tmux
//! considers active. That choice lives in the popup's `Preview` sidecar, so
//! it survives fzf restarting the preview command whenever the cursor moves. A project that is not open previews
//! its directory instead: where it is, its git state, what is in it.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use std::{env, io};

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;

use crate::agent::{self, State};
use crate::ansi::{Colour, bold, clip_line, faint, fit_frame, tint};
use crate::popup::{Popup, Sidecar};
use crate::projects;
use crate::rows::RowId;
use crate::tmux::{self, Pane};

const GONE: &str = "Gone: this row no longer exists";

/// Each session's chosen preview window.
fn choices(popup: &Popup) -> Result<BTreeMap<String, String>> {
    Ok(popup.read_json(Sidecar::Preview)?.unwrap_or_default())
}

/// The pane most worth seeing among one session's `panes`: within `window`
/// while it exists, else anywhere. The agent that most wants attention wins,
/// so the preview shows what the row's dots announce. Without one, or on a
/// tie, the active window, else the first; within a window, its active pane.
fn pick<'a>(
    panes: &'a [Pane],
    states: &HashMap<String, State>,
    window: Option<&str>,
) -> Option<&'a Pane> {
    let window = window.filter(|id| panes.iter().any(|pane| pane.window_id == *id));
    panes
        .iter()
        .filter(|pane| window.is_none_or(|id| pane.window_id == id))
        // max_by_key keeps the last of equals; reversed, that is the first.
        .rev()
        .max_by_key(|pane| {
            let state = states.get(&pane.id).copied().unwrap_or(State::Plain);
            (
                state,
                pane.window_active,
                Reverse(pane.window_index),
                pane.active,
            )
        })
}

/// The pane a row previews: the best of a window row's window, or of the
/// window `ctrl-l` chose for a session row, else of the whole session.
fn target(popup: &Popup, id: &RowId) -> Result<Pane> {
    let (scope, window) = match id {
        RowId::Window(window) => (window, Some(window.clone())),
        RowId::Session(session) => (session, choices(popup)?.remove(session)),
        RowId::Project(path) => bail!("{} has no window", path.display()),
    };
    let panes = tmux::session_panes(scope)?;
    let states = agent::pane_states(&panes)?;
    pick(&panes, &states, window.as_deref())
        .cloned()
        .context("a session with no windows")
}

/// `ctrl-l`: move a session row's preview to its next window, wrapping.
pub fn next(row: &str) -> Result<()> {
    let Some(RowId::Session(session)) = RowId::parse(row) else {
        return Ok(());
    };
    advance(&Popup::current(), &session)
}

/// Move `session`'s preview in `popup` to the window after the one it
/// shows, wrapping.
fn advance(popup: &Popup, session: &str) -> Result<()> {
    let panes = tmux::session_panes(session)?;
    let states = agent::pane_states(&panes)?;
    let mut choices = choices(popup)?;
    let Some(now) = pick(&panes, &states, choices.get(session).map(String::as_str)) else {
        return Ok(());
    };
    // A session's panes come window by window, so this leaves one id each.
    let mut windows: Vec<&str> = panes.iter().map(|pane| pane.window_id.as_str()).collect();
    windows.dedup();
    let position = windows
        .iter()
        .position(|id| *id == now.window_id)
        .unwrap_or(0);
    let following = windows[(position + 1) % windows.len()].to_string();
    choices.insert(session.to_string(), following);
    popup.write_json(Sidecar::Preview, &choices)
}

/// One frame: a bold title line, a blank line, then the captured screen,
/// clipped to the viewport so fzf shows no scroll indicator. A project with
/// no session previews its directory instead.
fn render(popup: &Popup, id: &RowId, columns: usize, lines: usize) -> String {
    if let RowId::Project(path) = id {
        return directory(path, columns, lines);
    }
    let Ok(pane) = target(popup, id) else {
        return bold(GONE);
    };
    let mut title = format!(
        "{}  ·  {}: {}",
        pane.session_name, pane.window_index, pane.window_name
    );
    if pane.window_panes > 1 {
        title.push_str(&format!("  ·  {} panes", pane.window_panes));
    }
    let capture = tmux::run_lossy(&["capture-pane", "-p", "-e", "-t", &pane.id]);
    let frame = fit_frame(&capture, columns, lines.saturating_sub(2));
    let text = format!("{}\n\n{frame}", bold(&clip_line(&title, columns)));
    // An empty capture would leave trailing newlines; fzf counts those as lines.
    text.trim_end_matches('\n').to_string()
}

fn git(path: &Path, args: &[&str]) -> String {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

/// A project with no session yet: a bold path, the git branch and last
/// commit when there is a repository, then what is inside, directories first.
fn directory(path: &Path, columns: usize, lines: usize) -> String {
    let mut out = vec![bold(&projects::shorten(path)), String::new()];
    if path.join(".git").exists() {
        let branch = git(path, &["symbolic-ref", "--short", "-q", "HEAD"]);
        let last = git(path, &["log", "-1", "--format=%h %s"]);
        if !last.is_empty() {
            let branch = if branch.is_empty() { "HEAD" } else { &branch };
            out.push(format!("{} {last}", faint(branch)));
            out.push(String::new());
        }
    }
    let mut entries: Vec<(bool, String)> = fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        return None;
                    }
                    Some((entry.file_type().ok()?.is_dir(), name))
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    for (dir, name) in entries {
        out.push(if dir {
            tint(&format!("{name}/"), Colour::Blue)
        } else {
            name
        });
    }
    out.truncate(lines);
    out.iter()
        .map(|line| clip_line(line, columns))
        .collect::<Vec<_>>()
        .join("\n")
}

/// fzf's preview command; fzf re-runs it on a timer, and a process that
/// stayed alive would make it show a spinner.
pub fn run(row: &str) -> Result<()> {
    let Some(id) = RowId::parse(row) else {
        return Ok(());
    };
    let popup = Popup::current();
    // fzf says how big its preview window is; a shell gets the terminal's size.
    let (columns, lines) = match (
        env::var("FZF_PREVIEW_COLUMNS"),
        env::var("FZF_PREVIEW_LINES"),
    ) {
        (Ok(columns), Ok(lines)) => (columns.parse()?, lines.parse()?),
        _ => terminal::size().map_or((80, 24), |(columns, lines)| {
            (usize::from(columns), usize::from(lines))
        }),
    };
    let frame = render(&popup, &id, columns, lines);
    io::stdout().write_all(frame.as_bytes())?;
    Ok(())
}

/// Restores the terminal even if rendering fails half-way. Dropping the guard
/// runs the cleanup, which is Rust's way of writing `finally`.
struct RawTerminal {
    tty: fs::File,
}

impl RawTerminal {
    fn enter() -> Result<Self> {
        let mut tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .context("opening /dev/tty")?;
        terminal::enable_raw_mode()?;
        tty.write_all(b"\x1b[?1049h\x1b[?25l")?;
        Ok(RawTerminal { tty })
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        let _ = self.tty.write_all(b"\x1b[?25h\x1b[?1049l");
        let _ = terminal::disable_raw_mode();
    }
}

/// `ctrl-t`: fzf suspends its list and we fill the popup with the preview
/// until a key asks to leave. The fzf action to run on return is left in
/// the `View` sidecar for [`view_return`].
pub fn full(row: &str) -> Result<()> {
    let Some(id) = RowId::parse(row) else {
        return Ok(());
    };
    let popup = Popup::current();
    let mut terminal = RawTerminal::enter()?;
    let back = loop {
        let (columns, lines) = terminal::size()?;
        let frame = render(&popup, &id, columns as usize, lines as usize).replace('\n', "\r\n");
        write!(terminal.tty, "\x1b[H\x1b[2J{frame}")?;
        terminal.tty.flush()?;
        if !event::poll(Duration::from_secs(1))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match (key.code, key.modifiers) {
            // Only forward: ctrl-h reaches us as Backspace through tmux, so a
            // single wrapping key is the honest option. `advance` takes this
            // popup, so the choice holds in one kept in memory too.
            (KeyCode::Char('l'), _) => {
                if let RowId::Session(session) = &id {
                    advance(&popup, session)?;
                }
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => break "abort",
            (KeyCode::Enter, _) => break "accept",
            (KeyCode::Char('v'), KeyModifiers::CONTROL) => break "hide-preview",
            (KeyCode::Char('t'), KeyModifiers::CONTROL) => break "",
            _ => {}
        }
    };
    drop(terminal);
    popup.write(Sidecar::View, back)
}

/// After the full view: replay the action it left behind.
pub fn view_return() -> Result<()> {
    let popup = Popup::current();
    println!("{}", popup.take(Sidecar::View)?.unwrap_or_default());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pane `%n` in window `@w`; window `@0` and the first pane of each
    /// window are the active ones.
    fn pane(window: usize, n: usize, active: bool) -> Pane {
        Pane {
            session_id: "$0".into(),
            session_name: "s".into(),
            session_attached: false,
            session_path: "/nowhere".into(),
            window_id: format!("@{window}"),
            window_index: window,
            window_name: format!("w{window}"),
            window_active: window == 0,
            window_panes: 2,
            id: format!("%{n}"),
            active,
            pid: 1,
            dead: false,
            tty: String::new(),
            at_prompt: true,
            record: None,
        }
    }

    fn picked(panes: &[Pane], states: &[(&str, State)], window: Option<&str>) -> String {
        let states = states
            .iter()
            .map(|(id, state)| (id.to_string(), *state))
            .collect();
        pick(panes, &states, window).unwrap().id.clone()
    }

    #[test]
    fn the_agent_that_wants_attention_is_shown() {
        // Window @0 is active with a shell in front; agents sit in @1 and @2.
        let panes = [
            pane(0, 0, true),
            pane(0, 1, false),
            pane(1, 2, true),
            pane(1, 3, false),
            pane(2, 4, true),
        ];
        assert_eq!(picked(&panes, &[], None), "%0", "no agent: the active pane");
        assert_eq!(
            picked(&panes, &[("%3", State::Working)], None),
            "%3",
            "an agent anywhere in the session beats the active window, and \
             the active pane of its own"
        );
        assert_eq!(
            picked(
                &panes,
                &[("%3", State::Working), ("%4", State::Waiting)],
                None
            ),
            "%4",
            "waiting beats working"
        );
        assert_eq!(
            picked(
                &panes,
                &[("%3", State::Waiting), ("%4", State::Waiting)],
                None
            ),
            "%3",
            "a tie goes to the first window"
        );
        assert_eq!(
            picked(&panes, &[("%4", State::Waiting)], Some("@1")),
            "%2",
            "a chosen window keeps the preview"
        );
        assert_eq!(
            picked(&panes, &[("%1", State::Working)], Some("@0")),
            "%1",
            "and within it the agent's pane wins"
        );
        assert_eq!(
            picked(&panes, &[("%4", State::Waiting)], Some("@9")),
            "%4",
            "a chosen window that closed leaves it to the agents again"
        );
    }
}

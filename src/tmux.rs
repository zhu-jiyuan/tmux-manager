//! A thin wrapper over the `tmux` command line.
//!
//! Every call here is one subprocess (about 4 ms), so callers pack all the
//! fields they need into a single format string instead of asking twice.
//! Which server we talk to comes from the `TMUX` environment variable, exactly
//! as it would for a user typing `tmux` inside a session; outside tmux it is
//! the default server, which may not be running yet.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Result, bail};

/// Whether tm runs inside tmux, so a client is there to switch.
pub fn inside() -> bool {
    env::var_os("TMUX").is_some_and(|value| !value.is_empty())
}

/// tmux with `args`. `-u` because only inside tmux does it take the caller
/// for UTF-8 regardless of the locale; otherwise it may print tabs and
/// names in other scripts as `_`.
fn tmux(args: &[&str]) -> Command {
    let mut command = Command::new("tmux");
    command.arg("-u").args(args);
    command
}

/// Run tmux and return its stdout, or an error carrying tmux's stderr.
pub fn run(args: &[&str]) -> Result<String> {
    finish(args, tmux(args).output()?)
}

/// Like [`run`], for a listing: with no server running there is nothing to
/// list. Outside tmux none runs until the first session starts one.
pub fn list(args: &[&str]) -> Result<String> {
    let output = tmux(args).output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    // tmux's words for no socket, and for a socket nobody listens on.
    if !output.status.success()
        && (stderr.starts_with("error connecting to ") || stderr.starts_with("no server running"))
    {
        return Ok(String::new());
    }
    finish(args, output)
}

fn finish(args: &[&str], output: Output) -> Result<String> {
    if !output.status.success() {
        bail!(
            "tmux {}: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end_matches('\n')
        .to_string())
}

/// Like [`run`], but a failure (say, a session that just vanished) yields "".
pub fn run_lossy(args: &[&str]) -> String {
    run(args).unwrap_or_default()
}

pub struct Session {
    pub id: String,
    pub name: String,
    /// The directory it was started in.
    pub path: PathBuf,
}

/// Every session; none with no server.
pub fn sessions() -> Result<Vec<Session>> {
    Ok(list(&[
        "list-sessions",
        "-F",
        "#{session_id}\t#{session_name}\t#{session_path}",
    ])?
    .lines()
    .filter_map(|line| {
        let mut fields = line.splitn(3, '\t');
        Some(Session {
            id: fields.next()?.to_string(),
            name: fields.next()?.to_string(),
            path: PathBuf::from(fields.next()?),
        })
    })
    .collect())
}

/// A detached session named `name`, started in `dir` (else tm's own
/// directory), and the server with it if none runs; its id.
pub fn new_session(name: &str, dir: Option<&Path>) -> Result<String> {
    let dir = dir.map(|dir| dir.to_string_lossy());
    let mut args = vec!["new-session", "-d", "-s", name, "-P", "-F", "#{session_id}"];
    if let Some(dir) = &dir {
        args.extend(["-c", dir]);
    }
    run(&args)
}

/// One pane with everything the rows and the agent collector need about it,
/// its window and its session.
#[derive(Clone)]
pub struct Pane {
    pub session_id: String,
    pub session_name: String,
    pub session_attached: bool,
    /// The directory the session was started in.
    pub session_path: String,
    pub window_id: String,
    pub window_index: usize,
    pub window_name: String,
    pub window_active: bool,
    pub window_panes: usize,
    pub id: String,
    pub index: usize,
    /// The window's active pane.
    pub active: bool,
    pub pid: i32,
    pub dead: bool,
    pub tty: String,
    /// The foreground process has the default shell's name: usually the pane
    /// sits at its prompt. A shell running a command line (`new-window
    /// 'claude; exec zsh'`) looks the same while the command runs.
    pub at_prompt: bool,
    /// The record a harness's hook stored on the pane (`@tm-agent`), if any.
    pub record: Option<String>,
    /// The directory the pane's foreground process is in.
    pub path: String,
    /// The title its program set, tabs made spaces; empty while it is still
    /// tmux's default, the host name.
    pub title: String,
}

/// Everything a [`Pane`] holds, in one format string. Names come last
/// because they may contain anything, even tabs.
const PANE_FORMAT: &str = "#{session_id}\t#{session_attached}\t#{window_id}\t#{window_index}\t#{window_active}\t#{window_panes}\t\
     #{pane_id}\t#{pane_index}\t#{pane_active}\t#{pane_pid}\t#{pane_dead}\t#{pane_tty}\t#{==:#{pane_current_command},#{b:default-shell}}\t\
     #{@tm-agent}\t#{session_path}\t#{pane_current_path}\t\
     #{?#{==:#{pane_title},#{host}},,#{s/\t/ /:pane_title}}\t#{session_name}\t#{window_name}";

/// Every pane on the server in one call, sessions and windows in tmux's
/// order; none with no server.
pub fn panes() -> Result<Vec<Pane>> {
    parse_panes(&list(&["list-panes", "-a", "-F", PANE_FORMAT])?)
}

/// Every pane of the session `target` names (a window id names its
/// session), windows in index order.
pub fn session_panes(target: &str) -> Result<Vec<Pane>> {
    parse_panes(&run(&[
        "list-panes",
        "-s",
        "-t",
        target,
        "-F",
        PANE_FORMAT,
    ])?)
}

fn parse_panes(text: &str) -> Result<Vec<Pane>> {
    text.lines()
        .map(|line| {
            let [
                session_id,
                attached,
                window_id,
                window_index,
                window_active,
                window_panes,
                id,
                index,
                active,
                pid,
                dead,
                tty,
                at_prompt,
                record,
                session_path,
                path,
                title,
                session_name,
                window_name,
            ] = line.splitn(19, '\t').collect::<Vec<_>>()[..]
            else {
                bail!("list-panes: {line}");
            };
            Ok(Pane {
                session_id: session_id.to_string(),
                session_name: session_name.to_string(),
                session_attached: attached != "0",
                session_path: session_path.to_string(),
                window_id: window_id.to_string(),
                window_index: window_index.parse()?,
                window_name: window_name.to_string(),
                window_active: window_active == "1",
                window_panes: window_panes.parse()?,
                id: id.to_string(),
                index: index.parse()?,
                active: active == "1",
                pid: pid.parse()?,
                dead: dead == "1",
                tty: tty.to_string(),
                at_prompt: at_prompt == "1",
                record: (!record.is_empty()).then(|| record.to_string()),
                path: path.to_string(),
                title: title.to_string(),
            })
        })
        .collect()
}

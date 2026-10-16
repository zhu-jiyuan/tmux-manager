//! Agent activity: one state per tmux window.
//!
//! Two sources feed it. The harnesses' lifecycle hooks call `tm hook`, which
//! stores a small JSON record on the pane as a tmux user option. Without
//! hooks, or when the record is stale, the last screen lines say whether the
//! agent is waiting. What a hook event or a screen means belongs to the
//! harness that produced it (`harness.rs`); this file finds the processes,
//! keeps the records and merges the answers.
//!
//! Cost matters because the switcher refreshes every second. The collector
//! skips panes sitting at their shell prompt, runs one `ps` for the rest,
//! and only captures the screen of panes where it found an agent.

use std::cell::OnceCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::io::{self, Read};
use std::os::unix::process::parent_id;
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::harness::{self, Harness, Record};
use crate::tmux::{self, Pane};

/// The tmux pane option a hook stores its record in; `tmux::panes` reads it
/// back as `#{@tm-agent}`.
pub const OPTION: &str = "@tm-agent";

/// Ordered by priority: waiting beats working beats plain, so the strongest
/// state of a set of panes is simply the maximum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Plain,
    Working,
    Waiting,
}

/// Why an agent waits, when it says: the kinds of OSC 7501's `blocked`,
/// and the two ways a turn ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reason {
    /// A tool needs approval.
    Permission,
    /// It asked the user something.
    Question,
    /// The turn finished.
    Done,
    /// The turn failed, or a limit stopped it.
    Error,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Reason::Permission => "permission",
            Reason::Question => "question",
            Reason::Done => "done",
            Reason::Error => "error",
        }
    }
}

/// What an agent is doing and, if it is known, why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    pub reason: Option<Reason>,
}

impl Status {
    pub fn new(state: State, reason: Option<Reason>) -> Self {
        Status { state, reason }
    }
}

/// The agent in one pane: which harness runs it, and its status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Agent {
    pub harness: &'static str,
    pub status: Status,
}

struct Process {
    parent: i32,
    started: String,
    command: String,
}

type Table = HashMap<i32, Process>;

/// Only the processes attached to the given terminals (`ttys003`, `pts/2`):
/// everything running inside those panes. Far cheaper than the whole table,
/// because `ps -t` asks the kernel for just those.
fn processes_on(ttys: &[&str]) -> Result<Table> {
    if ttys.is_empty() {
        return Ok(Table::new());
    }
    let output = Command::new("ps")
        .args(["-t", &ttys.join(","), "-o", "pid=,ppid=,lstart=,command="])
        .output()
        .context("running ps")?;
    // ps exits non-zero when a terminal has no processes; the output is still fine.
    Ok(parse_ps(&String::from_utf8_lossy(&output.stdout)))
}

/// `/dev/ttys003` as `ps -t` wants it.
fn tty_name(tty: &str) -> &str {
    tty.strip_prefix("/dev/").unwrap_or(tty)
}

fn parse_ps(text: &str) -> Table {
    let mut table = Table::new();
    for line in text.lines() {
        // pid ppid <5 words of lstart> command...
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 8 {
            continue;
        }
        let (Ok(pid), Ok(parent)) = (fields[0].parse(), fields[1].parse()) else {
            continue;
        };
        table.insert(
            pid,
            Process {
                parent,
                started: fields[2..7].join(" "),
                command: fields[7..].join(" "),
            },
        );
    }
    table
}

/// `root` and everything below it.
fn descendants(root: i32, table: &Table) -> HashSet<i32> {
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for (pid, process) in table {
        children.entry(process.parent).or_default().push(*pid);
    }
    let mut found = HashSet::new();
    let mut pending = vec![root];
    while let Some(pid) = pending.pop() {
        if found.insert(pid)
            && let Some(kids) = children.get(&pid)
        {
            pending.extend(kids);
        }
    }
    found
}

/// The agent in one pane whose process tree starts at `root`, given the
/// record stored on it; none when no harness runs there. The record is live
/// when its pid runs a harness in the tree and its start time matches, so a
/// reused PID cannot resurrect a stale one; only the harness running that
/// pid gets it. Every harness found in the tree answers, and the strongest
/// state wins. The screen is captured at most once, and only if a harness
/// asks for it.
fn pane_agent(
    root: i32,
    record: Option<&str>,
    table: &Table,
    capture: impl Fn() -> String,
) -> Option<Agent> {
    let agents: HashMap<i32, &'static dyn Harness> = descendants(root, table)
        .into_iter()
        .filter_map(|pid| harness::detect(&table.get(&pid)?.command).map(|h| (pid, h)))
        .collect();
    let live = record
        .and_then(|text| serde_json::from_str::<Record>(text).ok())
        .filter(|saved| {
            agents.contains_key(&saved.pid)
                && table
                    .get(&saved.pid)
                    .is_some_and(|process| process.started == saved.started)
        });
    let cell = OnceCell::new();
    let screen = || cell.get_or_init(&capture).clone();
    harness::ALL
        .iter()
        .filter(|harness| agents.values().any(|agent| agent.name() == harness.name()))
        .map(|harness| {
            let own = live
                .as_ref()
                .filter(|saved| agents[&saved.pid].name() == harness.name());
            Agent {
                harness: harness.name(),
                status: harness.pane_state(own, &screen),
            }
        })
        .max_by_key(|agent| agent.status.state)
}

/// The agents in `panes`, keyed by pane id; a pane with none is left out. A
/// pane could hold an agent when something other than the shell is in front
/// or a hook record is on it; which harness, if any, is left to its command
/// line in `ps`, never guessed from a name. `TM_AGENT_SCAN=always` checks
/// every pane.
pub fn agents(panes: &[Pane]) -> Result<HashMap<String, Agent>> {
    let always = env::var("TM_AGENT_SCAN").is_ok_and(|v| v == "always");
    // A record catches the agents a shell runs from a command line, which
    // look like a pane at its prompt.
    let suspicious =
        |pane: &Pane| !pane.dead && (always || !pane.at_prompt || pane.record.is_some());
    let ttys: Vec<&str> = panes
        .iter()
        .filter(|pane| suspicious(pane))
        .map(|pane| tty_name(&pane.tty))
        .collect();
    let table = processes_on(&ttys)?;

    Ok(panes
        .iter()
        .filter(|pane| suspicious(pane))
        .filter_map(|pane| {
            let agent = pane_agent(pane.pid, pane.record.as_deref(), &table, || {
                // The visible pane plus 20 lines of history, so a short pane
                // still fills the tail the hints read.
                tmux::run_lossy(&["capture-pane", "-p", "-t", &pane.id, "-S", "-20"])
            })?;
            Some((pane.id.clone(), agent))
        })
        .collect())
}

/// One state per window of `panes`, keyed by window id: the strongest of
/// its panes' agents, plain where there is none.
pub fn by_window(panes: &[Pane], agents: &HashMap<String, Agent>) -> BTreeMap<String, State> {
    let mut windows = BTreeMap::new();
    for pane in panes {
        let state = agents
            .get(&pane.id)
            .map_or(State::Plain, |agent| agent.status.state);
        let window = windows
            .entry(pane.window_id.clone())
            .or_insert(State::Plain);
        *window = (*window).max(state);
    }
    windows
}

/// One state per window on the server, keyed by window id.
pub fn states(panes: &[Pane]) -> Result<BTreeMap<String, State>> {
    Ok(by_window(panes, &agents(panes)?))
}

/// Called by a harness's hook. Asks the harness what the call means, finds
/// the harness's process above us, and records the state on the pane.
/// Silent whenever it cannot: hooks must not disturb agents.
pub fn hook(name: &str, event: Option<&str>) -> Result<()> {
    let pane = env::var("TMUX_PANE").unwrap_or_default();
    if !pane.starts_with('%') || !tmux::inside() {
        return Ok(());
    }
    let Some(harness) = harness::by_name(name) else {
        return Ok(());
    };
    let mut stdin = String::new();
    if event.is_none() {
        io::stdin().read_to_string(&mut stdin)?;
    }
    let Some(update) = harness.hook(event, &stdin) else {
        return Ok(());
    };
    // Everything in this pane shares its terminal, so ask ps for just that.
    let tty = tmux::run(&["display-message", "-p", "-t", &pane, "#{pane_tty}"])?;
    let table = processes_on(&[tty_name(&tty)])?;
    let mut owner = None;
    let mut pid = parent_id() as i32;
    while let Some(process) = table.get(&pid) {
        if harness.runs(&process.command) {
            owner = Some((pid, process.started.clone()));
            break;
        }
        if process.parent == pid {
            break;
        }
        pid = process.parent;
    }
    let Some((pid, started)) = owner else {
        return Ok(());
    };
    let record = Record {
        state: update.state,
        reason: update.reason,
        pid,
        started,
        harness: name.to_string(),
        event: update.event,
    };
    tmux::run(&[
        "set-option",
        "-p",
        "-t",
        &pane,
        OPTION,
        &serde_json::to_string(&record)?,
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state_of(root: i32, record: Option<&str>, table: &Table, screen: fn() -> String) -> State {
        pane_agent(root, record, table, screen).map_or(State::Plain, |agent| agent.status.state)
    }

    #[test]
    fn stale_records_fall_back_to_the_screen() {
        let table = |command: &str| {
            let mut t = Table::new();
            t.insert(
                10,
                Process {
                    parent: 1,
                    started: "new".into(),
                    command: command.into(),
                },
            );
            t
        };
        let stale = json!({"pid": 10, "started": "old", "state": "working", "harness": "codex", "event": "x"}).to_string();
        assert_eq!(
            state_of(10, Some(&stale), &table("codex"), || "ready".into()),
            State::Waiting
        );
        assert_eq!(
            state_of(10, Some(&stale), &table("bash"), || {
                "esc to interrupt".into()
            }),
            State::Plain,
            "no harness process, no state"
        );
        let fresh = json!({"pid": 10, "started": "new", "state": "working", "harness": "codex", "event": "x"}).to_string();
        assert_eq!(
            state_of(10, Some(&fresh), &table("codex"), || "ready".into()),
            State::Working
        );
        assert_eq!(
            state_of(10, None, &table("codex"), || "ready".into()),
            State::Waiting,
            "no record: the harness reads the screen"
        );
    }

    #[test]
    fn ps_output_parses() {
        let table = parse_ps(
            "  123     1 Fri Sep  9 21:47:00 2026 /bin/zsh -l\n 99 123 Fri Sep  9 21:48:00 2026 codex\nbad line\n",
        );
        assert_eq!(table[&123].started, "Fri Sep 9 21:47:00 2026");
        assert_eq!(table[&99].parent, 123);
        assert_eq!(descendants(123, &table), HashSet::from([123, 99]));
    }
}

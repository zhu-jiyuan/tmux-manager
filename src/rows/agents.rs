//! Agents mode: a row per pane where an agent works or waits, the ones
//! waiting for the user first and tmux's order within each state; stars do
//! not reorder them.
//!
//! Each row takes two lines, and fzf moves, matches and picks it whole. The
//! first is the dot and the session's title, or the directory when the
//! agent set no title; the second, faint, is the harness, `session:window`
//! (`.pane` in a split window), the directory when the title took the first
//! line, and why it waits and for how long, as far as the hooks tell.

use std::cmp::Reverse;
use std::path::Path;

use super::{Data, Row, RowId, dots};
use crate::agent::{self, Agent, State};
use crate::ansi::{bold, faint};
use crate::harness;
use crate::projects;
use crate::tmux::Pane;

pub(super) fn rows(data: &Data) -> Vec<Row> {
    let mut found: Vec<_> = data
        .panes
        .iter()
        .filter_map(|pane| Some((pane, data.agents.get(&pane.id)?)))
        .filter(|(_, agent)| agent.status.state > State::Plain)
        .collect();
    // A stable sort, so tmux's order holds within each state.
    found.sort_by_key(|(_, agent)| Reverse(agent.status.state));
    found
        .into_iter()
        .map(|(pane, agent)| {
            let shown = format!(
                "{} {}\n  {}",
                dots(&[agent.status.state]),
                bold(&headline(pane, agent)),
                faint(&details(pane, agent, data.now).join(" · "))
            );
            Row {
                id: RowId::Pane(pane.id.clone()),
                name: pane.session_name.clone(),
                tree: shown.clone(),
                breadcrumb: shown,
                dots: String::new(),
                badge: String::new(),
            }
        })
        .collect()
}

/// The session's title as its harness reads the pane's, unless all it says
/// is the directory's name (Codex titles its pane so).
fn title(pane: &Pane, agent: &Agent) -> Option<String> {
    let title = harness::by_name(agent.harness)?.title(&pane.title)?;
    let directory = Path::new(&pane.path).file_name()?.to_string_lossy();
    (title != directory).then_some(title)
}

fn directory(pane: &Pane) -> String {
    projects::shorten(Path::new(&pane.path))
}

/// The first line: the title, else the directory.
fn headline(pane: &Pane, agent: &Agent) -> String {
    title(pane, agent).unwrap_or_else(|| directory(pane))
}

/// The second line's parts: harness, where, the directory if the first line
/// is a title, then why and since when.
pub(crate) fn details(pane: &Pane, agent: &Agent, now: u64) -> Vec<String> {
    let mut location = format!("{}:{}", pane.session_name, pane.window_index);
    if pane.window_panes > 1 {
        location.push_str(&format!(".{}", pane.index));
    }
    let mut parts = vec![agent.harness.to_string(), location];
    if title(pane, agent).is_some() {
        parts.push(directory(pane));
    }
    let status = agent.status;
    let when = [
        status.reason.map(|reason| reason.label().to_string()),
        status.since.map(|since| agent::age(since, now)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    if !when.is_empty() {
        parts.push(when);
    }
    parts
}

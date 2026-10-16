//! Agents mode: a row per pane where an agent works or waits, the ones
//! waiting for the user first and tmux's order within each state; stars do
//! not reorder them. The name column, all of it searchable, packs the
//! harness, where the pane is (`session:window`, `.pane` when the window is
//! split), its window's name and its directory; the dot and why the agent
//! waits follow, as in the other lists.

use std::cmp::Reverse;
use std::path::Path;

use super::{Data, Row, RowId, badge, dots, star};
use crate::agent::State;
use crate::ansi::{clip_line, faint, pad, visible_width};
use crate::projects;

/// The most cells a window name and a directory take; the list has half a
/// popup to share with the dot and the reason.
const WINDOW_WIDTH: usize = 16;
const DIRECTORY_WIDTH: usize = 24;

pub(super) fn rows(data: &Data) -> Vec<Row> {
    let mut found: Vec<_> = data
        .panes
        .iter()
        .filter_map(|pane| Some((pane, data.agents.get(&pane.id)?)))
        .filter(|(_, agent)| agent.status.state > State::Plain)
        .collect();
    // A stable sort, so tmux's order holds within each state.
    found.sort_by_key(|(_, agent)| Reverse(agent.status.state));

    let cells: Vec<[String; 4]> = found
        .iter()
        .map(|(pane, agent)| {
            let mut location = format!("{}:{}", pane.session_name, pane.window_index);
            if pane.window_panes > 1 {
                location.push_str(&format!(".{}", pane.index));
            }
            [
                agent.harness.to_string(),
                location,
                head(&pane.window_name, WINDOW_WIDTH),
                tail(&projects::shorten(Path::new(&pane.path)), DIRECTORY_WIDTH),
            ]
        })
        .collect();
    let width = |column: usize| {
        cells
            .iter()
            .map(|cell| visible_width(&cell[column]))
            .max()
            .unwrap_or(0)
    };
    let widths = [width(0), width(1), width(2)];

    found
        .iter()
        .zip(cells)
        .map(|((pane, agent), [harness, location, window, directory])| {
            let shown = format!(
                "{}  {}  {}  {}",
                pad(&harness, widths[0]),
                pad(&location, widths[1]),
                pad(&window, widths[2]),
                faint(&directory)
            );
            let starred = data.favorites.contains(&pane.session_name);
            Row {
                id: RowId::Pane(pane.id.clone()),
                name: pane.session_name.clone(),
                tree: star(starred, &shown),
                breadcrumb: star(starred, &shown),
                dots: dots(&[agent.status.state]),
                badge: agent
                    .status
                    .reason
                    .map_or_else(String::new, |reason| badge(reason.label())),
            }
        })
        .collect()
}

/// `text` in at most `width` cells, an ellipsis marking the cut.
fn head(text: &str, width: usize) -> String {
    if visible_width(text) <= width {
        return text.to_string();
    }
    format!("{}…", clip_line(text, width.saturating_sub(1)))
}

/// The end of `text` in at most `width` cells, an ellipsis marking the cut:
/// what is left of a deep directory is the part that names it.
fn tail(text: &str, width: usize) -> String {
    if visible_width(text) <= width {
        return text.to_string();
    }
    let mut kept = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        used += visible_width(c.encode_utf8(&mut [0; 4]));
        if used > width {
            break;
        }
        kept.push(c);
    }
    format!("…{}", kept.iter().rev().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_names_keep_the_end_that_names_them() {
        assert_eq!(head("claude", 16), "claude");
        assert_eq!(head("a-very-long-window", 8), "a-very-…");
        assert_eq!(tail("~/w/tm", 24), "~/w/tm");
        assert_eq!(tail("~/workspace/tmux-manager", 13), "…tmux-manager");
        assert_eq!(tail("~/项目/工具", 6), "…/工具");
    }
}

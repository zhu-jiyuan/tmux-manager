//! Projects mode: every directory under `@tm-projects`. One with a session
//! started in it is that session's row, dots and all; one without carries
//! its path as the id, which Enter turns into a session. Open ones come
//! before the rest, each in name order, and `build` puts starred ones first.

use std::collections::HashMap;
use std::path::Path;

use super::{Data, Group, Row, RowId, badge, count, star};

pub(super) fn rows(groups: &[Group], data: &Data) -> Vec<Row> {
    // A session belongs to the project it was started in, the rule
    // `projects::open` follows too, so Enter on the row lands in this very
    // session. One that only shares the project's name lives elsewhere.
    let mut by_dir: HashMap<&Path, &Group> = HashMap::new();
    for group in groups {
        if let Some(dir) = data.session_dirs.get(&group.session.session_id) {
            by_dir.entry(dir.as_path()).or_insert(group);
        }
    }

    let mut rows: Vec<Row> = data
        .projects
        .iter()
        .map(|project| {
            let place = crate::projects::shorten(project.path.parent().unwrap_or(&project.path));
            match by_dir.get(project.path.as_path()) {
                Some(group) => {
                    let badge = format!(
                        "{place} · {}{}",
                        count(group.windows.len(), "window"),
                        group.attached()
                    );
                    group.row(data, &group.session.session_name, &badge)
                }
                None => {
                    // Named after the session Enter will make, so a session
                    // elsewhere that has the directory's name keeps its star.
                    let name = crate::projects::free_name(&project.path, |name| {
                        groups
                            .iter()
                            .any(|group| group.session.session_name == name)
                    });
                    let starred = data.favorites.contains(&name);
                    Row {
                        id: RowId::Project(project.path.clone()),
                        tree: star(starred, &name),
                        breadcrumb: star(starred, &name),
                        name,
                        dots: String::new(),
                        badge: badge(&place),
                    }
                }
            }
        })
        .collect();
    rows.sort_by_key(|row| !matches!(row.id, RowId::Session(_)));
    rows
}

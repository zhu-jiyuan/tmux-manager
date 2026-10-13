//! One open popup: the row snapshot fzf is showing and the state kept
//! beside it.
//!
//! The picker (`switch`) creates the snapshot and exports its path as
//! `TM_SNAPSHOT`, so every subcommand fzf runs finds the same popup through
//! [`Popup::current`]. Small bits of state live in *sidecars*: files next to
//! the snapshot, one per [`Sidecar`] variant, so they survive each subcommand
//! being a fresh process and vanish with the popup. Nothing else on disk is
//! per popup.
//!
//! Every piece of this state has an owner and goes with it. A popup's files
//! belong to its picker process, whose pid names them: `cleanup` removes
//! them when it exits, and the next popup's [`Popup::create`] when it was
//! killed. Run from a shell, with no `TM_SNAPSHOT`, a subcommand is the
//! first key of a popup of its own that lives in memory as long as the
//! process: it starts empty, and nothing reaches the disk. To drive several
//! keys against one popup (prompt, then enter), name a snapshot you own in
//! `TM_SNAPSHOT`, as the integration tests do.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::paths;

/// What the list shows. `tab` cycles through them in this order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Sessions,
    Windows,
    Projects,
}

impl Mode {
    /// The prompt names the mode, since tmux cannot retitle an open popup.
    pub fn prompt(self) -> &'static str {
        match self {
            Mode::Sessions => "sessions> ",
            Mode::Windows => "windows> ",
            Mode::Projects => "projects> ",
        }
    }

    pub fn next(self) -> Mode {
        match self {
            Mode::Sessions => Mode::Windows,
            Mode::Windows => Mode::Projects,
            Mode::Projects => Mode::Sessions,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Mode::Sessions => "sessions",
            Mode::Windows => "windows",
            Mode::Projects => "projects",
        }
    }

    fn parse(text: &str) -> Mode {
        match text.trim() {
            "windows" => Mode::Windows,
            "projects" => Mode::Projects,
            _ => Mode::Sessions,
        }
    }
}

/// The kinds of state a popup keeps beside its snapshot.
#[derive(Clone, Copy)]
pub enum Sidecar {
    /// The mode, by name; absent means sessions.
    Mode,
    /// The open inline prompt, as JSON (`actions::Pending`).
    Prompt,
    /// Each session's chosen preview window, as JSON (`preview`).
    Preview,
    /// The fzf action to replay after the full-screen preview.
    View,
    /// Present while the key legend is hidden.
    Help,
    /// The `@tm-projects` value as the popup opened (`projects::capture`).
    Projects,
}

impl Sidecar {
    /// The extension that tells the files apart.
    fn extension(self) -> &'static str {
        match self {
            Sidecar::Mode => "mode",
            Sidecar::Prompt => "prompt",
            Sidecar::Preview => "preview",
            Sidecar::View => "view",
            Sidecar::Help => "help",
            Sidecar::Projects => "projects",
        }
    }
}

/// Where a popup keeps its snapshot and sidecars.
enum Store {
    /// Files named after the snapshot: a popup the picker opened, or one whose
    /// snapshot the caller named in `TM_SNAPSHOT`, and so owns.
    Files(PathBuf),
    /// Sidecars by extension, in this process's memory: a run from a shell.
    /// It has no snapshot, so no rows on show.
    Memory(RefCell<BTreeMap<&'static str, String>>),
}

pub struct Popup {
    store: Store,
}

impl Popup {
    /// The popup whose fzf runs us, from `TM_SNAPSHOT`; from a shell, a
    /// fresh one that lives as long as this process and never touches the
    /// disk.
    pub fn current() -> Popup {
        match paths::snapshot() {
            Some(snapshot) => Popup::at(snapshot),
            None => Popup::memory(),
        }
    }

    fn at(snapshot: impl Into<PathBuf>) -> Popup {
        Popup {
            store: Store::Files(snapshot.into()),
        }
    }

    fn memory() -> Popup {
        Popup {
            store: Store::Memory(RefCell::default()),
        }
    }

    /// A popup for this process, after sweeping what popups that were
    /// killed rather than closed left behind.
    pub fn create() -> Result<Popup> {
        let run = paths::run_dir()?;
        let me = process::id();
        remove_stale(&run, me);
        Ok(Popup::at(run.join(format!("switch-{me}.txt"))))
    }

    /// The snapshot fzf reads; `None` for a popup in memory, which has none.
    pub fn snapshot(&self) -> Option<&Path> {
        match &self.store {
            Store::Files(snapshot) => Some(snapshot),
            Store::Memory(_) => None,
        }
    }

    /// Whether rows are on show: never for a popup in memory.
    pub fn exists(&self) -> bool {
        self.snapshot().is_some_and(Path::exists)
    }

    pub fn read_snapshot(&self) -> Result<String> {
        let snapshot = self.snapshot().context("no rows on show")?;
        fs::read_to_string(snapshot).with_context(|| format!("reading {}", snapshot.display()))
    }

    pub fn write_snapshot(&self, text: &str) -> Result<()> {
        paths::write_atomic(self.snapshot().context("no rows on show")?, text)
    }

    fn path(snapshot: &Path, sidecar: Sidecar) -> PathBuf {
        snapshot.with_extension(sidecar.extension())
    }

    pub fn has(&self, sidecar: Sidecar) -> bool {
        match &self.store {
            Store::Files(snapshot) => Popup::path(snapshot, sidecar).exists(),
            Store::Memory(memory) => memory.borrow().contains_key(sidecar.extension()),
        }
    }

    pub fn read(&self, sidecar: Sidecar) -> Option<String> {
        match &self.store {
            Store::Files(snapshot) => fs::read_to_string(Popup::path(snapshot, sidecar)).ok(),
            Store::Memory(memory) => memory.borrow().get(sidecar.extension()).cloned(),
        }
    }

    pub fn write(&self, sidecar: Sidecar, text: &str) -> Result<()> {
        match &self.store {
            Store::Files(snapshot) => paths::write_atomic(&Popup::path(snapshot, sidecar), text),
            Store::Memory(memory) => {
                memory
                    .borrow_mut()
                    .insert(sidecar.extension(), text.to_string());
                Ok(())
            }
        }
    }

    /// Gone afterwards, whether or not it was there.
    pub fn remove(&self, sidecar: Sidecar) -> Result<()> {
        match &self.store {
            Store::Files(snapshot) => match fs::remove_file(Popup::path(snapshot, sidecar)) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.into()),
                _ => Ok(()),
            },
            Store::Memory(memory) => {
                memory.borrow_mut().remove(sidecar.extension());
                Ok(())
            }
        }
    }

    /// Read and remove: for state that is consumed once.
    pub fn take(&self, sidecar: Sidecar) -> Result<Option<String>> {
        let text = self.read(sidecar);
        if text.is_some() {
            self.remove(sidecar)?;
        }
        Ok(text)
    }

    pub fn read_json<T: DeserializeOwned>(&self, sidecar: Sidecar) -> Result<Option<T>> {
        self.read(sidecar)
            .map(|text| serde_json::from_str(&text))
            .transpose()
            .with_context(|| match &self.store {
                Store::Files(snapshot) => {
                    format!("parsing {}", Popup::path(snapshot, sidecar).display())
                }
                Store::Memory(_) => format!("parsing the {} sidecar", sidecar.extension()),
            })
    }

    pub fn write_json<T: Serialize>(&self, sidecar: Sidecar, value: &T) -> Result<()> {
        self.write(sidecar, &serde_json::to_string(value)?)
    }

    pub fn mode(&self) -> Mode {
        self.read(Sidecar::Mode)
            .map(|text| Mode::parse(&text))
            .unwrap_or(Mode::Sessions)
    }

    pub fn set_mode(&self, mode: Mode) -> Result<()> {
        if mode == Mode::Sessions {
            self.remove(Sidecar::Mode)
        } else {
            self.write(Sidecar::Mode, mode.name())
        }
    }

    /// Everything this popup left on disk: every file named after the
    /// snapshot (`switch-<pid>.*`), so a new sidecar needs no list here. A
    /// popup in memory has nothing there.
    pub fn cleanup(&self) {
        let Some(snapshot) = self.snapshot() else {
            return;
        };
        let (Some(dir), Some(stem)) = (snapshot.parent(), snapshot.file_stem()) else {
            return;
        };
        let prefix = format!("{}.", stem.to_string_lossy());
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// The pid of the picker that owns a file in `run`, from its name:
/// `switch-<pid>.<anything>` (the snapshot, its sidecars, and the temporary
/// files of `write_atomic`).
fn owner(name: &str) -> Option<u32> {
    let (pid, _) = name.strip_prefix("switch-")?.split_once('.')?;
    pid.parse().ok().filter(|pid| *pid > 0)
}

/// Whether a process with this pid runs; signal 0 checks without touching it.
/// `run` is per user, so a pid we may not signal is not one of our popups:
/// it died, and another user's process has its number now.
fn alive(pid: u32) -> bool {
    libc::pid_t::try_from(pid).is_ok_and(|pid| unsafe { libc::kill(pid, 0) } == 0)
}

/// Keep only the files of popups that are open: each file whose owner is
/// dead goes, sidecars without a snapshot included, and so does any other
/// file in `run` not named after a picker's pid (tm makes no directories
/// there, and leaves any it finds). `me` is the popup being created, so
/// whatever a dead process with the same pid left cannot become its state.
fn remove_stale(run: &Path, me: u32) {
    let Ok(entries) = fs::read_dir(run) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let open = owner(&name.to_string_lossy()).is_some_and(|pid| pid != me && alive(pid));
        if !open {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system's temp dir, removed by the caller.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tm-{tag}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_popup_in_memory_keeps_state_for_its_process_only() {
        let popup = Popup::memory();
        assert!(popup.snapshot().is_none() && !popup.exists());
        assert_eq!(popup.mode(), Mode::Sessions, "it starts empty");
        popup.set_mode(Mode::Projects).unwrap();
        assert_eq!(
            popup.mode(),
            Mode::Projects,
            "read back in the same process"
        );
        popup.write(Sidecar::View, "accept").unwrap();
        assert_eq!(
            popup.take(Sidecar::View).unwrap().as_deref(),
            Some("accept")
        );
        assert!(!popup.has(Sidecar::View), "taken once");
        assert!(popup.write_snapshot("rows").is_err(), "no rows on show");
        popup.cleanup();
        assert!(!Popup::memory().has(Sidecar::Mode), "another starts empty");
    }

    #[test]
    fn the_sweep_keeps_only_the_files_of_open_popups() {
        let run = scratch("sweep");
        let me = process::id();
        // cargo, which runs this test, outlives it.
        let open = std::os::unix::process::parent_id();
        // Beyond any pid_max: never a running process.
        let dead = 2_147_483_000u32;
        let kept = [
            format!("switch-{open}.mode"),
            format!("switch-{open}.tmp{dead}"),
            format!("switch-{open}.txt"),
        ];
        let swept = [
            format!("switch-{dead}.txt"),
            format!("switch-{dead}.prompt"),
            // Killed before the snapshot was written.
            format!("switch-{}.mode", dead + 1),
            // A dead process with the new popup's pid left a prompt.
            format!("switch-{me}.prompt"),
            "cli.txt".to_string(),
            "cli.mode".to_string(),
            "switch-0.txt".to_string(),
            "switch-x.txt".to_string(),
        ];
        for name in kept.iter().chain(&swept) {
            fs::write(run.join(name), "").unwrap();
        }
        remove_stale(&run, me);
        let left = names(&run);
        let _ = fs::remove_dir_all(&run);
        assert_eq!(left, kept);
    }

    #[test]
    fn cleanup_takes_only_its_own_files() {
        let run = scratch("cleanup");
        for name in [
            "switch-7.txt",
            "switch-7.prompt",
            "switch-7.tmp9",
            "switch-70.txt",
        ] {
            fs::write(run.join(name), "").unwrap();
        }
        Popup::at(run.join("switch-7.txt")).cleanup();
        let left = names(&run);
        let _ = fs::remove_dir_all(&run);
        assert_eq!(left, ["switch-70.txt"]);
    }
}

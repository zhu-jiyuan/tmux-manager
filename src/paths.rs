//! Where tmm keeps things on disk.
//!
//! Persistent state (favorites) lives in the state directory. Per-popup
//! scratch files (the row snapshot fzf is showing and its sidecars, see
//! `popup`) live in a `run` subdirectory, named after the pid of the
//! `switch` that owns them, and go when it does.

use std::path::{Path, PathBuf};
use std::{env, fs, process};

use anyhow::{Context, Result};

pub fn home() -> PathBuf {
    env::home_dir().expect("a home directory")
}

/// `$TMM_STATE_DIR`, else `$XDG_STATE_HOME/tmm`, else `~/.local/state/tmm`.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = env::var_os("TMM_STATE_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(xdg) = env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(xdg).join("tmm");
    }
    home().join(".local/state/tmm")
}

pub fn run_dir() -> Result<PathBuf> {
    let dir = state_dir().join("run");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

pub fn switcher_file() -> PathBuf {
    state_dir().join("switcher.json")
}

/// The row snapshot of the popup whose fzf runs us. `switch` exports it so
/// the commands fzf runs share small bits of state through sidecars. `None`
/// from a shell (an empty value counts as unset): see `Popup::current`.
pub fn snapshot() -> Option<PathBuf> {
    env::var_os("TMM_SNAPSHOT")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// Write via a temporary file and rename, so readers never see a half-written file.
pub fn write_atomic(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp{}", process::id()));
    fs::write(&temporary, content).with_context(|| format!("writing {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

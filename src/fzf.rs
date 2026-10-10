//! Locating fzf and quoting the commands it runs.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

/// `bg-transform`, `every()`, `--footer`, `--gutter`, `--id-nth`.
const MIN_VERSION: (u32, u32, u32) = (0, 74, 3);

/// fzf on `PATH`, else in the usual Homebrew places (tmux's own PATH can be short).
pub fn locate() -> Option<PathBuf> {
    let on_path = env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>());
    on_path
        .map(|dir| dir.join("fzf"))
        .chain(["/opt/homebrew/bin/fzf", "/usr/local/bin/fzf"].map(PathBuf::from))
        .find(|candidate| candidate.is_file())
}

pub fn check_version(fzf: &Path) -> Result<()> {
    let output = Command::new(fzf).arg("--version").output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.split_whitespace().next().unwrap_or("");
    let parts: Vec<u32> = first
        .split('.')
        .take(3)
        .filter_map(|part| part.parse().ok())
        .collect();
    let version = match parts[..] {
        [major, minor, patch] => (major, minor, patch),
        _ => bail!("cannot read fzf version from {first:?}"),
    };
    if version < MIN_VERSION {
        let (major, minor, patch) = MIN_VERSION;
        bail!("update fzf to {major}.{minor}.{patch}+ (found {first})");
    }
    Ok(())
}

/// Single-quote for `/bin/sh`, the shell fzf runs our commands with.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// This binary's path, quoted, for commands that call back into tm: the
/// ones fzf runs on each key, and the agents' hooks.
pub fn me() -> String {
    quote(&env::current_exe().expect("own path").to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quoting() {
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), "'it'\\''s'");
    }
}

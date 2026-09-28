//! Persistent preferences: which sessions are starred.

use std::collections::BTreeSet;
use std::{fs, io};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Default, PartialEq, Serialize, Deserialize)]
pub struct Switcher {
    /// Session names. A `BTreeSet` keeps the file sorted and diff-friendly.
    pub favorites: BTreeSet<String>,
}

impl Switcher {
    /// A missing file means "no favorites yet". Anything else that goes
    /// wrong is an error, so a save never replaces stars it could not read.
    pub fn load() -> Result<Self> {
        let path = paths::switcher_file();
        match fs::read_to_string(&path) {
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Switcher::default()),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let text = serde_json::to_string_pretty(self)? + "\n";
        paths::write_atomic(&paths::switcher_file(), &text)
    }
}

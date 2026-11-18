//! What the desktop app last said about itself, for the CLI to read.
//!
//! The app and the CLI are separate binaries that share `~/.taurus` and nothing
//! else: neither starts the other, and neither can ask the other its version.
//! But they read and write the same config, so a CLI two releases behind the
//! app it sits beside is not just missing features, it may misread a file the
//! newer app wrote. Nothing said so. This file is how it can.
//!
//! The app writes it on every launch of an installed copy (a dev build says
//! nothing, since its version is whatever the clone happens to be), and again
//! whenever it puts the `taurus` command on the PATH. `taurus doctor` reads it
//! and compares.
//!
//! It is a record, not configuration: nobody is meant to edit it, a missing
//! or unreadable one means only "no app has said anything", and losing it
//! costs one warning until the app next starts.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config;

/// The file, under [`config::home_dir`].
pub const FILE_NAME: &str = "desktop.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The app's version when it last ran.
    pub version: String,
    /// The CLI inside the app's bundle, when the bundle has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundled_cli: Option<PathBuf>,
    /// The `taurus` the app put on the PATH, when it has put one there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<Installed>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    /// Where the command is: `/usr/local/bin/taurus`, say.
    pub path: PathBuf,
    pub kind: InstallKind,
    /// For a copy, the app version it was copied from. A link has no version
    /// of its own: it is always the bundle's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    /// A symlink into the bundle, which an update replaces underneath it.
    Link,
    /// A copy, made where a link would not survive: on Windows, where links
    /// need developer mode, and from an AppImage, whose path changes on every
    /// launch. The app copies it again when its version moves.
    Copy,
}

pub fn path() -> PathBuf {
    config::home_dir().join(FILE_NAME)
}

/// The record, or `None` when there is none or it can't be read.
pub fn load() -> Option<Record> {
    load_from(&path())
}

pub fn load_from(path: &Path) -> Option<Record> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(record: &Record) -> Result<(), String> {
    save_to(&path(), record)
}

pub fn save_to(path: &Path, record: &Record) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("couldn't create {}: {e}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
    text.push('\n');
    config::replace_file(path, &text).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_survives_the_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE_NAME);
        let record = Record {
            version: "0.4.1".into(),
            bundled_cli: Some("/Applications/Taurus.app/Contents/MacOS/taurus-cli".into()),
            installed: Some(Installed {
                path: "/usr/local/bin/taurus".into(),
                kind: InstallKind::Link,
                version: None,
            }),
        };
        save_to(&file, &record).unwrap();
        assert_eq!(load_from(&file), Some(record));
    }

    #[test]
    fn a_missing_or_torn_record_is_no_record() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE_NAME);
        assert_eq!(load_from(&file), None);
        std::fs::write(&file, "{ not json").unwrap();
        assert_eq!(load_from(&file), None);
    }
}

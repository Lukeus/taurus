//! Putting the CLI the app ships on the PATH, and keeping it current.
//!
//! # Why the app carries a CLI at all
//!
//! They are separate binaries that share `~/.taurus`. Before this, a release
//! carried only the app, and the CLI came from `cargo install` in a clone, so
//! it stayed whatever version it was built at while the app updated itself
//! underneath it, both reading and writing the same files. A release build now
//! bundles `taurus-cli` beside the app's own executable (see
//! `tauri.bundle-cli.conf.json` and `scripts/cli-sidecar.mjs`), so every app
//! update carries the CLI that matches it.
//!
//! # Why it's named `taurus-cli` inside the bundle
//!
//! `tauri-build` copies a sidecar into cargo's output folder, where `taurus`
//! is the CLI's own build, and on Windows the app's executable is `Taurus.exe`,
//! the same name to a case-insensitive filesystem. So the bundle's copy is
//! `taurus-cli`, and the command this puts on the PATH is `taurus`.
//!
//! # Link where it survives an update, copy where it doesn't
//!
//! - **macOS:** a link at `/usr/local/bin/taurus` into the `.app`. An update
//!   replaces the `.app` in place, so the link points at the new CLI with
//!   nothing done. `/usr/local/bin` is root's on most Macs, so this can ask
//!   for an administrator password, the way an editor's "install command" does.
//! - **Linux, `.deb` or `.rpm`:** a link at `~/.local/bin/taurus` to the
//!   package's `/usr/bin/taurus-cli`. No password.
//! - **Linux, AppImage:** a copy at `~/.local/bin/taurus`. An AppImage mounts
//!   somewhere new on every launch, so a link into it breaks when it exits.
//! - **Windows:** a copy at `%LOCALAPPDATA%\Taurus\bin\taurus.exe`, with that
//!   folder added to the user's PATH. Windows links need developer mode, and
//!   a folder holding `Taurus.exe` can't go on the PATH, because `taurus`
//!   would start the app.
//!
//! A copy goes stale when the app updates, so [`on_launch`] copies it again
//! whenever the app's version has moved. A user who removed it stays removed.
//!
//! Nothing here runs unless someone presses the button, except that refresh,
//! and it only refreshes something the button made.

use std::path::{Path, PathBuf};

use serde::Serialize;
use taurus_host::desktop::{self, InstallKind, Installed, Record};
use ts_rs::TS;

/// The bundle's copy, without its extension.
pub const SIDECAR: &str = "taurus-cli";
/// What it's called on the PATH.
pub const COMMAND: &str = "taurus";

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// The CLI in this app's bundle, when it has one.
///
/// Beside the executable on every platform: that's where the bundler puts an
/// `externalBin`. A dev build has one only when it was built with the bundle
/// config, and even then it's in `target/`, which is no place to link to.
pub fn bundled() -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    let candidate = exe_path.parent()?.join(exe(SIDECAR));
    candidate.is_file().then_some(candidate)
}

/// What Settings shows.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct CliStatus {
    /// The app's version, which is the bundled CLI's.
    pub version: String,
    /// Where the command would go, or is.
    #[ts(optional)]
    pub target: Option<String>,
    /// Whether the app has put the command there and it's still there.
    pub installed: bool,
    /// Whether installing needs an administrator's password.
    pub needs_admin: bool,
    /// The folder [`Self::target`] is in isn't on the PATH, so the command
    /// can't be found by name until it is.
    #[ts(optional)]
    pub not_on_path: Option<String>,
    /// A different `taurus` that a terminal finds first.
    #[ts(optional)]
    pub shadowed_by: Option<String>,
    /// Why the button can't do anything here.
    #[ts(optional)]
    pub cannot_install: Option<String>,
}

/// Where the command goes, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub dest: PathBuf,
    pub kind: InstallKind,
    /// The folder needs an administrator to write to.
    pub admin: bool,
}

/// What a platform needs to decide a [`Plan`], gathered so the decision can be
/// tested without the platform.
#[derive(Debug, Default)]
pub struct Env {
    pub home: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    /// Set by an AppImage's runtime to the image's own path.
    pub appimage: bool,
}

impl Env {
    pub fn current() -> Self {
        let var = |name| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Self {
            home: var("HOME").or_else(|| var("USERPROFILE")),
            local_app_data: var("LOCALAPPDATA"),
            appimage: var("APPIMAGE").is_some(),
        }
    }
}

/// Where this platform puts the command, for a bundle at `bundled`.
pub fn plan(bundled: &Path, env: &Env) -> Result<Plan, String> {
    if cfg!(target_os = "macos") {
        let at = bundled.to_string_lossy();
        // Gatekeeper runs a quarantined app from a randomized read-only copy
        // until it's moved, and a disk image is gone when it's ejected. A link
        // to either breaks within the hour.
        if at.contains("/AppTranslocation/") || at.starts_with("/Volumes/") {
            return Err(
                "Move Taurus to your Applications folder and open it from there first. \
                 It's running from a temporary location that a link can't point at."
                    .into(),
            );
        }
        let dir = PathBuf::from("/usr/local/bin");
        let admin = !writable(&dir);
        Ok(Plan {
            dest: dir.join(COMMAND),
            kind: InstallKind::Link,
            admin,
        })
    } else if cfg!(windows) {
        let base = env
            .local_app_data
            .clone()
            .ok_or("LOCALAPPDATA isn't set, so there's nowhere to put it")?;
        Ok(Plan {
            dest: base.join("Taurus").join("bin").join(exe(COMMAND)),
            kind: InstallKind::Copy,
            admin: false,
        })
    } else {
        let home = env
            .home
            .clone()
            .ok_or("HOME isn't set, so there's nowhere to put it")?;
        Ok(Plan {
            dest: home.join(".local").join("bin").join(COMMAND),
            kind: if env.appimage {
                InstallKind::Copy
            } else {
                InstallKind::Link
            },
            admin: false,
        })
    }
}

/// Whether a file can be made in `dir`, without making one. A missing folder
/// is writable when its parent is.
fn writable(dir: &Path) -> bool {
    let mut at = dir;
    loop {
        match std::fs::metadata(at) {
            Ok(meta) => return meta.is_dir() && probe(at),
            Err(_) => match at.parent() {
                Some(parent) => at = parent,
                None => return false,
            },
        }
    }
}

fn probe(dir: &Path) -> bool {
    let at = dir.join(format!(".taurus-probe-{}", std::process::id()));
    let made = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&at)
        .is_ok();
    if made {
        let _ = std::fs::remove_file(&at);
    }
    made
}

/// What's at the destination already.
#[derive(Debug, PartialEq, Eq)]
pub enum Occupant {
    Nothing,
    /// Something this app put there, which it may replace.
    Ours,
    /// Something else, which it must not.
    Other,
}

/// Whether `dest` holds nothing, the app's own command, or someone else's.
///
/// A link is ours when it points at a `taurus-cli`: whichever copy of the app
/// made it, a link into a bundle is what this would make. A file is ours only
/// when the record says the app copied it there, because a `taurus` from
/// `cargo install` is a file too, and replacing it unasked would be rude.
pub fn occupant(dest: &Path, record: Option<&Record>) -> Occupant {
    let Ok(meta) = std::fs::symlink_metadata(dest) else {
        return Occupant::Nothing;
    };
    if meta.file_type().is_symlink() {
        let points_at_ours = std::fs::read_link(dest).is_ok_and(|target| {
            target
                .file_stem()
                .is_some_and(|stem| stem.eq_ignore_ascii_case(SIDECAR))
        });
        return if points_at_ours {
            Occupant::Ours
        } else {
            Occupant::Other
        };
    }
    let recorded = record
        .and_then(|r| r.installed.as_ref())
        .is_some_and(|i| i.kind == InstallKind::Copy && i.path == dest);
    if recorded {
        Occupant::Ours
    } else {
        Occupant::Other
    }
}

/// The first `taurus` a terminal would run, when it isn't `dest`.
fn shadowed_by(dest: &Path, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let path = path?;
    let name = exe(COMMAND);
    let same = |a: &Path, b: &Path| {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        canon(a) == canon(b)
    };
    for dir in std::env::split_paths(path) {
        let candidate = dir.join(&name);
        if candidate.is_file() {
            return (!same(&candidate, dest)).then_some(candidate);
        }
        if dir == dest.parent().unwrap_or(Path::new("")) {
            // Ours would be found here, even if it isn't there yet.
            return None;
        }
    }
    None
}

fn on_path(dir: &Path, path: Option<&std::ffi::OsStr>) -> bool {
    let in_env = path.is_some_and(|p| std::env::split_paths(p).any(|d| d == dir));
    in_env || windows::user_path_has(dir)
}

/// What Settings shows.
pub fn status(version: &str) -> CliStatus {
    let mut status = CliStatus {
        version: version.to_string(),
        target: None,
        installed: false,
        needs_admin: false,
        not_on_path: None,
        shadowed_by: None,
        cannot_install: None,
    };
    if tauri::is_dev() {
        status.cannot_install = Some(
            "This is a development build. Install the command from an installed copy, or run \
             `cargo install --path crates/taurus-cli --locked` from the clone."
                .into(),
        );
        return status;
    }
    let Some(bundled) = bundled() else {
        status.cannot_install = Some(
            "This build doesn't include the command. Release builds do; from a clone, run \
             `cargo install --path crates/taurus-cli --locked`."
                .into(),
        );
        return status;
    };
    let plan = match plan(&bundled, &Env::current()) {
        Ok(plan) => plan,
        Err(reason) => {
            status.cannot_install = Some(reason);
            return status;
        }
    };
    let record = desktop::load();
    let path = std::env::var_os("PATH");
    let dir = plan.dest.parent().unwrap_or(Path::new("/"));

    status.target = Some(plan.dest.display().to_string());
    status.needs_admin = plan.admin;
    status.installed = occupant(&plan.dest, record.as_ref()) == Occupant::Ours;
    if !on_path(dir, path.as_deref()) {
        status.not_on_path = Some(dir.display().to_string());
    }
    status.shadowed_by = shadowed_by(&plan.dest, path.as_deref()).map(|p| p.display().to_string());
    status
}

/// Puts the command on the PATH and records that it did.
pub fn install(version: &str) -> Result<(), String> {
    if tauri::is_dev() {
        return Err("This is a development build, which has no command to install.".into());
    }
    let bundled = bundled().ok_or("This build doesn't include the command.")?;
    let plan = plan(&bundled, &Env::current())?;
    let mut record = desktop::load().unwrap_or_default();

    if occupant(&plan.dest, Some(&record)) == Occupant::Other {
        return Err(format!(
            "There's already a `{COMMAND}` at {} that Taurus didn't put there. Remove it, \
             then try again.",
            plan.dest.display()
        ));
    }

    match plan.kind {
        InstallKind::Link => link(&bundled, &plan)?,
        InstallKind::Copy => copy(&bundled, &plan.dest)?,
    }
    if let Some(dir) = plan.dest.parent() {
        windows::add_to_user_path(dir)?;
    }

    record.version = version.to_string();
    record.bundled_cli = Some(bundled);
    record.installed = Some(Installed {
        path: plan.dest,
        kind: plan.kind,
        version: (plan.kind == InstallKind::Copy).then(|| version.to_string()),
    });
    desktop::save(&record)
}

#[cfg(unix)]
fn link(bundled: &Path, plan: &Plan) -> Result<(), String> {
    if plan.admin {
        return macos::link_as_admin(bundled, &plan.dest);
    }
    let dest = &plan.dest;
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    }
    // Made beside it and renamed over, so a failure leaves the old link, not
    // none.
    let staged = dest.with_file_name(format!(".{COMMAND}.{}", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    std::os::unix::fs::symlink(bundled, &staged)
        .and_then(|()| std::fs::rename(&staged, dest))
        .map_err(|e| {
            let _ = std::fs::remove_file(&staged);
            format!("couldn't link {}: {e}", dest.display())
        })
}

#[cfg(not(unix))]
fn link(_bundled: &Path, _plan: &Plan) -> Result<(), String> {
    Err("links aren't used on this platform".into())
}

/// Copies the bundle's CLI to `dest`, replacing what's there.
///
/// The old one is moved aside rather than overwritten: Windows refuses to
/// overwrite a running executable but lets it be renamed, and a `taurus repl`
/// left open in a terminal is a running executable.
fn copy(bundled: &Path, dest: &Path) -> Result<(), String> {
    let dir = dest.parent().ok_or("nowhere to copy to")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    let staged = dest.with_extension("new");
    let aside = dest.with_extension("old");
    let _ = std::fs::remove_file(&aside);
    std::fs::copy(bundled, &staged)
        .map_err(|e| format!("couldn't copy the command to {}: {e}", dir.display()))?;
    if dest.exists() {
        std::fs::rename(dest, &aside)
            .map_err(|e| format!("couldn't replace {}: {e}", dest.display()))?;
    }
    std::fs::rename(&staged, dest).map_err(|e| {
        // Put the old one back, so a failure leaves a working command.
        let _ = std::fs::rename(&aside, dest);
        format!("couldn't replace {}: {e}", dest.display())
    })?;
    // Fails while the old one is still running, which is harmless: the next
    // copy removes it first.
    let _ = std::fs::remove_file(&aside);
    Ok(())
}

/// Brings the record up to date with this launch, and refreshes a copy the
/// app made that's now older than the app.
///
/// Run once at startup, off the main thread. Says nothing from a dev build,
/// whose version is whatever the clone is at and whose CLI is in `target/`.
pub fn on_launch(version: &str) {
    if tauri::is_dev() {
        return;
    }
    let before = desktop::load();
    let mut record = before.clone().unwrap_or_default();
    record.version = version.to_string();
    record.bundled_cli = bundled();

    if let Some(installed) = record.installed.clone() {
        let still_ours = occupant(&installed.path, before.as_ref()) == Occupant::Ours;
        if !still_ours {
            // Removed, or replaced by something else. Either way it isn't ours
            // to refresh any more.
            tracing::info!(path = %installed.path.display(), "the installed taurus command is gone");
            record.installed = None;
        } else if installed.kind == InstallKind::Copy
            && installed.version.as_deref() != Some(version)
        {
            match &record.bundled_cli {
                Some(bundled) => match copy(bundled, &installed.path) {
                    Ok(()) => {
                        tracing::info!(path = %installed.path.display(), %version, "refreshed the taurus command");
                        record.installed = Some(Installed {
                            version: Some(version.to_string()),
                            ..installed
                        });
                    }
                    Err(e) => tracing::warn!(error = %e, "couldn't refresh the taurus command"),
                },
                None => tracing::warn!(
                    "the taurus command is a copy, and this build has none to refresh it from"
                ),
            }
        }
    }

    if before.as_ref() != Some(&record) {
        if let Err(e) = desktop::save(&record) {
            tracing::warn!(error = %e, "couldn't record this app's version for the CLI");
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::Path;

    /// Makes the link through `osascript`, which shows the system's own
    /// password prompt. No helper is installed, and nothing runs as root but
    /// these two commands.
    pub fn link_as_admin(bundled: &Path, dest: &Path) -> Result<(), String> {
        let dir = dest.parent().ok_or("nowhere to link to")?;
        let command = format!(
            "mkdir -p {} && ln -sfn {} {}",
            super::shell_quote(&dir.to_string_lossy()),
            super::shell_quote(&bundled.to_string_lossy()),
            super::shell_quote(&dest.to_string_lossy()),
        );
        let script = format!(
            "do shell script {} with administrator privileges",
            super::applescript_string(&command)
        );
        let output = std::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|e| format!("couldn't ask for an administrator password: {e}"))?;
        if output.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&output.stderr);
        // -128 is the Cancel button.
        if err.contains("-128") {
            return Err("Cancelled. Nothing was installed.".into());
        }
        Err(format!("couldn't link {}: {}", dest.display(), err.trim()))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod macos {
    use std::path::Path;

    pub fn link_as_admin(_bundled: &Path, dest: &Path) -> Result<(), String> {
        Err(format!("{} isn't writable", dest.display()))
    }
}

/// `'…'`, with each `'` closed, escaped and reopened.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// An AppleScript string literal.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', r"\\").replace('"', "\\\""))
}

/// The user's PATH, in the registry, which is what a new terminal reads.
#[cfg(windows)]
mod windows {
    use std::path::Path;

    use windows_registry::{Type, CURRENT_USER};

    fn read() -> Option<(String, Type)> {
        let key = CURRENT_USER.open("Environment").ok()?;
        let value = key.get_value("Path").ok()?;
        let ty = value.ty();
        let text = String::try_from(value).ok()?;
        Some((text, ty))
    }

    fn contains(path: &str, dir: &Path) -> bool {
        let dir = dir.to_string_lossy();
        let dir = dir.trim_end_matches('\\');
        path.split(';')
            .any(|entry| entry.trim_end_matches('\\').eq_ignore_ascii_case(dir))
    }

    pub fn user_path_has(dir: &Path) -> bool {
        read().is_some_and(|(path, _)| contains(&path, dir))
    }

    /// Appends `dir` to the user's PATH, keeping the value's type: a PATH with
    /// `%USERPROFILE%` in it is an expandable string, and writing it back as a
    /// plain one would stop every entry like that from resolving.
    ///
    /// A terminal opened after this sees it. One already open doesn't, and
    /// neither does a terminal started by an Explorer that hasn't heard yet;
    /// signing out and back in settles both.
    pub fn add_to_user_path(dir: &Path) -> Result<(), String> {
        let (path, ty) = read().unwrap_or((String::new(), Type::ExpandString));
        if contains(&path, dir) {
            return Ok(());
        }
        let dir = dir.to_string_lossy();
        let next = if path.trim().is_empty() {
            dir.to_string()
        } else {
            format!("{};{dir}", path.trim_end_matches(';'))
        };
        let key = CURRENT_USER
            .create("Environment")
            .map_err(|e| format!("couldn't open the user environment: {e}"))?;
        let written = if ty == Type::ExpandString {
            key.set_expand_string("Path", &next)
        } else {
            key.set_string("Path", &next)
        };
        written.map_err(|e| format!("couldn't add {dir} to your PATH: {e}"))
    }
}

#[cfg(not(windows))]
mod windows {
    use std::path::Path;

    pub fn user_path_has(_dir: &Path) -> bool {
        false
    }

    pub fn add_to_user_path(_dir: &Path) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_survives_a_path_with_quotes_in_it() {
        assert_eq!(shell_quote("/Apps/It's.app"), r"'/Apps/It'\''s.app'");
        assert_eq!(
            applescript_string(r#"say "hi" \ there"#),
            r#""say \"hi\" \\ there""#
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_translocated_or_mounted_app_cant_be_linked_to() {
        let env = Env::default();
        for at in [
            "/private/var/folders/x/AppTranslocation/ABC/d/Taurus.app/Contents/MacOS/taurus-cli",
            "/Volumes/Taurus/Taurus.app/Contents/MacOS/taurus-cli",
        ] {
            assert!(plan(Path::new(at), &env).is_err(), "{at}");
        }
        let plan = plan(
            Path::new("/Applications/Taurus.app/Contents/MacOS/taurus-cli"),
            &env,
        )
        .unwrap();
        assert_eq!(plan.dest, Path::new("/usr/local/bin/taurus"));
        assert_eq!(plan.kind, InstallKind::Link);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn an_appimage_is_copied_and_a_package_is_linked() {
        let mut env = Env {
            home: Some("/home/me".into()),
            ..Env::default()
        };
        let linked = plan(Path::new("/usr/bin/taurus-cli"), &env).unwrap();
        assert_eq!(linked.dest, Path::new("/home/me/.local/bin/taurus"));
        assert_eq!(linked.kind, InstallKind::Link);
        env.appimage = true;
        assert_eq!(
            plan(Path::new("/tmp/.mount_x/usr/bin/taurus-cli"), &env)
                .unwrap()
                .kind,
            InstallKind::Copy
        );
    }

    #[cfg(unix)]
    #[test]
    fn only_what_the_app_made_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("taurus");
        assert_eq!(occupant(&dest, None), Occupant::Nothing);

        std::os::unix::fs::symlink("/Applications/Old.app/Contents/MacOS/taurus-cli", &dest)
            .unwrap();
        assert_eq!(occupant(&dest, None), Occupant::Ours);

        std::fs::remove_file(&dest).unwrap();
        std::os::unix::fs::symlink("/opt/homebrew/bin/something", &dest).unwrap();
        assert_eq!(occupant(&dest, None), Occupant::Other);

        // A file is someone else's, a `cargo install` say, unless the record
        // says the app copied it there.
        std::fs::remove_file(&dest).unwrap();
        std::fs::write(&dest, "binary").unwrap();
        assert_eq!(occupant(&dest, None), Occupant::Other);
        let record = Record {
            version: "0.4.1".into(),
            bundled_cli: None,
            installed: Some(Installed {
                path: dest.clone(),
                kind: InstallKind::Copy,
                version: Some("0.4.1".into()),
            }),
        };
        assert_eq!(occupant(&dest, Some(&record)), Occupant::Ours);
    }

    #[test]
    fn a_copy_replaces_the_old_one_and_leaves_nothing_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let bundled = dir.path().join("taurus-cli");
        let dest = dir.path().join("bin").join("taurus");
        std::fs::write(&bundled, "new").unwrap();
        copy(&bundled, &dest).unwrap();
        std::fs::write(&bundled, "newer").unwrap();
        copy(&bundled, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "newer");
        let left: Vec<_> = std::fs::read_dir(dest.parent().unwrap()).unwrap().collect();
        assert_eq!(left.len(), 1);
    }

    #[test]
    fn a_taurus_earlier_on_the_path_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let cargo = dir.path().join("cargo");
        let ours = dir.path().join("ours");
        std::fs::create_dir_all(&cargo).unwrap();
        std::fs::create_dir_all(&ours).unwrap();
        std::fs::write(cargo.join(exe(COMMAND)), "old").unwrap();
        let dest = ours.join(exe(COMMAND));

        let path = std::env::join_paths([&cargo, &ours]).unwrap();
        assert_eq!(
            shadowed_by(&dest, Some(&path)),
            Some(cargo.join(exe(COMMAND)))
        );

        let path = std::env::join_paths([&ours, &cargo]).unwrap();
        assert_eq!(shadowed_by(&dest, Some(&path)), None);
    }
}

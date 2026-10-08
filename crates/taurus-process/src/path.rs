//! The PATH a child is started with, once it has been read again.
//!
//! At startup the login shell's PATH is merged into this process's own
//! environment, before any thread exists (see `taurus_tools::login_path`).
//! That can't be done twice: setting an environment variable while other
//! threads may be reading the environment is a data race in the C library on
//! unix. So a PATH read later lives here instead, behind a lock, and every
//! place Taurus starts a program hands it to the child explicitly.
//!
//! Until something is read again, nothing here does anything: [`apply`] leaves
//! a command inheriting the process environment, exactly as it did before.

use std::ffi::OsString;
use std::sync::RwLock;

static LIVE: RwLock<Option<OsString>> = RwLock::new(None);

/// The PATH a child gets: the last one read again, else this process's own.
pub fn current() -> OsString {
    replaced().unwrap_or_else(|| std::env::var_os("PATH").unwrap_or_default())
}

/// The PATH read again, when one has been. `None` means children inherit the
/// process environment.
pub fn replaced() -> Option<OsString> {
    LIVE.read().ok().and_then(|live| live.clone())
}

/// Makes `path` the PATH every child started from now on gets.
pub fn set(path: impl Into<OsString>) {
    if let Ok(mut live) = LIVE.write() {
        *live = Some(path.into());
    }
}

/// Gives a command the current PATH, when one has been read again.
///
/// On unix the program itself is looked up on the PATH the command carries,
/// not the parent's, so a program installed after startup is found too.
pub fn apply(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    if let Some(path) = replaced() {
        command.env("PATH", path);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_path_read_again_reaches_the_child_and_finds_its_program() {
        // A program in a directory the process environment doesn't have, the
        // way `npm i -g` lands one after the app has started.
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let name = {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.path().join("installed-later");
            std::fs::write(&path, "#!/bin/sh\necho found\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            "installed-later"
        };
        #[cfg(windows)]
        let name = {
            std::fs::write(dir.path().join("installed-later.bat"), "@echo found\r\n").unwrap();
            "installed-later.bat"
        };

        let before = tokio::process::Command::new(name).output().await;
        assert!(before.is_err(), "not on the startup PATH");

        let mut path = std::env::split_paths(&current()).collect::<Vec<_>>();
        path.insert(0, dir.path().to_path_buf());
        set(std::env::join_paths(path).unwrap());

        // Linux won't run a file that any process has open for writing, and a
        // sibling test that forks while this one's write is still open holds
        // it until that child execs. That's milliseconds, so it's waited out
        // here rather than left to flake as "Text file busy".
        let mut busy = 0;
        let out = loop {
            let mut command = tokio::process::Command::new(name);
            match apply(&mut command).output().await {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && busy < 50 => {
                    busy += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                other => break other.expect("found on the new PATH"),
            }
        };
        assert!(String::from_utf8_lossy(&out.stdout).contains("found"));
    }
}

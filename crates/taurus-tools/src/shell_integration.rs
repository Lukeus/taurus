//! Starting the dock's shell so it marks where each command begins and ends.
//!
//! The marks are the `OSC 133` convention described in
//! [`crate::blocks`]. A shell prints them from hooks, and the hooks have
//! to be installed by something that runs inside the shell, after the user's
//! own startup files and without editing any of them. Each shell needs its own
//! way in, and this is the same way in VS Code uses:
//!
//! - **zsh** reads its startup files from `$ZDOTDIR`. The shell starts with
//!   that pointing at a directory of Taurus's whose `.zshenv`, `.zprofile`,
//!   `.zshrc` and `.zlogin` each source the user's own file of the same name
//!   and then hand back. The hooks go in after the user's `.zshrc`, and
//!   `.zlogin`, read last, puts `ZDOTDIR` back how it was found.
//! - **bash** takes `--init-file`, which it reads in place of `~/.bashrc`.
//!   Taurus's file reads what bash would have read and then adds the hooks.
//!
//! Every other shell starts exactly as it did before, unmarked: fish and
//! PowerShell have integration points of their own that aren't written yet,
//! and `cmd.exe` has none. The dock still works there, just without blocks.
//! See `docs/known-gaps.md`.
//!
//! # A login shell, as before
//!
//! `portable-pty` starts the default shell as a *login* shell (its `argv[0]`
//! is `-zsh`), which is also what Terminal.app does on macOS. That's where
//! Homebrew's `PATH` setup usually lives (`.zprofile`), so an integrated shell
//! has to stay a login shell or it would lose it. zsh gets `-l`. bash can't
//! combine `--login` with `--init-file`, so Taurus's file reads the profile
//! files itself, in bash's own order.
//!
//! # Where the scripts live
//!
//! Compiled in, and written under `~/.taurus/shell-integration/<version>/` the
//! first time a shell needs them. A file is only rewritten when its contents
//! differ, so opening a shell costs a few reads. Versioned, so a shell started
//! by one build never sources a half-updated set from another.

use std::path::{Path, PathBuf};

use portable_pty::CommandBuilder;
use tracing::warn;

/// The scripts, as `(path under the version directory, contents)`.
const FILES: &[(&str, &str)] = &[
    (
        "zsh/.zshenv",
        include_str!("../shell-integration/zsh/.zshenv"),
    ),
    (
        "zsh/.zprofile",
        include_str!("../shell-integration/zsh/.zprofile"),
    ),
    (
        "zsh/.zshrc",
        include_str!("../shell-integration/zsh/.zshrc"),
    ),
    (
        "zsh/.zlogin",
        include_str!("../shell-integration/zsh/.zlogin"),
    ),
    (
        "zsh/taurus.zsh",
        include_str!("../shell-integration/zsh/taurus.zsh"),
    ),
    (
        "taurus.bash",
        include_str!("../shell-integration/taurus.bash"),
    ),
];

/// A shell this module knows how to integrate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Zsh,
    Bash,
}

impl Kind {
    /// Recognizes a shell by its program's name: `/bin/zsh`, `zsh-5.9`.
    pub fn of(program: &str) -> Option<Self> {
        let name = Path::new(program).file_name()?.to_str()?;
        let name = name.strip_suffix(".exe").unwrap_or(name);
        if name == "zsh" || name.starts_with("zsh-") {
            Some(Self::Zsh)
        } else if name == "bash" || name.starts_with("bash-") {
            Some(Self::Bash)
        } else {
            None
        }
    }
}

/// The program's name, for labels: `zsh`.
pub fn name_of(program: &str) -> String {
    Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(program)
        .to_string()
}

/// A shell started with the hooks, if `program` is one this can integrate and
/// the scripts could be written. `None` means start it the ordinary way.
///
/// `home` is Taurus's config directory, `~/.taurus`.
pub fn command(program: &str, home: &Path) -> Option<CommandBuilder> {
    let kind = Kind::of(program)?;
    let dir = match install(home) {
        Ok(dir) => dir,
        Err(e) => {
            // A shell without marks is still a shell. Said in the log because
            // the dock has nowhere to put it that isn't in the way.
            warn!(error = %e, "could not write the shell integration; starting the shell without it");
            return None;
        }
    };
    let mut builder = CommandBuilder::new(program);
    match kind {
        Kind::Zsh => {
            let zdotdir = dir.join("zsh");
            let user = std::env::var("ZDOTDIR").ok().filter(|d| !d.is_empty());
            builder.arg("-l");
            builder.env("ZDOTDIR", &zdotdir);
            builder.env("TAURUS_ZDOTDIR", &zdotdir);
            builder.env("TAURUS_USER_ZDOTDIR", user.clone().unwrap_or_default());
            builder.env(
                "TAURUS_ZDOTDIR_WAS_SET",
                if user.is_some() { "1" } else { "0" },
            );
        }
        Kind::Bash => {
            builder.arg("--init-file");
            builder.arg(dir.join("taurus.bash"));
            builder.env("TAURUS_SHELL_LOGIN", "1");
        }
    }
    Some(builder)
}

/// Writes the scripts where a shell can source them, and says where.
pub fn install(home: &Path) -> std::io::Result<PathBuf> {
    let dir = home
        .join("shell-integration")
        .join(env!("CARGO_PKG_VERSION"));
    for (relative, contents) in FILES {
        let path = dir.join(relative);
        if std::fs::read_to_string(&path).is_ok_and(|on_disk| on_disk == *contents) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Written beside and renamed over, so a shell starting in the same
        // instant never sources half a file.
        let partial = path.with_extension("partial");
        std::fs::write(&partial, contents)?;
        std::fs::rename(&partial, &path)?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shells_are_recognized_by_name_wherever_they_are_installed() {
        assert_eq!(Kind::of("/bin/zsh"), Some(Kind::Zsh));
        assert_eq!(Kind::of("/opt/homebrew/bin/bash"), Some(Kind::Bash));
        assert_eq!(Kind::of("zsh-5.9"), Some(Kind::Zsh));
        assert_eq!(Kind::of("/usr/bin/fish"), None);
        assert_eq!(Kind::of("pwsh.exe"), None);
        assert_eq!(name_of("/bin/zsh"), "zsh");
        assert_eq!(name_of(r"pwsh.exe"), "pwsh");
    }

    #[test]
    fn the_scripts_are_written_once_and_left_alone_after() {
        let home = tempfile::tempdir().unwrap();
        let dir = install(home.path()).unwrap();
        let rc = dir.join("zsh/.zshrc");
        assert!(rc.is_file());
        let first = std::fs::metadata(&rc).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        install(home.path()).unwrap();
        assert_eq!(std::fs::metadata(&rc).unwrap().modified().unwrap(), first);
        // A file someone edited goes back to what this build ships.
        std::fs::write(&rc, "# changed").unwrap();
        install(home.path()).unwrap();
        assert_ne!(std::fs::read_to_string(&rc).unwrap(), "# changed");
    }

    /// Runs `script` in a real shell started the way the dock starts one,
    /// with `home` as its `$HOME`, and returns every command it ran.
    #[cfg(unix)]
    fn run_shell(program: &str, home: &Path, script: &str) -> Vec<crate::blocks::BlockText> {
        use portable_pty::{native_pty_system, PtySize};
        use std::io::{Read, Write};

        let taurus = tempfile::tempdir().unwrap();
        let mut builder = command(program, taurus.path()).expect("a shell this integrates");
        builder.env("HOME", home);
        builder.env("TERM", "xterm-256color");
        builder.cwd(home);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 200,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut child = pair.slave.spawn_command(builder).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut writer = pair.master.take_writer().unwrap();

        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });

        let mut tracker = crate::blocks::BlockTracker::default();
        let mut seen = Vec::new();
        // One line at a time, each after the prompt before it has been
        // drawn: typed ahead, a line editor may echo it before the prompt,
        // which is a real terminal's behavior and not what this is testing.
        let mut lines = script.lines().chain(std::iter::once("exit")).peekable();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut prompts_seen = 0;
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(chunk) => {
                    seen.extend_from_slice(&chunk);
                    tracker.feed(&chunk);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            let prompts = String::from_utf8_lossy(&seen).matches("\x1b]133;B").count();
            if prompts > prompts_seen {
                prompts_seen = prompts;
                if let Some(line) = lines.next() {
                    writer.write_all(format!("{line}\n").as_bytes()).unwrap();
                    writer.flush().unwrap();
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the shell never finished; it said:\n{}",
                String::from_utf8_lossy(&seen)
            );
        }
        let _ = child.wait();
        assert!(
            lines.peek().is_none(),
            "the shell stopped drawing prompts:\n{}",
            String::from_utf8_lossy(&seen)
        );
        tracker.recent(100)
    }

    #[cfg(unix)]
    fn find(blocks: &[crate::blocks::BlockText], command: &str) -> crate::blocks::BlockText {
        blocks
            .iter()
            .find(|b| b.block.command == command)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "no block for {command:?} in {:#?}",
                    blocks.iter().map(|b| &b.block).collect::<Vec<_>>()
                )
            })
    }

    #[cfg(unix)]
    #[test]
    fn zsh_marks_each_command_and_still_reads_your_own_startup_files() {
        if !Path::new("/bin/zsh").exists() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        std::fs::write(
            home_path.join(".zprofile"),
            "export FROM_PROFILE=profile-ran\n",
        )
        .unwrap();
        // A precmd hook of the user's own, which ours has to sit around
        // rather than replace, and a prompt that is rebuilt on every draw.
        std::fs::write(
            home_path.join(".zshrc"),
            "alias greet='echo hello from an alias'\nmy_precmd() { PS1='mine> ' }\nprecmd_functions+=(my_precmd)\n",
        )
        .unwrap();
        std::fs::create_dir(home_path.join("sub dir")).unwrap();

        let blocks = run_shell(
            "/bin/zsh",
            &home_path,
            "greet\necho $FROM_PROFILE\nfalse\necho ${ZDOTDIR-unset}\ncd 'sub dir'\nprintf 'caf\\303\\251\\n'",
        );

        let greet = find(&blocks, "greet");
        assert_eq!(greet.output, "hello from an alias");
        assert_eq!(greet.block.exit, Some(0));
        assert_eq!(
            find(&blocks, "echo $FROM_PROFILE").output,
            "profile-ran",
            "a login shell reads .zprofile"
        );
        assert_eq!(find(&blocks, "false").block.exit, Some(1));
        assert_eq!(
            find(&blocks, "echo ${ZDOTDIR-unset}").output,
            "unset",
            "ZDOTDIR goes back to how it was found"
        );
        let after_cd = find(&blocks, "printf 'caf\\303\\251\\n'");
        assert_eq!(after_cd.output, "café");
        assert_eq!(
            after_cd.block.cwd.as_deref(),
            Some(home_path.join("sub dir").to_str().unwrap()),
            "the directory follows a cd, spaces and all"
        );
    }

    #[cfg(unix)]
    #[test]
    fn bash_marks_each_command_and_still_reads_your_own_startup_files() {
        if !Path::new("/bin/bash").exists() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        // The usual arrangement: the profile sources .bashrc.
        std::fs::write(
            home_path.join(".bash_profile"),
            "export FROM_PROFILE=profile-ran\n[ -r ~/.bashrc ] && . ~/.bashrc\n",
        )
        .unwrap();
        std::fs::write(
            home_path.join(".bashrc"),
            "alias greet='echo hello from an alias'\nPROMPT_COMMAND='MINE=1'\nHISTCONTROL=ignorespace\n",
        )
        .unwrap();

        let blocks = run_shell(
            "/bin/bash",
            &home_path,
            "greet\necho $FROM_PROFILE\nfalse\n echo hidden\necho $MINE",
        );

        assert_eq!(find(&blocks, "greet").output, "hello from an alias");
        assert_eq!(find(&blocks, "echo $FROM_PROFILE").output, "profile-ran");
        assert_eq!(find(&blocks, "false").block.exit, Some(1));
        assert_eq!(
            find(&blocks, "echo $MINE").output,
            "1",
            "your PROMPT_COMMAND still runs"
        );
        // A leading space keeps a command out of history, which is a request
        // not to keep it. It's still a block, without its line.
        let hidden = find(&blocks, "");
        assert_eq!(hidden.output, "hidden");
    }
}

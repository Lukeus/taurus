//! Finding the PATH the user actually has, when the app was not started from a
//! shell.
//!
//! A desktop app launched from the Dock, Finder, or a `.desktop` entry is
//! started by the session manager, not by a shell. It inherits that launcher's
//! environment, and on macOS launchd's is `/usr/bin:/bin:/usr/sbin:/sbin` and
//! nothing else. None of the profile a user has spent years assembling is in
//! it: no Homebrew, no nvm, no pyenv, no `~/.local/bin`.
//!
//! That is invisible in development, because `tauri dev` is run from a terminal
//! and inherits a complete PATH. It shows up in an installed build as an MCP
//! server that will not start, or a skill whose interpreter is "not installed"
//! on a machine where it plainly is — the single most common reason a working
//! `mcp.json` does nothing. `npx` is the usual casualty, because the servers
//! everybody installs first are npm packages and every popular Node manager
//! puts `npx` somewhere launchd has never heard of.
//!
//! So the shell is asked. Once, at startup, before anything is spawned: run the
//! user's login shell the way a terminal would and read back the PATH it ends up
//! with. The answer is merged into this process's own environment rather than
//! threaded through every call site, because the consumers are
//! [`std::process::Command`], `portable-pty`, and the interpreter lookup in
//! `taurus-skills` — all of which read the process environment, and all of which
//! then get the fix for free.
//!
//! # Why an interactive shell
//!
//! `-l` alone reads the login files: `.zprofile`, `.bash_profile`, `.profile`.
//! It does not read `.zshrc` or `.bashrc`, and that is where nvm, pyenv, rbenv,
//! and conda actually install themselves — every one of those tools writes its
//! setup into the interactive file. A login-only probe would come back without
//! the very entries this exists to find, so `-i` is asked for too, and the
//! output is fenced with markers because an interactive shell also prints
//! whatever the user's profile prints.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Set to any non-empty value to skip the probe entirely.
///
/// For someone whose profile is slow or has side effects they would rather not
/// pay at launch, and for tests, which must not run the developer's `.zshrc`.
pub const SKIP_ENV: &str = "TAURUS_SKIP_LOGIN_PATH";

/// How long the shell gets before it is killed and the probe gives up.
///
/// A profile that takes longer than this is one the user is already unhappy
/// with, and a launch that hangs on it would be worse than a PATH that is
/// merely incomplete.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Fences the answer off from whatever an interactive profile prints on its way
/// past. Long enough not to occur by accident in a MOTD.
const BEGIN: &str = "__TAURUS_PATH_BEGIN__";
const END: &str = "__TAURUS_PATH_END__";

/// What the probe found, once it has run.
static RESOLVED: OnceLock<Outcome> = OnceLock::new();

/// What asking the shell came to.
///
/// Kept rather than discarded because the MCP panel shows it: "this server
/// could not start" and "here is where Taurus looked for it" are the same
/// question, and answering only the first sends people to check a spelling that
/// was never wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The PATH now in force for this process and everything it starts.
    pub path: String,
    /// The directories the shell knew about that the launcher did not. Empty
    /// when the app was started from a terminal, which is the case where this
    /// whole module has nothing to do.
    pub added: Vec<String>,
    /// Why the shell was not asked, or why asking failed. `None` on success.
    pub skipped: Option<String>,
}

/// Merges the login shell's PATH into this process's, and returns what happened.
///
/// Call once, as early in startup as possible: everything spawned afterwards
/// inherits the result, and anything spawned before it does not. Calling again
/// is free and returns the first answer — the shell is asked exactly once per
/// process.
pub fn adopt() -> &'static Outcome {
    RESOLVED.get_or_init(resolve)
}

/// The PATH in force, whether or not [`adopt`] has run.
///
/// What a diagnostic should print: the question being answered is "where did
/// Taurus look", and that is this process's PATH regardless of how it got there.
pub fn current() -> String {
    std::env::var("PATH").unwrap_or_default()
}

/// Every directory on the current PATH, for a panel that lists them.
pub fn entries() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default()
}

/// Whether `program` can be found on the PATH in force, and where.
///
/// The check the MCP panel runs before it lets someone save a stdio server, so
/// "npx is not on this app's PATH" is said while the entry is still on screen
/// rather than discovered at the next reload.
pub fn which(program: &str) -> Option<PathBuf> {
    // An explicit path is not a PATH lookup. Answering one from the search
    // directories would report `/opt/homebrew/bin/./node` for `./node`.
    if program.contains('/') || (cfg!(windows) && program.contains('\\')) {
        let path = PathBuf::from(program);
        return is_executable(&path).then_some(path);
    }
    for dir in entries() {
        for candidate in with_extensions(&dir.join(program)) {
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn resolve() -> Outcome {
    let before = current();

    if let Some(reason) = skip_reason() {
        return Outcome {
            path: before,
            added: Vec::new(),
            skipped: Some(reason),
        };
    }

    let started = Instant::now();
    let from_shell = match ask_shell() {
        Ok(path) => path,
        Err(reason) => {
            return Outcome {
                path: before,
                added: Vec::new(),
                skipped: Some(reason),
            }
        }
    };

    let (path, added) = merge(&from_shell, &before);
    tracing::info!(
        added = added.len(),
        ms = started.elapsed().as_millis(),
        "login shell PATH adopted"
    );
    // Safe in edition 2021, and this runs before the app starts any thread that
    // reads the environment — which is why it is called first thing in `run`
    // rather than lazily on the first spawn.
    std::env::set_var("PATH", &path);

    Outcome {
        path,
        added,
        skipped: None,
    }
}

/// Why not to ask, when there is a reason.
fn skip_reason() -> Option<String> {
    if std::env::var_os(SKIP_ENV).is_some_and(|v| !v.is_empty()) {
        return Some(format!("{SKIP_ENV} is set"));
    }
    // Windows has no equivalent problem: a GUI process there inherits the user
    // and machine PATH from the registry, which is the same one a console gets.
    if cfg!(windows) {
        return Some("not needed on Windows, where a GUI process gets the full PATH".into());
    }
    None
}

/// Runs the login shell and reads the PATH it settles on.
fn ask_shell() -> Result<String, String> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string());

    // Interactive first, because that is the one that reads `.zshrc` and so the
    // one that knows about nvm. A shell that refuses `-i` without a terminal
    // still gets a login-only chance rather than being given up on.
    let mut last = String::new();
    for flags in [["-l", "-i", "-c"], ["-l", "-c", ""]] {
        let args: Vec<&str> = flags.iter().copied().filter(|f| !f.is_empty()).collect();
        match run(&shell, &args) {
            Ok(path) if !path.trim().is_empty() => return Ok(path),
            Ok(_) => last = format!("{shell} reported an empty PATH"),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// One attempt, bounded by [`TIMEOUT`].
fn run(shell: &str, args: &[&str]) -> Result<String, String> {
    use std::process::{Command, Stdio};

    let script = format!(r#"printf '{BEGIN}%s{END}' "$PATH""#);
    let mut child = Command::new(shell)
        .args(args)
        .arg(&script)
        // Not inherited: an interactive shell with the app's stdin can block
        // waiting on it, and a profile that prints is noise on the app's own
        // stderr.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Some profiles branch on this and try to draw. `dumb` is the
        // conventional way to say "there is nothing to draw on".
        .env("TERM", "dumb")
        .spawn()
        .map_err(|e| format!("could not run {shell}: {e}"))?;

    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{shell} did not answer within {}s; its startup files may be slow",
                    TIMEOUT.as_secs()
                ));
            }
            Err(e) => return Err(format!("could not wait for {shell}: {e}")),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("could not read from {shell}: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    between(&text).ok_or_else(|| format!("{shell} printed no PATH between the markers"))
}

/// The fenced answer, ignoring everything a profile printed around it.
fn between(text: &str) -> Option<String> {
    let start = text.find(BEGIN)? + BEGIN.len();
    let rest = &text[start..];
    let end = rest.find(END)?;
    Some(rest[..end].to_string())
}

/// The shell's PATH, then anything the launcher had that it did not.
///
/// The shell's order is kept rather than the launcher's, because that order is a
/// preference the user expressed: someone who puts `~/.local/bin` before
/// `/usr/bin` means the one in `~/.local/bin`. Nothing already present is
/// re-added, so a launcher entry the shell also has stays where the shell put it.
fn merge(from_shell: &str, current: &str) -> (String, Vec<String>) {
    let mut ordered: Vec<PathBuf> = Vec::new();
    let mut push = |dir: PathBuf| {
        if !dir.as_os_str().is_empty() && !ordered.contains(&dir) {
            ordered.push(dir);
        }
    };

    for dir in std::env::split_paths(from_shell) {
        push(dir);
    }
    let had: Vec<PathBuf> = std::env::split_paths(current).collect();
    for dir in &had {
        push(dir.clone());
    }

    let added = ordered
        .iter()
        .filter(|dir| !had.contains(dir))
        .map(|dir| dir.display().to_string())
        .collect();

    let joined = std::env::join_paths(&ordered)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| current.to_string());
    (joined, added)
}

#[cfg(windows)]
fn with_extensions(base: &std::path::Path) -> Vec<PathBuf> {
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let mut out = vec![base.to_path_buf()];
    for ext in pathext.split(';').filter(|e| !e.is_empty()) {
        let mut with = base.as_os_str().to_os_string();
        with.push(ext);
        out.push(PathBuf::from(with));
    }
    out
}

#[cfg(not(windows))]
fn with_extensions(base: &std::path::Path) -> Vec<PathBuf> {
    vec![base.to_path_buf()]
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_is_taken_from_between_the_markers() {
        // The reason the markers exist: an interactive shell prints whatever the
        // user's profile prints, and it lands on the same stream.
        let noisy = format!(
            "Welcome back!\nnvm: using v22\n{BEGIN}/usr/local/bin:/usr/bin{END}\nHave a nice day\n"
        );
        assert_eq!(between(&noisy).unwrap(), "/usr/local/bin:/usr/bin");
    }

    #[test]
    fn output_with_no_markers_is_not_mistaken_for_a_path() {
        // A shell that failed to run the script still exits zero in some
        // configurations. Taking its banner as a PATH would replace a working
        // one with a sentence.
        assert!(between("command not found: printf\n").is_none());
        assert!(between(&format!("{BEGIN}unterminated")).is_none());
    }

    #[test]
    fn the_shells_order_wins_and_nothing_is_duplicated() {
        // Someone who puts ~/.local/bin ahead of /usr/bin means the one in
        // ~/.local/bin. Appending the shell's entries to launchd's instead would
        // silently invert that.
        let (merged, added) = merge("/home/u/.local/bin:/usr/bin", "/usr/bin:/sbin");
        assert_eq!(merged, "/home/u/.local/bin:/usr/bin:/sbin");
        assert_eq!(added, vec!["/home/u/.local/bin"]);
    }

    #[test]
    fn nothing_the_launcher_had_is_lost() {
        // The merge only ever adds. A shell whose PATH is missing a directory
        // the app was started with must not take it away — that would break the
        // case this is meant to fix in the other direction.
        let (merged, added) = merge("/opt/homebrew/bin", "/usr/bin:/bin");
        assert_eq!(merged, "/opt/homebrew/bin:/usr/bin:/bin");
        assert_eq!(added, vec!["/opt/homebrew/bin"]);
    }

    #[test]
    fn a_terminal_launch_adds_nothing() {
        // Started from a shell, the app already has the answer, and the probe
        // has nothing to contribute. Worth asserting because "added" is what the
        // panel shows, and a list of directories that were already there would
        // read as a fix that was needed.
        let (merged, added) = merge("/usr/bin:/bin", "/usr/bin:/bin");
        assert_eq!(merged, "/usr/bin:/bin");
        assert!(added.is_empty());
    }

    #[test]
    fn the_escape_hatch_stops_the_probe() {
        std::env::set_var(SKIP_ENV, "1");
        let reason = skip_reason().expect("setting the variable must skip the probe");
        std::env::remove_var(SKIP_ENV);
        assert!(reason.contains(SKIP_ENV), "{reason}");
    }

    #[test]
    fn which_finds_a_real_program_and_admits_when_it_cannot() {
        // Whatever the platform, one of these exists; none of these do.
        let found = which("sh").or_else(|| which("cmd"));
        assert!(found.is_some(), "PATH is {}", current());
        assert!(which("definitely-not-a-real-program-xyz").is_none());
    }

    #[test]
    fn an_explicit_path_is_checked_rather_than_searched() {
        // `command` in an mcp.json is often an absolute path precisely because
        // PATH did not work. Running that through the search directories would
        // answer for a different file.
        assert!(which("/definitely/not/here/npx").is_none());
        if cfg!(unix) {
            assert_eq!(which("/bin/sh"), Some(PathBuf::from("/bin/sh")));
        }
    }

    #[test]
    #[cfg(unix)]
    fn asking_a_real_shell_comes_back_with_a_real_path() {
        // The whole mechanism end to end, against the one shell every unix has.
        // If the fencing, the flags, or the script quoting are wrong, this is
        // where it shows — the alternative is finding out on a user's Dock.
        let path = run("/bin/sh", &["-l", "-c"]).expect("sh must answer");
        assert!(path.contains('/'), "{path:?}");
        assert!(
            !path.contains(BEGIN),
            "the markers must be stripped: {path:?}"
        );
    }
}

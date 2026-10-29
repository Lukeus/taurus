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
//!
//! # Asking again
//!
//! A program installed after launch (`npm i -g` in a terminal beside the app)
//! isn't on the PATH read at startup. [`rescan`] asks the shell again. It can't
//! write the answer into the process environment the way startup does, because
//! other threads are running by then, so it goes to
//! [`taurus_process::path`], which every place Taurus starts a program reads.
//!
//! # Windows
//!
//! There's no login shell to ask, and the problem has a different shape. A
//! process there inherits a copy of whatever started it, taken at that moment:
//! Explorer's, or VS Code's, or that of a terminal opened last week. The PATH a
//! user actually has is the one saved in the registry, system entries then
//! their own, and an installer such as rustup writes there. So a Taurus started
//! before `cargo` was installed, or from anything that was, can't find `cargo`,
//! while a new Windows Terminal tab can, because Windows Terminal builds each
//! tab's environment from the registry rather than from its own.
//!
//! That's what this does on Windows, in the same two places: the registry is
//! read at startup and again on [`rescan`]. It's appended rather than put
//! first, unlike the shell's answer. Whatever started Taurus may have put a
//! toolchain ahead on purpose (a Developer Prompt, an activated venv), and the
//! registry's copy is the same list, older, with nothing to say about order.

use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};
#[cfg(not(windows))]
use std::time::Duration;
use std::time::Instant;

/// Where the PATH is read from, as the panel names it.
#[cfg(not(windows))]
pub const SOURCE: &str = "your login shell's PATH";
#[cfg(windows)]
pub const SOURCE: &str = "the PATH saved in your Windows settings";

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
#[cfg(not(windows))]
const TIMEOUT: Duration = Duration::from_secs(5);

/// Fences the answer off from whatever an interactive profile prints on its way
/// past. Long enough not to occur by accident in a MOTD.
#[cfg(any(not(windows), test))]
const BEGIN: &str = "__TAURUS_PATH_BEGIN__";
#[cfg(any(not(windows), test))]
const END: &str = "__TAURUS_PATH_END__";

/// What the probe found, once it has run.
static RESOLVED: OnceLock<Outcome> = OnceLock::new();

/// What the last [`rescan`] found, once one has run.
static RESCANNED: RwLock<Option<Outcome>> = RwLock::new(None);

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

/// Asks the login shell for its PATH again, and gives it to every program
/// started from now on.
///
/// Only ever adds: the shell's directories come first, in its order, and
/// nothing already searched is dropped. Blocks for as long as the shell takes,
/// up to [`TIMEOUT`], so call it off the async runtime.
pub fn rescan() -> Outcome {
    // Read, never run: running startup's probe here would write the process
    // environment with other threads alive, which is what this exists to avoid.
    let startup_added = RESOLVED.get().map(|o| o.added.clone()).unwrap_or_default();
    let before = current();
    let outcome = match skip_reason().map_or_else(ask, Err) {
        Err(reason) => Outcome {
            path: before,
            added: startup_added,
            skipped: Some(reason),
        },
        Ok(from_shell) => {
            let (path, new) = adopt_order(&from_shell, &before);
            if !new.is_empty() {
                tracing::info!(added = ?new, "PATH read again");
            }
            taurus_process::path::set(&path);
            // Everything the shell has contributed since launch, not only this
            // time, which is what the panel's "added" has always meant.
            let mut added = startup_added;
            for dir in new {
                if !added.contains(&dir) {
                    added.push(dir);
                }
            }
            Outcome {
                path,
                added,
                skipped: None,
            }
        }
    };
    if let Ok(mut last) = RESCANNED.write() {
        *last = Some(outcome.clone());
    }
    outcome
}

/// Gives a command the PATH read again, when there is one. See
/// [`taurus_process::path::apply`].
pub use taurus_process::path::apply;

/// The PATH read again, for a child built some other way (a pty's
/// `CommandBuilder`). `None` until a [`rescan`] has run.
pub fn replaced() -> Option<std::ffi::OsString> {
    taurus_process::path::replaced()
}

/// The latest answer: the last [`rescan`], else the startup one.
pub fn latest() -> Outcome {
    RESCANNED
        .read()
        .ok()
        .and_then(|last| last.clone())
        .unwrap_or_else(|| adopt().clone())
}

/// The PATH in force, whether or not [`adopt`] has run.
///
/// What a diagnostic should print: the question being answered is "where did
/// Taurus look", and that is the PATH a child gets regardless of how it got
/// there — the one read again, when there is one.
pub fn current() -> String {
    taurus_process::path::current()
        .to_string_lossy()
        .into_owned()
}

/// Every directory on the current PATH, for a panel that lists them.
pub fn entries() -> Vec<PathBuf> {
    std::env::split_paths(&taurus_process::path::current()).collect()
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
    let from_shell = match ask() {
        Ok(path) => path,
        Err(reason) => {
            return Outcome {
                path: before,
                added: Vec::new(),
                skipped: Some(reason),
            }
        }
    };

    let (path, added) = adopt_order(&from_shell, &before);
    tracing::info!(
        added = added.len(),
        ms = started.elapsed().as_millis(),
        "PATH adopted"
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
    None
}

/// The PATH as the user has it: the login shell's.
#[cfg(not(windows))]
fn ask() -> Result<String, String> {
    ask_shell()
}

/// The PATH as the user has it: the registry's.
#[cfg(windows)]
fn ask() -> Result<String, String> {
    ask_registry()
}

/// What [`ask`] found, put ahead of what this process has.
#[cfg(not(windows))]
fn adopt_order(found: &str, current: &str) -> (String, Vec<String>) {
    merge(found, current)
}

/// What [`ask`] found, put after what this process has. See the module docs.
#[cfg(windows)]
fn adopt_order(found: &str, current: &str) -> (String, Vec<String>) {
    append(current, found)
}

/// Reads the system and user PATH from the registry, the way a new Windows
/// Terminal tab gets it.
#[cfg(windows)]
fn ask_registry() -> Result<String, String> {
    use windows_registry::{Type, CURRENT_USER, LOCAL_MACHINE};

    const SYSTEM: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
    let strings = |key: &windows_registry::Key| -> Vec<(String, String)> {
        key.values()
            .map(|values| {
                values
                    .filter(|(_, v)| matches!(v.ty(), Type::String | Type::ExpandString))
                    .filter_map(|(name, v)| String::try_from(v).ok().map(|v| (name, v)))
                    .collect()
            })
            .unwrap_or_default()
    };
    let system = LOCAL_MACHINE
        .open(SYSTEM)
        .map(|k| strings(&k))
        .map_err(|e| format!("could not read the system PATH from the registry: {e}"))?;
    // An account with no variables of its own has no key, which is not a
    // failure: the system's PATH is still the whole answer.
    let user = CURRENT_USER
        .open("Environment")
        .map(|k| strings(&k))
        .unwrap_or_default();
    let path = registry_path(&system, &user, |name| std::env::var(name).ok());
    if path.is_empty() {
        return Err("the registry has no PATH".into());
    }
    Ok(path)
}

/// The PATH Windows would build from these registry values: the system's,
/// then the user's, each with its `%VARIABLES%` expanded.
///
/// A variable is looked up in the user's values, then the system's, then this
/// process's environment. The registry ones come first because they may be
/// newer than this process: a `%RUSTUP_HOME%` set by the same installer that
/// put it on PATH. The process comes last for `%USERPROFILE%` and
/// `%SystemRoot%`, which Windows sets per session and never saves.
#[cfg(any(windows, test))]
fn registry_path(
    system: &[(String, String)],
    user: &[(String, String)],
    process: impl Fn(&str) -> Option<String>,
) -> String {
    let find = |values: &[(String, String)], name: &str| {
        values
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let lookup = |name: &str| {
        // PATH inside PATH would expand to itself, or to the stale copy this
        // exists to replace. Windows leaves it as written, and so does this.
        if name.eq_ignore_ascii_case("path") {
            return None;
        }
        find(user, name)
            .or_else(|| find(system, name))
            .or_else(|| process(name))
    };
    [find(system, "Path"), find(user, "Path")]
        .into_iter()
        .flatten()
        .flat_map(|raw| {
            expand(&raw, lookup)
                .split(';')
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|dir| !dir.trim().is_empty())
        .collect::<Vec<_>>()
        .join(";")
}

/// `%NAME%` replaced by its value, as `ExpandEnvironmentStrings` does. A name
/// with no value is left as written, percent signs and all.
#[cfg(any(windows, test))]
fn expand(text: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        match (!name.is_empty()).then(|| lookup(name)).flatten() {
            Some(value) => {
                out.push_str(&value);
                rest = &after[end + 1..];
            }
            None => {
                // The closing sign may open the next name: `%UNSET%%HOME%`.
                out.push('%');
                out.push_str(name);
                rest = &after[end..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// What this process has, then whatever `found` has that it doesn't.
///
/// Windows compares directories without regard to case or a trailing
/// backslash, so `C:\Users\u\.cargo\bin\` is not a new one beside
/// `c:\users\u\.cargo\bin`. Said here rather than left to `PathBuf`, whose
/// comparison is by component and keeps case.
#[cfg(any(windows, test))]
fn append(current: &str, found: &str) -> (String, Vec<String>) {
    let key = |dir: &str| dir.trim_end_matches(['\\', '/']).to_lowercase();
    let mut ordered: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut added = Vec::new();
    let pieces = |s: &str| {
        s.split(';')
            .filter(|d| !d.trim().is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    for (dir, new) in pieces(current)
        .into_iter()
        .map(|d| (d, false))
        .chain(pieces(found).into_iter().map(|d| (d, true)))
    {
        if seen.contains(&key(&dir)) {
            continue;
        }
        seen.push(key(&dir));
        if new {
            added.push(dir.clone());
        }
        ordered.push(dir);
    }
    (ordered.join(";"), added)
}

/// Runs the login shell and reads the PATH it settles on.
#[cfg(not(windows))]
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
#[cfg(not(windows))]
fn run(shell: &str, args: &[&str]) -> Result<String, String> {
    run_with(shell, args, &[])
}

/// [`run`], with variables set for the shell on top of this process's own.
///
/// For the test that runs a real shell, which has to keep the developer's own
/// startup files out of it: a slow `~/.profile` fails that test on that one
/// machine, and a result that depends on whose laptop it ran on says nothing
/// about this code.
#[cfg(not(windows))]
fn run_with(
    shell: &str,
    args: &[&str],
    env: &[(&str, &std::ffi::OsStr)],
) -> Result<String, String> {
    use std::process::{Command, Stdio};

    let script = format!(r#"printf '{BEGIN}%s{END}' "$PATH""#);
    let mut child = Command::new(shell)
        .args(args)
        .arg(&script)
        .envs(env.iter().copied())
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
#[cfg(any(not(windows), test))]
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
#[cfg(any(not(windows), test))]
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

    /// Joins with whatever separates PATH entries on this platform.
    ///
    /// `:` on unix and `;` on Windows, and the merge is written in terms of
    /// `split_paths`/`join_paths` rather than either — so a test that hardcoded
    /// one would be asserting against the wrong string on the other platform
    /// while the code under it was correct.
    fn path(entries: &[&str]) -> String {
        std::env::join_paths(entries)
            .expect("test entries must be joinable")
            .to_string_lossy()
            .into_owned()
    }

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
        let (merged, added) = merge(
            &path(&["/home/u/.local/bin", "/usr/bin"]),
            &path(&["/usr/bin", "/sbin"]),
        );
        assert_eq!(merged, path(&["/home/u/.local/bin", "/usr/bin", "/sbin"]));
        assert_eq!(added, vec!["/home/u/.local/bin"]);
    }

    #[test]
    fn nothing_the_launcher_had_is_lost() {
        // The merge only ever adds. A shell whose PATH is missing a directory
        // the app was started with must not take it away — that would break the
        // case this is meant to fix in the other direction.
        let (merged, added) = merge("/opt/homebrew/bin", &path(&["/usr/bin", "/bin"]));
        assert_eq!(merged, path(&["/opt/homebrew/bin", "/usr/bin", "/bin"]));
        assert_eq!(added, vec!["/opt/homebrew/bin"]);
    }

    #[test]
    fn a_terminal_launch_adds_nothing() {
        // Started from a shell, the app already has the answer, and the probe
        // has nothing to contribute. Worth asserting because "added" is what the
        // panel shows, and a list of directories that were already there would
        // read as a fix that was needed.
        let both = path(&["/usr/bin", "/bin"]);
        let (merged, added) = merge(&both, &both);
        assert_eq!(merged, both);
        assert!(added.is_empty());
    }

    #[test]
    fn the_escape_hatch_stops_the_probe() {
        std::env::set_var(SKIP_ENV, "1");
        let reason = skip_reason().expect("setting the variable must skip the probe");
        std::env::remove_var(SKIP_ENV);
        assert!(reason.contains(SKIP_ENV), "{reason}");
    }

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_registry_path_is_the_systems_then_the_users_expanded() {
        // The case that started this: rustup writes `%USERPROFILE%\.cargo\bin`
        // into the user's Path, unexpanded, and `%USERPROFILE%` is in no
        // registry key at all — only in the session's environment.
        let system = vars(&[
            ("Path", r"%SystemRoot%\system32;%SystemRoot%"),
            ("windir", r"%SystemRoot%"),
        ]);
        let user = vars(&[("Path", r"%USERPROFILE%\.cargo\bin;;C:\tools")]);
        let process = |name: &str| match name {
            "SystemRoot" => Some(r"C:\Windows".to_string()),
            "USERPROFILE" => Some(r"C:\Users\u".to_string()),
            _ => None,
        };
        assert_eq!(
            registry_path(&system, &user, process),
            r"C:\Windows\system32;C:\Windows;C:\Users\u\.cargo\bin;C:\tools"
        );
    }

    #[test]
    fn a_variable_saved_since_launch_is_expanded_from_the_registry() {
        // An installer that sets CARGO_HOME and puts %CARGO_HOME%\bin on PATH
        // in the same breath. This process has neither; the registry has both.
        let user = vars(&[
            ("PATH", r"%cargo_home%\bin"),
            ("CARGO_HOME", r"D:\rust\cargo"),
        ]);
        assert_eq!(registry_path(&[], &user, |_| None), r"D:\rust\cargo\bin");
    }

    #[test]
    fn expansion_leaves_what_it_cannot_resolve_as_written() {
        let lookup = |name: &str| (name == "HOME").then(|| "h".to_string());
        assert_eq!(expand("%HOME%/x", lookup), "h/x");
        assert_eq!(expand("%UNSET%%HOME%", lookup), "%UNSET%h");
        assert_eq!(expand("50%", lookup), "50%");
        assert_eq!(expand("%%", lookup), "%%");
        // PATH inside PATH is not expanded: it would bring back the stale copy.
        assert_eq!(
            registry_path(&vars(&[("Path", "%PATH%;C:\\a")]), &[], |_| Some(
                "x".into()
            )),
            "%PATH%;C:\\a"
        );
    }

    #[test]
    fn appending_keeps_the_launchers_order_and_adds_only_the_missing() {
        // A Developer Prompt's toolchain first stays first. The cargo directory
        // differs only in case and a trailing backslash, so it is not new.
        let (merged, added) = append(
            r"C:\vs\bin;C:\Users\u\.cargo\bin\;C:\Windows",
            r"C:\Windows;c:\users\u\.cargo\bin;C:\Users\u\.local\bin",
        );
        assert_eq!(
            merged,
            r"C:\vs\bin;C:\Users\u\.cargo\bin\;C:\Windows;C:\Users\u\.local\bin"
        );
        assert_eq!(added, vec![r"C:\Users\u\.local\bin"]);
    }

    #[test]
    #[cfg(windows)]
    fn the_real_registry_has_a_path_with_system32_on_it() {
        // End to end against the machine's own registry, the way `ask` does
        // it: every Windows has System32 on the system PATH.
        let path = ask_registry().expect("the registry must answer");
        assert!(path.to_lowercase().contains(r"\system32"), "{path:?}");
        assert!(!path.contains("%SystemRoot%"), "expanded: {path:?}");
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
    fn asking_again_adds_what_was_installed_since_and_reaches_children() {
        // A stand-in for the user's shell that knows one directory more than
        // this process does — the state after `npm i -g` in a terminal. A
        // stand-in rather than the real one, so the developer's profile isn't
        // what's being tested.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let shell = dir.path().join("fake-shell");
        std::fs::write(
            &shell,
            format!("#!/bin/sh\nprintf '{BEGIN}%s{END}' \"/installed/later:$PATH\"\n"),
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();

        let before = std::env::var_os("SHELL");
        std::env::set_var("SHELL", &shell);
        let outcome = rescan();
        match before {
            Some(value) => std::env::set_var("SHELL", value),
            None => std::env::remove_var("SHELL"),
        }

        assert_eq!(outcome.skipped, None);
        assert!(
            outcome.added.contains(&"/installed/later".to_string()),
            "{outcome:?}"
        );
        assert!(
            entries().contains(&PathBuf::from("/installed/later")),
            "what children get"
        );
        assert_eq!(latest(), outcome, "what the panel shows");
        assert!(
            !std::env::var("PATH").unwrap().contains("/installed/later"),
            "the process environment is never written after startup"
        );
    }

    #[test]
    #[cfg(unix)]
    fn asking_a_real_shell_comes_back_with_a_real_path() {
        // The whole mechanism end to end, against the one shell every unix has.
        // If the fencing, the flags, or the script quoting are wrong, this is
        // where it shows — the alternative is finding out on a user's Dock.
        //
        // Under an empty home, so that it is this code being tested and not
        // the developer's profile: `-l` reads `~/.profile`, and one that is
        // slow or prints fails the test on that one machine.
        let home = tempfile::TempDir::new().unwrap();
        let path = run_with(
            "/bin/sh",
            &["-l", "-c"],
            &[
                ("HOME", home.path().as_os_str()),
                ("ENV", std::ffi::OsStr::new("")),
            ],
        )
        .expect("sh must answer");
        assert!(path.contains('/'), "{path:?}");
        assert!(
            !path.contains(BEGIN),
            "the markers must be stripped: {path:?}"
        );
    }
}

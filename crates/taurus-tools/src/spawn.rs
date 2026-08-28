//! Starting child processes without a console window appearing.
//!
//! A release build of the app sets `windows_subsystem = "windows"`, so it runs
//! with no console attached. On Windows, starting a console program from a
//! process that has no console makes the system allocate one — and it is a real
//! window, so every `run_command` flashed a black box on screen, and a slow one
//! left it sitting there. The output was still captured correctly; the window
//! was pure noise, and it stole focus while it was up.
//!
//! That attribute is `cfg_attr(not(debug_assertions))`, which is why this never
//! appeared in development: a debug build has a console of its own, the child
//! inherits it, and nothing new opens. It reproduces only in a release or
//! installed build.
//!
//! `CREATE_NO_WINDOW` is the flag that says "console program, no console
//! window". It is not the same as detaching: the child still gets its standard
//! handles, which is what keeps the pipes Taurus reads from working.
//!
//! Every place that starts a child *through `tokio::process`* goes through
//! here. A spawn site that forgets is invisible on the platforms most of this
//! is developed on, and shows up only as a flicker on somebody else's machine.
//!
//! **One child does not, and cannot: the pseudo-terminal.** `run_command` with
//! `pty: true` goes through `portable-pty`, which builds its own
//! `CreateProcessW` call and takes no `tokio::process::Command` to configure —
//! so this flag never reaches it, and on Windows a console window appears for
//! as long as the command runs. It is the same mechanism described above,
//! arriving through the one door this module cannot cover, and the qualifier in
//! the first line of this doc is load-bearing: the sentence used to claim every
//! spawn site, which is how the gap survived being written down. See
//! [`crate::builtin::pty`] and the known-gaps entry it points at.

/// `CREATE_NO_WINDOW` from the Win32 process creation flags.
///
/// Spelled out rather than pulled from a `windows-sys` dependency: it is one
/// stable constant, and the alternative is a platform-only crate in the tree
/// for a single number.
///
/// Defined on every platform so the tests below can check it anywhere. Only
/// Windows ever applies it.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Configures a command so starting it opens no console window.
///
/// A no-op off Windows, where the problem does not exist.
pub fn no_console(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Puts a child in a process group of its own.
///
/// `0` means "a new group, led by this child", and everything the child starts
/// inherits it. That is the whole point: without a group there is nothing to
/// signal but the child itself, and for a `sh -c "..."` the child is the shell
/// rather than the program somebody actually wanted stopped. Measured before
/// this existed — a hook whose script ran `sleep 47` left the `sleep` running
/// after the shell was killed, in every shape a script can be written in.
///
/// Only for children something will later end deliberately. A command left in
/// the parent's group is one a terminal's Ctrl-C reaches by itself, which is
/// worth more than a group for anything running in the foreground of a CLI.
///
/// A no-op off Unix, where the equivalent is a Job Object and a dependency
/// this workspace does not have. See `docs/known-gaps.md`.
pub fn own_group(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(unix)]
    command.process_group(0);
    command
}

/// Ends every process in the group `leader` heads.
///
/// Paired with [`own_group`], and only meaningful for a child given one. Safe
/// to call for a child that has already exited: the group is gone, the signal
/// finds nothing, and nothing here reports it.
///
/// `SIGKILL` rather than `SIGTERM`. Every path that calls this is one where
/// something has already been asked to stop and has not — a timeout that
/// elapsed, a Stop that was pressed — so the polite signal has in effect been
/// sent and ignored already.
///
/// # Why this shells out
///
/// Signalling a process group is `kill(-pgid, …)`, and there is no way to say
/// that in `std`. The two ways to reach it from Rust are an `unsafe` call into
/// libc, which this workspace forbids outright and does not weaken for one
/// line, or a crate wrapping the same call — a platform-only dependency for a
/// single function, which is the trade [`no_console`] above already declines
/// for a single constant.
///
/// So it runs `kill`, which is POSIX, is on every machine this branch compiles
/// for, and takes a negative pid to mean the group. It costs a fork, and it
/// costs one only on a path where something has already hung or been stopped
/// by hand — never in the ordinary life of a command.
///
/// Best-effort by design, and deliberately *not* the only kill: every caller
/// still ends the child itself through `start_kill` or `kill_on_drop`. This
/// reaches what the child started, so a machine without a `kill` binary is
/// left exactly where it was before this existed rather than worse off.
pub async fn kill_group(leader: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = leader {
        let mut command = tokio::process::Command::new("kill");
        command.args(["-KILL", &format!("-{pid}")]);
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::null());
        command.stderr(std::process::Stdio::null());
        // Awaited rather than detached, so the helper is reaped here instead of
        // becoming something the runtime has to tidy up later. It is a signal
        // and an exit; it does not outlive this call by anything measurable.
        let _ = command.status().await;
    }
    #[cfg(not(unix))]
    let _ = leader;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_is_the_documented_win32_value() {
        // The whole fix is this number. A typo in it would compile, run, and
        // keep flashing a window on the one platform the test suite here cannot
        // observe — so the constant is asserted rather than trusted.
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
        assert_eq!(CREATE_NO_WINDOW, 134_217_728);
    }

    #[tokio::test]
    async fn a_configured_command_still_runs_and_still_pipes() {
        // The failure worth guarding against is a flag that suppresses the
        // window by breaking the child's standard handles, which would take the
        // captured output with it.
        let mut command = tokio::process::Command::new(if cfg!(windows) { "cmd" } else { "echo" });
        if cfg!(windows) {
            command.args(["/C", "echo hello"]);
        } else {
            command.arg("hello");
        }
        no_console(&mut command);

        let output = command.output().await.expect("the command must still run");
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
    }
}

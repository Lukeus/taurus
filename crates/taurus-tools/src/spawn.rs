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

/// The command that ends a process tree, per platform.
///
/// Returned as data rather than run here, and taking the platform as an
/// argument rather than reading `cfg!`, for the reason [`CREATE_NO_WINDOW`]
/// above is defined everywhere: the tests can then check both spellings on
/// whichever machine happens to be running them. Windows-only code is
/// otherwise never compiled on the machines this is developed on, so a typo in
/// it is invisible until CI — and a wrong argument to a kill is invisible even
/// then, because the call still succeeds and simply kills nothing.
///
/// Unix signals the process *group*, which is why the pid is negated and why
/// [`own_group`] had to put the child in one. Windows has no such thing to
/// signal: `taskkill /T` walks the parent-child tree instead, which is also why
/// it has to run while the parent is still alive.
///
/// # Why the `--`
///
/// Because without it, on Linux, this kills the wrong thing. `kill -KILL -123`
/// reads to procps as two signal options and no pid, and what it does then is
/// signal *its own process group* — so the runaway tree lives and the caller
/// dies. Measured on Ubuntu with procps-ng 3.3.17: eleven of twelve process
/// groups survived, each time with the `kill` itself dead of the signal it had
/// been asked to send. The one that lived through it had a single-digit pgid,
/// which is the sort of luck a developer machine has and a CI runner does not.
///
/// `--` ends option parsing, so what follows is a pid whatever it looks like.
/// Twelve of twelve, and unchanged on macOS, where the bare form worked.
///
/// # Why a pid can be refused
///
/// `None` for a pid that does not name a tree, and this is the one guard in
/// this module that is not tidiness. Negating the pid is what asks for the
/// group, so the arithmetic has to hold: `-0` is `0`, which on Unix means
/// *this process's own group*, and `-1` means **every process the user owns**.
/// Anything above `i32::MAX` gets there by truncation — `u32::MAX` arrives at
/// `kill(2)` as `-1`, because procps parses the argument into a `pid_t` and
/// wraps.
///
/// Measured, not reasoned about: `kill -KILL -4294967295` on Ubuntu SIGKILLs
/// the whole session, uninvolved processes included. That is what took out
/// three CI runs — the job did not fail, it stopped existing, so it hung with
/// no logs until it was cancelled by hand. macOS never showed it: BSD `kill`
/// rejects the out-of-range pid and does nothing.
///
/// So the range is checked here, in the one place that spells the command,
/// rather than at each caller.
pub fn kill_command(pid: u32, windows: bool) -> Option<(&'static str, Vec<String>)> {
    // 0 is this group, 1 is init and reads as "everything", and past `i32::MAX`
    // the negation wraps into one of those two.
    if !(2..=i32::MAX as u32).contains(&pid) {
        return None;
    }
    Some(if windows {
        // Target first, then the switches. That is the order `taskkill`'s own
        // grammar documents — `{/PID pid | /IM name} [/T] [/F]` — and with
        // `/T` ahead of `/PID` it killed the named process and walked no tree
        // at all: measured on a runner, all three children left standing with
        // their parent links intact.
        (
            "taskkill",
            vec!["/PID".into(), pid.to_string(), "/T".into(), "/F".into()],
        )
    } else {
        // `--` is load-bearing. See above.
        ("kill", vec!["-KILL".into(), "--".into(), format!("-{pid}")])
    })
}

/// Ends `leader` and everything it started.
///
/// Safe to call for a process that has already exited: the tree is gone, the
/// command finds nothing, and nothing here reports it.
///
/// `SIGKILL`, and `/F`. Every path that calls this is one where something has
/// already been asked to stop and has not — a timeout that elapsed, a Stop
/// that was pressed — so the polite signal has in effect been sent and ignored
/// already.
///
/// # Why this shells out
///
/// Neither platform's mechanism is reachable from `std`. Signalling a process
/// group is `kill(-pgid, …)`; killing a tree on Windows is a Job Object. Both
/// mean either an `unsafe` call, which this workspace forbids outright and does
/// not weaken for one line, or a platform-only crate for a single function —
/// the trade [`no_console`] above already declines for a single constant.
///
/// So it runs the tool each platform ships for exactly this. It costs a fork,
/// and only on a path where something has already hung or been stopped by
/// hand — never in the ordinary life of a command.
///
/// Best-effort by design, and deliberately *not* the only kill: every caller
/// still ends the child itself through `start_kill` or `kill_on_drop`. This
/// reaches what the child started, so a machine missing the tool is left where
/// it was rather than worse off.
pub async fn kill_tree(leader: Option<u32>) {
    if let Err(trouble) = try_kill_tree(leader).await {
        // A tree kill that quietly fails looks exactly like one that had
        // nothing to do, which is how a `/T` that walked nothing survived
        // three rounds of CI.
        tracing::warn!("{trouble}");
    }
}

/// [`kill_tree`], with what went wrong if anything did.
async fn try_kill_tree(leader: Option<u32>) -> Result<(), String> {
    let Some(pid) = leader else {
        return Ok(());
    };
    // A pid that does not name a tree is not signalled at all, rather than
    // signalled and hoped about. See [`kill_command`] for what the arithmetic
    // turns those into.
    let Some((program, args)) = kill_command(pid, cfg!(windows)) else {
        return Err(format!(
            "{pid} does not name a process tree, so nothing was signalled"
        ));
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(&args)
        .stdin(std::process::Stdio::null())
        // Captured rather than discarded. Both tools report what they could
        // not do on their own output, and it is the only account of a kill
        // that ran and reached nothing.
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // So the helper does not flash a console of its own on Windows, which is
    // the very thing this module exists for.
    no_console(&mut command);
    // Awaited rather than detached, so it is reaped here instead of becoming
    // something the runtime has to tidy up later. It is a signal and an exit.
    let out = command
        .output()
        .await
        .map_err(|e| format!("could not run {program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    // A tree that had already exited is the ordinary case and reports itself
    // here as a failure, so the message says what was asked rather than
    // claiming something is wrong.
    let said = [&out.stderr, &out.stdout]
        .into_iter()
        .map(|s| String::from_utf8_lossy(s).trim().to_string())
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "and said nothing".into());
    Err(format!(
        "{program} {} exited {:?}: {said}",
        args.join(" "),
        out.status.code()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kill_is_spelled_the_way_each_platform_spells_it() {
        /*
         * Both arms are checked from whichever machine is running this,
         * because neither is checked by the compiler on the other one. The
         * failure being guarded against is not a build error — it is a kill
         * that runs, succeeds, and kills nothing, which looks exactly like a
         * tree that had already exited.
         */
        let (program, args) = kill_command(4321, false).expect("an ordinary pid");
        assert_eq!(program, "kill");
        // Negative pid is the process group, which is the whole reason the
        // child was given one — and `--` is what stops procps from reading it
        // as a signal and killing this process's group instead.
        assert_eq!(args, ["-KILL", "--", "-4321"]);

        let (program, args) = kill_command(4321, true).expect("an ordinary pid");
        assert_eq!(program, "taskkill");
        // `/T` is the tree and `/F` is force, and both come *after* the target:
        // ahead of `/PID` the tree walk silently does not happen.
        assert_eq!(args, ["/PID", "4321", "/T", "/F"]);
    }

    #[test]
    fn a_pid_that_would_negate_into_something_larger_is_refused() {
        /*
         * Checked here and never run, and the distinction matters more than
         * usual: this test cannot assert by calling `kill_tree`, because the
         * assertion would be the bug. `kill -KILL -4294967295` on Linux
         * truncates to `kill(-1, SIGKILL)` — every process the user owns —
         * and it is what killed three CI runners from inside a test that was
         * written to prove killing something already gone is quiet.
         *
         * So the guard is asserted on the spelling, which is pure, and the
         * only pids that ever reach a real `kill` are ones this accepted.
         */
        for platform in [false, true] {
            // This process's own group.
            assert!(kill_command(0, platform).is_none());
            // `-1`: everything.
            assert!(kill_command(1, platform).is_none());
            // Wraps to `-1` in a `pid_t`.
            assert!(kill_command(u32::MAX, platform).is_none());
            assert!(kill_command(i32::MAX as u32 + 1, platform).is_none());

            // And the ends of the range that is real.
            assert!(kill_command(2, platform).is_some());
            assert!(kill_command(i32::MAX as u32, platform).is_some());
        }
    }

    #[tokio::test]
    async fn killing_a_tree_that_is_already_gone_is_quiet() {
        // The ordinary case on every successful command: by the time anything
        // asks, there is nothing left to kill. A pid is deliberately not
        // invented for this — see the test above for why an out-of-range one
        // must never be handed to a real `kill`.
        kill_tree(None).await;
    }

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

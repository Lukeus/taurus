//! A child that is ended together with everything it started.

use std::io;
use std::process::ExitStatus;

use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};

#[cfg(windows)]
use process_wrap::tokio::ChildWrapper;

/// A child process, started so that it can be ended together with everything
/// it starts.
///
/// For a child something will later end on purpose: a hook that hits its
/// timeout, a background command that is stopped. Both are nearly always a
/// shell, so the child is `/bin/sh` or `cmd.exe` and the work is *its* child —
/// ending the child alone would leave the linter, the build or the watcher
/// running while the app said it had stopped. Measured before any of this
/// existed: a hook whose script ran `sleep 47` left the `sleep` running after
/// the shell was killed, in every shape a script can be written in.
///
/// # Unix: a process group
///
/// The child leads a group of its own, and everything it starts inherits the
/// group. [`end`](Self::end) signals the group, which reaches a grandchild whose
/// parent has already exited, because a group outlives its leader. What
/// escapes is a process that leaves on purpose: `setsid`, or a daemon's double
/// fork.
///
/// # Windows: a Job Object
///
/// Windows has no group to signal. Walking the parent links down from the
/// child — which is what `taskkill /T` does — cannot reach a process whose
/// parent has already exited, and that is the shape an npm `.cmd` shim leaves
/// every time it runs. A Job Object has no such hole. The child is started
/// suspended, put in a job of its own, and only then let go, so there is no
/// moment in which it can start something the job does not hold; everything it
/// starts after that is in the job whatever becomes of its parents, and
/// [`end`](Self::end) ends the job. What escapes is anything started outside
/// the tree on the child's behalf — a scheduled task, a service.
///
/// Suspending, assigning and resuming a process are `unsafe` Win32 calls, which
/// this workspace forbids in its own code, so the job comes from
/// `process-wrap` — the crate `watchexec` ends its trees with.
///
/// # Dropping one
///
/// Ends the child itself, through `kill_on_drop`, and not the tree. The same on
/// both platforms, so a hook or command that leaves something running and exits
/// on its own is treated the same way on each: only [`end`](Self::end) reaches
/// past the child.
pub struct Tree {
    #[cfg(unix)]
    child: tokio::process::Child,
    /// The group's id, which is the child's pid. Read at the start because
    /// tokio forgets a child's pid once it has been waited on, and the group
    /// can still hold everything the child started after that — a hook whose
    /// shell has exited while something it started holds its stdout open.
    #[cfg(unix)]
    group: Option<u32>,
    #[cfg(windows)]
    child: Box<dyn ChildWrapper>,
}

#[cfg(unix)]
impl Tree {
    /// Starts `command` as the leader of a process group of its own.
    ///
    /// Sets `kill_on_drop` on it; see "Dropping one" above.
    pub fn spawn(mut command: Command) -> io::Result<Self> {
        command.kill_on_drop(true);
        // `0` is "a new group, led by this child".
        command.process_group(0);
        let child = command.spawn()?;
        let group = child.id();
        Ok(Self { child, group })
    }

    /// The child's stdin, if it was piped and has not been taken already.
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    /// The child's stdout, if it was piped and has not been taken already.
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    /// The child's stderr, if it was piped and has not been taken already.
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    /// Waits for the child itself to exit.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }

    /// Ends the child and everything it started, and says what went wrong if
    /// anything did.
    ///
    /// `SIGKILL`. Every path that calls this is one where something has already
    /// been asked to stop and has not — a timeout that elapsed, a Stop that was
    /// pressed — so the polite signal has in effect been sent and ignored
    /// already.
    ///
    /// A tree that had already exited reports itself as a failure here, with
    /// what `kill` said, so the message says what was asked rather than
    /// claiming something is wrong.
    pub async fn end(&mut self) -> Result<(), String> {
        let group = kill_group(self.group).await;
        // The leader as well, which the signal to its group has nearly always
        // reached already. This is for a machine where `kill` itself could not
        // be run, which is then left where it was rather than worse off.
        let _ = self.child.start_kill();
        group
    }
}

#[cfg(windows)]
impl Tree {
    /// Starts `command` suspended, puts it in a Job Object of its own, and
    /// only then lets it run.
    ///
    /// Sets `kill_on_drop` on it; see "Dropping one" above. The creation flags
    /// are this function's: whatever the command carried is replaced, and
    /// `CREATE_NO_WINDOW` is always among them.
    pub fn spawn(mut command: Command) -> io::Result<Self> {
        use process_wrap::tokio::{CommandWrap, CreationFlags, JobObject};
        use windows::Win32::System::Threading::PROCESS_CREATION_FLAGS;

        command.kill_on_drop(true);
        let child = CommandWrap::from(command)
            // Through this wrapper rather than `no_console`, and before
            // `JobObject`. `JobObject` sets the creation flags itself — it has
            // to, to start the child suspended — and in doing so replaces
            // whatever the command carried, so a `no_console` applied to the
            // command is lost and every hook opens a console window. Flags
            // given this way it folds into its own, but only if this wrapper
            // runs first: the other order overwrites its `CREATE_SUSPENDED`,
            // and the child runs before it is in the job.
            .wrap(CreationFlags(PROCESS_CREATION_FLAGS(
                crate::CREATE_NO_WINDOW,
            )))
            .wrap(JobObject)
            .spawn()?;
        Ok(Self { child })
    }

    /// The child's stdin, if it was piped and has not been taken already.
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin().take()
    }

    /// The child's stdout, if it was piped and has not been taken already.
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout().take()
    }

    /// The child's stderr, if it was piped and has not been taken already.
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr().take()
    }

    /// Waits for the child itself to exit.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }

    /// Ends the child and everything it started, and says what went wrong if
    /// anything did.
    ///
    /// Terminates the job, which ends every process in it at once — no tree
    /// to walk, no parent that has to still be alive, and nothing to start
    /// first. A job whose processes have all exited already is ended without
    /// complaint.
    pub async fn end(&mut self) -> Result<(), String> {
        self.child
            .start_kill()
            .map_err(|e| format!("could not end the Job Object the process tree is in: {e}"))
    }
}

/// The command that signals a whole process group, per the Unix `kill`.
///
/// Returned as data rather than run here so the tests can check it on any
/// machine, since a wrong argument to a kill is invisible even on the platform
/// it is wrong for: the call succeeds and reaches nothing — or reaches
/// something else.
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
/// `None` for a pid that does not name a group, and this is the one guard here
/// that is not tidiness. Negating the pid is what asks for the group, so the
/// arithmetic has to hold: `-0` is `0`, which means *this process's own
/// group*, and `-1` means **every process the user owns**. Anything above
/// `i32::MAX` gets there by truncation — `u32::MAX` arrives at `kill(2)` as
/// `-1`, because procps parses the argument into a `pid_t` and wraps.
///
/// Measured, not reasoned about: `kill -KILL -4294967295` on Ubuntu SIGKILLs
/// the whole session, uninvolved processes included. That is what took out
/// three CI runs — the job did not fail, it stopped existing, so it hung with
/// no logs until it was cancelled by hand. macOS never showed it: BSD `kill`
/// rejects the out-of-range pid and does nothing.
#[cfg(any(unix, test))]
fn kill_command(pid: u32) -> Option<(&'static str, Vec<String>)> {
    // 0 is this group, 1 is init and reads as "everything", and past `i32::MAX`
    // the negation wraps into one of those two.
    if !(2..=i32::MAX as u32).contains(&pid) {
        return None;
    }
    // `--` is load-bearing. See above.
    Some(("kill", vec!["-KILL".into(), "--".into(), format!("-{pid}")]))
}

/// Signals the group `group` leads, and says what went wrong if anything did.
///
/// Shells out, because signalling a group is `kill(-pgid, …)` and `std` does
/// not offer it: the alternatives are an `unsafe` call, which this workspace
/// forbids, or a crate for one function. So it runs the `kill` every Unix
/// ships. It costs a fork, and only on a path where something has already hung
/// or been stopped by hand — never in the ordinary life of a command.
#[cfg(unix)]
async fn kill_group(group: Option<u32>) -> Result<(), String> {
    use std::process::Stdio;

    let Some(pid) = group else {
        return Ok(());
    };
    // A pid that does not name a group is not signalled at all, rather than
    // signalled and hoped about. See [`kill_command`].
    let Some((program, args)) = kill_command(pid) else {
        return Err(format!(
            "{pid} does not name a process group, so nothing was signaled"
        ));
    };
    // Awaited rather than detached, so it is reaped here instead of becoming
    // something the runtime has to tidy up later.
    let out = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        // Captured rather than discarded. `kill` reports what it could not do
        // on its own output, and that is the only account of a kill that ran
        // and reached nothing.
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("could not run {program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
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
    use std::process::Stdio;
    use std::time::Duration;

    use tokio::io::AsyncReadExt;

    use super::*;

    /// A shell running `line`, piped, the way every caller starts one.
    fn shell(unix: &str, windows: &[&str]) -> Command {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.arg("/C").args(windows);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", unix]);
            command
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    #[test]
    fn the_group_kill_is_spelled_with_its_dash_dash() {
        /*
         * Checked from whichever machine is running this. The failure being
         * guarded against is not a build error — it is a kill that runs,
         * succeeds, and signals the wrong group, which looks exactly like a
         * tree that had already exited until Taurus itself is the one killed.
         */
        let (program, args) = kill_command(4321).expect("an ordinary pid");
        assert_eq!(program, "kill");
        // Negative pid is the process group, which is the whole reason the
        // child was given one — and `--` is what stops procps from reading it
        // as a signal and killing this process's group instead.
        assert_eq!(args, ["-KILL", "--", "-4321"]);
    }

    #[test]
    fn a_pid_that_would_negate_into_something_larger_is_refused() {
        /*
         * Checked here and never run, and the distinction matters more than
         * usual: this cannot be asserted by calling `kill_group`, because the
         * assertion would be the bug. `kill -KILL -4294967295` on Linux
         * truncates to `kill(-1, SIGKILL)` — every process the user owns —
         * and it is what killed three CI runners from inside a test that was
         * written to prove killing something already gone is quiet.
         */
        // This process's own group.
        assert!(kill_command(0).is_none());
        // `-1`: everything.
        assert!(kill_command(1).is_none());
        // Wraps to `-1` in a `pid_t`.
        assert!(kill_command(u32::MAX).is_none());
        assert!(kill_command(i32::MAX as u32 + 1).is_none());

        // And the ends of the range that is real.
        assert!(kill_command(2).is_some());
        assert!(kill_command(i32::MAX as u32).is_some());
    }

    #[tokio::test]
    async fn a_tree_still_pipes() {
        // On Windows this is the check that the creation flags `spawn` sets
        // itself — suspended, then resumed, and no console — leave a child
        // that runs at all and whose standard handles still work.
        let mut tree = Tree::spawn(shell("echo hello", &["echo", "hello"])).expect("it must start");
        let mut out = String::new();
        tree.take_stdout()
            .expect("stdout was piped")
            .read_to_string(&mut out)
            .await
            .unwrap();
        assert!(tree.wait().await.unwrap().success());
        assert!(out.contains("hello"), "{out:?}");
    }

    #[tokio::test]
    async fn a_tree_reports_its_own_exit_code() {
        let mut tree = Tree::spawn(shell("exit 3", &["exit", "3"])).expect("it must start");
        assert_eq!(tree.wait().await.unwrap().code(), Some(3));
    }

    #[tokio::test]
    async fn ending_a_tree_ends_its_child_and_says_so_plainly() {
        // The child alone is the smallest tree there is. What a grandchild
        // needs is asserted where it matters, in the hook and background
        // command tests that stop one.
        let mut tree = Tree::spawn(shell("sleep 30", &["ping", "-n", "31", "127.0.0.1"]))
            .expect("it must start");
        assert_eq!(tree.end().await, Ok(()));
        let status = tokio::time::timeout(Duration::from_secs(10), tree.wait())
            .await
            .expect("the child was still running ten seconds after it was ended")
            .unwrap();
        assert!(!status.success());
    }
}

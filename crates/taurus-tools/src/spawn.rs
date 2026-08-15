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
//! Every place that starts a child goes through here. A spawn site that forgets
//! is invisible on the platforms most of this is developed on, and shows up
//! only as a flicker on somebody else's machine.

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

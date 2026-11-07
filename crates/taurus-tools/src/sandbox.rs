//! Refusing writes to a short list of paths, at the operating system.
//!
//! A command let through by a standing grant runs as the user, and a grant for
//! `python3` covers whatever `build.py` does. [`crate::protected`] looks
//! afterwards and repairs. This stops the write happening, for the paths where
//! that's possible without getting in the way.
//!
//! It's a write deny-list and not a sandbox in the full sense. Reads and the
//! network are open, because `git push` needs `~/.ssh`. Anything not on the list
//! is writable, and code runs with the user's rights.
//!
//! - macOS: `sandbox-exec`, which is deprecated and works. It matches paths as
//!   the kernel resolves them, so a symlink, a hard link and `.GIT/HOOKS` are all
//!   caught, and a path that doesn't exist yet can't be created.
//! - Linux: `bwrap`, when it's installed and allowed to run. A read-only bind
//!   needs something to bind over, so **a path that doesn't exist yet isn't
//!   protected there**, and a parent can be renamed out from under one. The look
//!   afterwards still covers what's in its set.
//! - Windows: nothing.
//!
//! The look afterwards stays on either way.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Where a person's own startup and credentials live, relative to their home.
///
/// With [`denied_paths`], the one reviewed list. Refusing a write here costs
/// something, so each entry is a place where a write runs code later, or hands
/// over an identity.
///
/// `.gitconfig` and `.config/git/config` aren't here, and neither is `.git/config`
/// in [`denied_paths`]. `git config user.name` and `git remote add` write them,
/// and a sandbox can't tell which setting changed. Those stay with
/// [`crate::protected`], which can.
const SENSITIVE_HOME: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".zshrc",
    ".zshenv",
    ".zprofile",
    ".zlogin",
    ".bashrc",
    ".bash_profile",
    ".bash_login",
    ".profile",
    ".config/fish",
    "Library/LaunchAgents",
    ".config/autostart",
    ".config/systemd/user",
];

/// What turns this off.
pub const OFF_ENV: &str = "TAURUS_NO_SANDBOX";

/// Whether `value`, from [`OFF_ENV`], asks for it off.
fn says_off(value: Option<&str>) -> bool {
    value.is_some_and(|v| !matches!(v.trim(), "" | "0" | "false" | "no" | "off"))
}

/// Whether the person has turned this off.
fn turned_off() -> bool {
    says_off(std::env::var(OFF_ENV).ok().as_deref())
}

/// Everything a covered command may not write, unresolved.
///
/// In the workspace: each git directory's `hooks` (a submodule's too), the
/// `.taurus` files that hand over capability, and `.taurus/plugins`. Outside it:
/// the config home in full, and [`SENSITIVE_HOME`]. A workspace kept inside the
/// config home is somebody writing a plugin, so there only the two global files
/// that grant things are on the list.
///
/// A repository that doesn't exist yet isn't listed, so `git init` and `git clone`
/// can make their hooks.
fn denied_paths(
    root: &Path,
    git_dirs: &[PathBuf],
    config_home: Option<&Path>,
    user_home: Option<&Path>,
) -> Vec<PathBuf> {
    fn git(dir: &Path, into: &mut Vec<PathBuf>, depth: usize) {
        into.push(dir.join("hooks"));
        if depth == 0 {
            return;
        }
        if let Ok(modules) = std::fs::read_dir(dir.join("modules")) {
            for module in modules.flatten() {
                git(&module.path(), into, depth - 1);
            }
        }
    }
    let mut denied = Vec::new();
    for dir in git_dirs {
        git(dir, &mut denied, 4);
    }
    let taurus = root.join(".taurus");
    for file in crate::protected::TAURUS_FILES {
        denied.push(taurus.join(file));
    }
    denied.push(taurus.join("plugins"));
    if let Some(home) = config_home {
        if root.starts_with(home) {
            denied.push(home.join("permissions.json"));
            denied.push(home.join("hooks.json"));
        } else {
            denied.push(home.to_path_buf());
        }
    }
    if let Some(user) = user_home {
        denied.extend(SENSITIVE_HOME.iter().map(|name| user.join(name)));
    }
    denied
}

/// How a command gets started under the list.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Backend {
    Seatbelt,
    Bubblewrap(PathBuf),
}

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Whether a trivial command starts under a backend, asked once for the process.
/// This is what tells a machine that can't from a command that failed.
fn backend() -> &'static Result<Backend, String> {
    static BACKEND: OnceLock<Result<Backend, String>> = OnceLock::new();
    BACKEND.get_or_init(|| {
        let tried = |program: &Path, args: &[&str]| {
            std::process::Command::new(program)
                .args(args)
                .stdin(std::process::Stdio::null())
                .output()
                .map_err(|e| e.to_string())
                .and_then(|out| {
                    if out.status.success() {
                        Ok(())
                    } else {
                        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
                    }
                })
        };
        if cfg!(target_os = "macos") {
            tried(
                Path::new(SANDBOX_EXEC),
                &["-p", "(version 1)(allow default)", "/usr/bin/true"],
            )
            .map(|()| Backend::Seatbelt)
            .map_err(|e| format!("sandbox-exec won't start here: {e}"))
        } else if cfg!(target_os = "linux") {
            let found = ["/usr/bin/bwrap", "/usr/local/bin/bwrap", "/bin/bwrap"]
                .iter()
                .map(PathBuf::from)
                .find(|path| path.is_file())
                .or_else(|| {
                    std::env::split_paths(&std::env::var_os("PATH")?)
                        .map(|dir| dir.join("bwrap"))
                        .find(|path| path.is_file())
                })
                .ok_or_else(|| "bubblewrap (bwrap) isn't installed".to_string())?;
            tried(
                &found,
                &[
                    "--dev-bind",
                    "/",
                    "/",
                    "--die-with-parent",
                    "--",
                    "/bin/true",
                ],
            )
            .map(|()| Backend::Bubblewrap(found))
            .map_err(|e| {
                format!("bwrap won't start here, usually because user namespaces are off: {e}")
            })
        } else {
            Err("there's no sandbox for this platform yet".to_string())
        }
    })
}

/// The list, ready to put a command under.
#[derive(Debug)]
pub struct Fence {
    backend: Backend,
    denied: Vec<PathBuf>,
    /// Folders refused whole, apart from [`Self::except`]: the parent's
    /// workspace, for a delegate working in a copy of it. See [`Fence::ask`].
    kept_out: Vec<PathBuf>,
    /// The git directories inside [`Self::kept_out`] a command in the copy has
    /// to write to: a worktree's commits go into the repository it came from.
    /// Their hooks are still on [`Self::denied`].
    except: Vec<PathBuf>,
    /// Short names to look for in an error, to tell this from any other refusal.
    names: Vec<String>,
}

/// What came of asking for a [`Fence`].
pub(crate) enum Asked {
    Fenced(Fence),
    /// No sandbox is available, and this is why.
    Unavailable(String),
    /// The person turned it off, which needs no saying.
    TurnedOff,
}

/// Each path as it's written and as it resolves, sorted with no repeats.
///
/// `/tmp` and `/var` are symlinks on macOS, and `.zshrc` is often one into a
/// dotfiles repository, and it's the resolved one a sandbox sees.
fn resolved(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut all = Vec::new();
    for path in paths {
        if let Some(real) = crate::path_guard::canonical_where_it_can(&path) {
            all.push(real);
        }
        all.push(path);
    }
    all.sort();
    all.dedup();
    all
}

/// A path as a string in a sandbox profile.
fn quoted(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The profile for `sandbox-exec`.
///
/// A write anywhere at or under a listed path is refused, which includes creating
/// it. Renaming or removing any folder above one is refused too, because `mv .git
/// g` followed by writing `g/hooks/x` and moving it back reaches the same file by
/// a route no path rule sees. That's `file-write-unlink` alone, so creating
/// something in those folders, or touching them, still works.
///
/// A folder kept out is refused the same way, apart from the git directories
/// excepted inside it, through `require-not`.
fn profile(denied: &[PathBuf], kept_out: &[PathBuf], except: &[PathBuf]) -> String {
    let mut ancestors: Vec<&Path> = denied
        .iter()
        .chain(kept_out)
        .flat_map(|path| path.ancestors().skip(1))
        .filter(|path| path.parent().is_some())
        .collect();
    ancestors.sort();
    ancestors.dedup();
    let mut subpaths: String = denied
        .iter()
        .map(|path| format!(" (subpath {})", quoted(path)))
        .collect();
    for folder in kept_out {
        let unless: String = except
            .iter()
            .filter(|dir| dir.starts_with(folder))
            .map(|dir| format!(" (require-not (subpath {}))", quoted(dir)))
            .collect();
        subpaths.push_str(&format!(
            " (require-all (subpath {}){unless})",
            quoted(folder)
        ));
    }
    let literals: String = ancestors
        .iter()
        .map(|path| format!(" (literal {})", quoted(path)))
        .collect();
    format!(
        "(version 1)(allow default)(deny file-write*{subpaths})(deny file-write-unlink{literals})"
    )
}

/// The arguments for `bwrap`. Only what exists can be bound over, and a listed
/// path inside another is covered by that one.
///
/// A folder kept out is bound read-only first, its excepted git directories are
/// bound writable over that, and the list goes on top of both, so a hook inside
/// an excepted directory is read-only again.
fn bubblewrap_args(denied: &[PathBuf], kept_out: &[PathBuf], except: &[PathBuf]) -> Vec<String> {
    let mut args: Vec<String> = ["--dev-bind", "/", "/"].map(str::to_owned).to_vec();
    let exists =
        |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|m| !m.file_type().is_symlink());
    for folder in kept_out.iter().filter(|p| exists(p)) {
        let text = folder.to_string_lossy().into_owned();
        args.extend(["--ro-bind".to_string(), text.clone(), text]);
        for dir in except.iter().filter(|d| d.starts_with(folder) && exists(d)) {
            let text = dir.to_string_lossy().into_owned();
            args.extend(["--bind".to_string(), text.clone(), text]);
        }
    }
    let mut bound: Vec<&Path> = Vec::new();
    for path in denied {
        let plain = std::fs::symlink_metadata(path).is_ok_and(|m| !m.file_type().is_symlink());
        if plain && !bound.iter().any(|done| path.starts_with(done)) {
            bound.push(path);
            let text = path.to_string_lossy().into_owned();
            args.extend(["--ro-bind".to_string(), text.clone(), text]);
        }
    }
    args.push("--die-with-parent".to_string());
    args.push("--".to_string());
    args
}

impl Fence {
    /// The fence for a command in `root`, when one can be had.
    ///
    /// `keep_out` is the parent's workspace, for a command run by a delegate in
    /// a copy of it. The file tools already can't reach it, and this is what
    /// stops a granted command reaching it instead: `cp` and `>` don't go
    /// through a path check. The repository's git directories inside it stay
    /// writable, since a worktree's objects and refs live there.
    pub(crate) fn ask(
        root: &Path,
        git_dirs: &[PathBuf],
        config_home: Option<&Path>,
        keep_out: Option<&Path>,
    ) -> Asked {
        if turned_off() {
            return Asked::TurnedOff;
        }
        let backend = match backend() {
            Ok(backend) => backend.clone(),
            Err(why) => return Asked::Unavailable(why.clone()),
        };
        let user_home = config_home
            .filter(|home| home.file_name().is_some_and(|name| name == ".taurus"))
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
        // Never the folder the command runs in, or one it's inside: a copy
        // somebody put under the workspace would refuse itself everything.
        let kept_out: Vec<PathBuf> = keep_out
            .filter(|folder| !root.starts_with(folder))
            .map(Path::to_path_buf)
            .into_iter()
            .collect();
        Asked::Fenced(Self::over(
            backend,
            denied_paths(root, git_dirs, config_home, user_home.as_deref()),
            kept_out,
            git_dirs.to_vec(),
        ))
    }

    fn over(
        backend: Backend,
        denied: Vec<PathBuf>,
        kept_out: Vec<PathBuf>,
        except: Vec<PathBuf>,
    ) -> Self {
        let denied = resolved(denied);
        let kept_out = resolved(kept_out);
        let except = resolved(except);
        let mut names: Vec<String> = denied
            .iter()
            .chain(&kept_out)
            .filter_map(|path| path.file_name())
            .map(|name| name.to_string_lossy().to_lowercase())
            .collect();
        names.sort();
        names.dedup();
        Self {
            backend,
            denied,
            kept_out,
            except,
            names,
        }
    }

    /// `program` and `args`, put under the list. The process keeps its pid on
    /// macOS, since `sandbox-exec` replaces itself. On Linux `bwrap` stays as its
    /// parent, in the same group, so a group kill reaches both.
    pub fn wrap(&self, program: String, args: Vec<String>) -> (String, Vec<String>) {
        match &self.backend {
            Backend::Seatbelt => {
                let mut all = vec![
                    "-p".to_string(),
                    profile(&self.denied, &self.kept_out, &self.except),
                    program,
                ];
                all.extend(args);
                (SANDBOX_EXEC.to_string(), all)
            }
            Backend::Bubblewrap(bwrap) => {
                let mut all = bubblewrap_args(&self.denied, &self.kept_out, &self.except);
                all.push(program);
                all.extend(args);
                (bwrap.to_string_lossy().into_owned(), all)
            }
        }
    }

    /// Whether this output is the sandbox itself failing, rather than the command.
    /// The command then never ran, under a sandbox or otherwise. Only the first
    /// thing printed counts, because a build that runs `sandbox-exec` of its own
    /// can print the same words partway through.
    pub(crate) fn failed_to_start(text: &str) -> bool {
        text.lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with("Exit code") && *line != "[stderr]")
            .is_some_and(|line| line.starts_with("sandbox-exec: ") || line.starts_with("bwrap: "))
    }

    /// The line of output that looks like this refusing a write, if there is one:
    /// a permission error that names something on the list. A write through a
    /// path the program built may not be named, and then this says nothing.
    pub(crate) fn refusal_in<'a>(&self, text: &'a str) -> Option<&'a str> {
        text.lines().find(|line| {
            let line = line.to_lowercase();
            [
                "operation not permitted",
                "read-only file system",
                "eperm",
                "erofs",
            ]
            .iter()
            .any(|phrase| line.contains(phrase))
                && self.names.iter().any(|name| line.contains(name.as_str()))
        })
    }
}

/// Says there's no sandbox here, the first time a conversation would have used it.
pub(crate) fn say_once(session: Option<&str>, why: &str) -> Option<String> {
    static SAID: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let first = SAID
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(session.unwrap_or("").to_string());
    first.then(|| {
        format!(
            "Commands under a standing grant run without the write protection here, because \
             {why}. The check after each command still applies."
        )
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::permission::{DenyAll, PermissionEngine};
    use crate::registry::ToolRegistry;
    use crate::tool::{ToolContext, ToolError};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    /// `true` when a real sandbox can be used, and says so plainly when not.
    fn usable(test: &str) -> bool {
        if turned_off() {
            let _ = std::io::Write::write_fmt(
                &mut std::io::stderr(),
                format_args!("SKIPPED {test}: {OFF_ENV} is set"),
            );
            return false;
        }
        match backend() {
            Ok(_) => true,
            Err(why) => {
                let _ = std::io::Write::write_fmt(
                    &mut std::io::stderr(),
                    format_args!("SKIPPED {test}: {why}"),
                );
                false
            }
        }
    }

    struct Bench {
        _dir: tempfile::TempDir,
        root: PathBuf,
        user: PathBuf,
        ctx: ToolContext,
        registry: ToolRegistry,
    }

    /// A project in a made-up home, whose `.taurus` makes that its user home, so
    /// the real `~/.ssh` is never in reach of a test.
    fn bench(programs: &[&str]) -> Bench {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().canonicalize().unwrap();
        let root = user.join("project");
        let home = user.join(".taurus");
        std::fs::create_dir_all(root.join(".git/hooks")).unwrap();
        std::fs::create_dir_all(root.join(".taurus")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(user.join(".ssh")).unwrap();
        std::fs::write(root.join(".git/hooks/keep"), "old\n").unwrap();
        std::fs::write(root.join(".git/config"), "[core]\n\tbare = false\n").unwrap();
        let allowed: Vec<String> = programs
            .iter()
            .map(|program| format!("\"run_command:{program}\""))
            .collect();
        std::fs::write(
            root.join(".taurus/permissions.json"),
            format!("{{\"allowed\": [{}]}}", allowed.join(",")),
        )
        .unwrap();
        let engine = PermissionEngine::new(&root, &home, Box::new(DenyAll));
        let ctx = ToolContext::new(&root, Arc::new(engine), CancellationToken::new());
        Bench {
            _dir: dir,
            root,
            user,
            ctx,
            registry: ToolRegistry::with_builtins(),
        }
    }

    impl Bench {
        fn script(&self, body: &str) {
            let path = self.root.join("x.sh");
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        async fn run(&self, input: serde_json::Value) -> Result<String, ToolError> {
            self.registry
                .execute("run_command", input, &self.ctx)
                .await
                .map(|output| output.to_string())
        }
        async fn line(&self, command: &str) -> String {
            self.run(serde_json::json!({ "command": command }))
                .await
                .unwrap()
        }
    }

    #[tokio::test]
    async fn the_list_is_enforced_and_everything_else_is_writable() {
        if !usable("the_list_is_enforced") {
            return;
        }
        let b = bench(&["./x.sh"]);
        b.script(&format!(
            "echo a > .git/hooks/x; echo hook=$?\n\
             echo a > .git/hooks/keep; echo keep=$?\n\
             echo a > .taurus/plugins-not-listed; echo other=$?\n\
             echo a > {user}/.ssh/x; echo ssh=$?\n\
             echo a > {user}/.taurus/anything; echo home=$?\n\
             echo a > .git/description; echo gitfile=$?\n\
             echo a > .git/config; echo config=$?\n\
             echo a > src.txt; echo plain=$?\n\
             touch .; echo touch=$?\n\
             exit 7",
            user = b.user.display()
        ));
        let text = b.line("./x.sh").await;
        // Any failure counts: a refused redirect is 1 from macOS's sh and 2 from dash.
        for name in ["hook", "keep", "ssh", "home"] {
            assert!(text.contains(&format!("{name}=")), "{name}: {text}");
            assert!(!text.contains(&format!("{name}=0")), "{name}: {text}");
        }
        for line in ["other=0", "gitfile=0", "config=0", "plain=0", "touch=0"] {
            assert!(text.contains(line), "{line}: {text}");
        }
        assert!(text.starts_with("Exit code 7"), "{text}");
        assert!(!b.root.join(".git/hooks/x").exists());
        assert_eq!(
            std::fs::read_to_string(b.root.join(".git/hooks/keep")).unwrap(),
            "old\n"
        );
    }

    /// A delegate in a copy of the workspace, running a granted command: the
    /// parent's folder is out of reach, apart from the git directory the copy's
    /// commits go into, and that directory's hooks are still refused.
    #[tokio::test]
    async fn a_command_in_a_copy_cannot_write_the_folder_it_was_copied_from() {
        if !usable("a_command_in_a_copy") {
            return;
        }
        let b = bench(&["./x.sh"]);
        let admin = b.root.join(".git/worktrees/wt");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::create_dir_all(b.root.join(".git/objects")).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        let copy = b.user.join("copies/wt");
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(copy.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        let script = copy.join("x.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 echo a > {main}/src.txt; echo main=$?\n\
                 echo a > {main}/.git/objects/x; echo objects=$?\n\
                 echo a > {main}/.git/worktrees/wt/index; echo admin=$?\n\
                 echo a > {main}/.git/hooks/x; echo hook=$?\n\
                 echo a > here.txt; echo here=$?\n",
                main = b.root.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ctx = b.ctx.clone().rooted_at(&copy, None);

        let text = b
            .registry
            .execute(
                "run_command",
                serde_json::json!({ "command": "./x.sh" }),
                &ctx,
            )
            .await
            .unwrap()
            .to_string();
        for refused in ["main", "hook"] {
            assert!(text.contains(&format!("{refused}=")), "{refused}: {text}");
            assert!(!text.contains(&format!("{refused}=0")), "{refused}: {text}");
        }
        for allowed in ["objects=0", "admin=0", "here=0"] {
            assert!(text.contains(allowed), "{allowed}: {text}");
        }
        assert!(!b.root.join("src.txt").exists());
        assert!(copy.join("here.txt").exists());
    }

    /// A call somebody approved runs bare, because they said yes.
    #[tokio::test]
    async fn an_approved_call_is_not_under_the_list() {
        if !usable("an_approved_call") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".git/hooks")).unwrap();
        let home = root.join("cfg");
        std::fs::create_dir_all(&home).unwrap();
        let engine = PermissionEngine::new(&root, &home, Box::new(crate::permission::AllowAll));
        let ctx = ToolContext::new(&root, Arc::new(engine), CancellationToken::new());
        let text = ToolRegistry::with_builtins()
            .execute(
                "run_command",
                serde_json::json!({"command": "touch .git/hooks/mine"}),
                &ctx,
            )
            .await
            .unwrap()
            .to_string();
        assert!(root.join(".git/hooks/mine").exists(), "{text}");
    }

    /// What only `sandbox-exec` can do, since a bind needs something to bind over.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn routes_around_the_list_are_closed_on_macos() {
        if !usable("routes_around_the_list") {
            return;
        }
        let b = bench(&["./x.sh"]);
        b.script(&format!(
            "echo a > .GIT/HOOKS/x; echo case=$?\n\
             ln -s .git/hooks l; echo a > l/y; echo link=$?; rm -f l\n\
             ln .git/hooks/keep h; echo hard=$?\n\
             mv .git g; echo rename=$?\n\
             cd ..; mv project p2; echo root=$?; cd project\n\
             echo a > .taurus/hooks.json; echo notyet=$?\n\
             mkdir -p .taurus/plugins/x; echo plugin=$?\n\
             echo a > {user}/.zshrc; echo zshrc=$?\n\
             mkdir -p {user}/Library/LaunchAgents; echo agents=$?\n\
             mv {user}/.ssh {user}/s; echo ssh=$?",
            user = b.user.display()
        ));
        let text = b.line("./x.sh").await;
        for line in [
            "case=1", "link=1", "hard=1", "rename=1", "root=1", "notyet=1", "plugin=1", "zshrc=1",
            "agents=1", "ssh=1",
        ] {
            assert!(text.contains(line), "{line}: {text}");
        }
    }

    #[tokio::test]
    async fn a_refused_write_tells_the_model_and_the_line_gets_asked_about() {
        if !usable("a_refused_write_tells") {
            return;
        }
        let b = bench(&["./x.sh"]);
        b.script("echo a > .git/hooks/x");
        let text = b.line("./x.sh").await;
        assert!(text.contains("refused"), "{text}");
        assert!(text.contains("approve"), "{text}");
        let again = b.run(serde_json::json!({"command": "./x.sh"})).await;
        assert!(matches!(again, Err(ToolError::Denied)), "{again:?}");
    }

    #[tokio::test]
    async fn every_way_of_running_works_under_it() {
        if !usable("every_way_of_running") {
            return;
        }
        let mut b = bench(&["./x.sh", "../x.sh"]);
        std::fs::create_dir_all(b.root.join("sub")).unwrap();
        b.script("pwd; echo \"[$CHECK]\"; exit 3");

        // A setting on the line, and where it was asked to run.
        let text = b
            .run(serde_json::json!({"command": "CHECK=1 ../x.sh", "cwd": "sub"}))
            .await
            .unwrap();
        assert!(text.starts_with("Exit code 3"), "{text}");
        assert!(text.contains("/project/sub"), "{text}");
        assert!(text.contains("[1]"), "{text}");

        // Nothing with stdin gets here: it's asked about, and approved runs bare.
        let asked = b
            .run(serde_json::json!({"command": "./x.sh", "stdin": "fed\n"}))
            .await;
        assert!(matches!(asked, Err(ToolError::Denied)), "{asked:?}");

        let text = b
            .run(serde_json::json!({"command": "./x.sh", "pty": true}))
            .await
            .unwrap();
        assert!(text.contains("Exit code 3"), "{text}");

        // Background, and a stop reaching what the script started, through the
        // wrapper.
        let jobs = Arc::new(crate::jobs::Jobs::new());
        b.ctx.jobs = Some(jobs.clone());
        b.script("sleep 60 & echo $! > child.pid; wait");
        b.run(serde_json::json!({"command": "./x.sh", "background": true}))
            .await
            .unwrap();
        let pid_file = b.root.join("child.pid");
        tokio::time::timeout(Duration::from_secs(20), async {
            while !pid_file.exists() || std::fs::read_to_string(&pid_file).unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        jobs.stop(1).await.unwrap();
        let alive = std::process::Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "the child under the wrapper outlived the stop");
    }

    #[tokio::test]
    async fn git_works_under_it_and_a_hook_still_runs() {
        if !usable("git_works_under_it") {
            return;
        }
        let git = |dir: &Path, args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
        };
        let b = bench(&["git", "./x.sh"]);
        std::fs::remove_dir_all(b.root.join(".git")).unwrap();
        if git(&b.root, &["init", "-q"]).is_err() {
            let _ = std::io::Write::write_fmt(
                &mut std::io::stderr(),
                format_args!("SKIPPED git_works_under_it: no git"),
            );
            return;
        }
        git(&b.root, &["config", "user.name", "t"]).unwrap();
        git(&b.root, &["config", "user.email", "t@t"]).unwrap();
        let hook = b.root.join(".git/hooks/pre-commit");
        std::fs::write(&hook, "#!/bin/sh\necho ran > hook-ran\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(b.root.join("a.txt"), "x").unwrap();
        git(&b.root, &["add", "a.txt"]).unwrap();

        let text = b.line("git commit -m first").await;
        assert!(text.starts_with("Exit code 0"), "{text}");
        assert!(b.root.join("hook-ran").exists(), "{text}");

        // The file the sandbox leaves to the look afterwards.
        let text = b.line("git config user.name other").await;
        assert!(
            text.starts_with("Exit code 0") && !text.contains("[taurus]"),
            "{text}"
        );
        assert!(std::fs::read_to_string(b.root.join(".git/config"))
            .unwrap()
            .contains("other"));

        let bare = b.user.join("remote.git");
        git(&b.user, &["init", "-q", "--bare", bare.to_str().unwrap()]).unwrap();
        let remote = format!("git remote add o {}", bare.display());
        assert!(b.line(&remote).await.starts_with("Exit code 0"));
        assert!(b
            .line("git push -q o HEAD:main")
            .await
            .starts_with("Exit code 0"));
        let text = b.line("git fetch -q o").await;
        assert!(text.starts_with("Exit code 0"), "{text}");
    }

    /// Real toolchains, on real homes. Not in the default run, because it depends
    /// on what's installed: `cargo test -- --ignored everyday_tools`.
    #[tokio::test]
    #[ignore]
    async fn everyday_tools_under_it() {
        if !usable("everyday_tools") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let config = root.join("cfg");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(root.join(".taurus")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join(".taurus/permissions.json"),
            r#"{"allowed": ["run_command:cargo", "run_command:npm", "run_command:pnpm"]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[workspace]\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        let engine = PermissionEngine::new(&root, &config, Box::new(DenyAll));
        let ctx = ToolContext::new(&root, Arc::new(engine), CancellationToken::new());
        let registry = ToolRegistry::with_builtins();
        for line in [
            "cargo check",
            "npm --version",
            "npm init -y",
            "pnpm --version",
            "pnpm init",
        ] {
            let text = registry
                .execute("run_command", serde_json::json!({ "command": line }), &ctx)
                .await
                .unwrap()
                .to_string();
            println!("{line}: {text}");
            if text.starts_with("Exit code 127") {
                println!("SKIPPED {line}: not installed");
                continue;
            }
            assert!(text.starts_with("Exit code 0"), "{line}: {text}");
        }
    }

    #[test]
    fn the_list_says_what_it_says() {
        let root = Path::new("/w");
        let denied = denied_paths(
            root,
            &[PathBuf::from("/w/.git")],
            Some(Path::new("/u/.taurus")),
            Some(Path::new("/u")),
        );
        for wanted in [
            "/w/.git/hooks",
            "/w/.taurus/permissions.json",
            "/w/.taurus/hooks.json",
            "/w/.taurus/mcp.json",
            "/w/.taurus/plugins",
            "/u/.taurus",
            "/u/.ssh",
            "/u/.gnupg",
            "/u/.aws",
            "/u/.zshrc",
            "/u/.bashrc",
            "/u/.config/fish",
            "/u/Library/LaunchAgents",
            "/u/.config/systemd/user",
        ] {
            assert!(denied.contains(&PathBuf::from(wanted)), "{wanted}");
        }
        // Left to the look afterwards, on purpose.
        for left in ["/w/.git/config", "/u/.gitconfig", "/u/.config/git/config"] {
            assert!(!denied.contains(&PathBuf::from(left)), "{left}");
        }
    }

    #[test]
    fn a_workspace_in_the_config_home_can_write_itself() {
        let home = Path::new("/u/.taurus");
        let denied = denied_paths(Path::new("/u/.taurus/plugins/x"), &[], Some(home), None);
        assert!(!denied.contains(&home.to_path_buf()));
        assert!(denied.contains(&home.join("permissions.json")));
    }

    #[test]
    fn a_path_cannot_break_out_of_a_profile() {
        let profile = profile(&[PathBuf::from("/w/a\"b\\c/hooks")], &[], &[]);
        assert!(
            profile.contains(r#"(subpath "/w/a\"b\\c/hooks")"#),
            "{profile}"
        );
        // The rule for a folder above it is narrower than the one for the path.
        assert!(profile.contains("(deny file-write-unlink"), "{profile}");
        assert!(profile.contains(r#"(literal "/w")"#), "{profile}");
        assert!(!profile.contains(r#"(literal "/")"#), "{profile}");
    }

    #[test]
    fn a_folder_kept_out_is_bound_read_only_under_its_writable_git_dir_and_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let git = main.join(".git");
        let hooks = git.join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let args = bubblewrap_args(
            std::slice::from_ref(&hooks),
            std::slice::from_ref(&main),
            std::slice::from_ref(&git),
        );
        let at = |flag: &str, path: &Path| {
            let text = path.to_string_lossy();
            args.windows(3)
                .position(|w| w[0] == flag && w[1] == text && w[2] == text)
                .unwrap_or_else(|| panic!("{flag} {text} in {args:?}"))
        };
        // Later binds win, so the order is the rule.
        assert!(at("--ro-bind", &main) < at("--bind", &git));
        assert!(at("--bind", &git) < at("--ro-bind", &hooks));

        let profile = profile(
            std::slice::from_ref(&hooks),
            std::slice::from_ref(&main),
            std::slice::from_ref(&git),
        );
        assert!(
            profile.contains(&format!(
                "(require-all (subpath {}) (require-not (subpath {})))",
                quoted(&main),
                quoted(&git)
            )),
            "{profile}"
        );
    }

    #[test]
    fn bubblewrap_binds_only_what_exists() {
        let dir = tempfile::tempdir().unwrap();
        let there = dir.path().join("there");
        std::fs::create_dir_all(there.join("in")).unwrap();
        let args = bubblewrap_args(
            &[there.clone(), there.join("in"), dir.path().join("missing")],
            &[],
            &[],
        );
        let text = args.join(" ");
        assert!(text.starts_with("--dev-bind / /"), "{text}");
        assert_eq!(
            args.iter().filter(|a| *a == "--ro-bind").count(),
            1,
            "{text}"
        );
        assert!(!text.contains("missing"), "{text}");
        assert!(!text.contains("--new-session"), "{text}");
    }

    #[test]
    fn turning_it_off_reads_as_a_person_would_write_it() {
        for off in ["1", "true", "yes"] {
            assert!(says_off(Some(off)), "{off}");
        }
        for on in ["", "0", "false", "no", "off"] {
            assert!(!says_off(Some(on)), "{on}");
        }
        assert!(!says_off(None));
    }

    #[test]
    fn the_sandbox_failing_is_told_from_the_command_failing() {
        assert!(Fence::failed_to_start(
            "Exit code 71\n[stderr]\nsandbox-exec: sandbox_apply: Operation not permitted"
        ));
        assert!(Fence::failed_to_start(
            "bwrap: Creating new namespace failed"
        ));
        assert!(!Fence::failed_to_start("error: could not compile"));
        assert!(!Fence::failed_to_start(
            "Exit code 1\nbuilding\nsandbox-exec: sandbox_apply: Operation not permitted"
        ));
    }

    #[test]
    fn a_refusal_is_told_from_any_other_permission_error() {
        let fence = Fence::over(
            Backend::Seatbelt,
            vec![PathBuf::from("/w/.git/hooks")],
            vec![],
            vec![],
        );
        assert!(fence
            .refusal_in("sh: .git/hooks/x: Operation not permitted")
            .is_some());
        assert!(fence
            .refusal_in("x: /w/.git/hooks/y: Read-only file system")
            .is_some());
        assert!(fence
            .refusal_in("sh: /etc/passwd: Operation not permitted")
            .is_none());
        assert!(fence.refusal_in(".git/hooks is fine").is_none());
    }

    #[test]
    fn no_sandbox_is_said_once_a_conversation() {
        assert!(say_once(Some("say-once-a"), "x").is_some());
        assert!(say_once(Some("say-once-a"), "x").is_none());
        assert!(say_once(Some("say-once-b"), "x").is_some());
    }
}

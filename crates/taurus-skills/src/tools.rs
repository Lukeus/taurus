//! The tools through which the model reaches the skill library.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::info;

use taurus_tools::tool::{parse_input, schema_for};
use taurus_tools::{Effect, Tool, ToolContext, ToolError, ToolResult};

use crate::catalog::SkillCatalog;
use crate::interpreter;
use crate::proposal::{validate_proposal, ProposalSink, ProposedScript, SkillProposal};

/// Shared, hot-reloadable catalog. Approving a skill updates it in place, so a
/// running session can use a skill it just wrote.
pub type SharedCatalog = Arc<RwLock<SkillCatalog>>;

// ---------------------------------------------------------------- load_skill

#[derive(Deserialize, JsonSchema)]
pub struct LoadSkillInput {
    /// The skill's name, exactly as listed in the Skills section.
    pub name: String,
}

pub struct LoadSkill {
    catalog: SharedCatalog,
}

impl LoadSkill {
    pub fn new(catalog: SharedCatalog) -> Self {
        Self { catalog }
    }
}

#[async_trait]
impl Tool for LoadSkill {
    fn name(&self) -> &str {
        "load_skill"
    }

    fn description(&self) -> &str {
        "Read the full procedure for one of the skills listed in the Skills section. Call this \
         before acting on a skill: the one-line index entry is not the instructions."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<LoadSkillInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        format!(
            "Load skill {}",
            input.get("name").and_then(|n| n.as_str()).unwrap_or("?")
        )
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let input: LoadSkillInput = parse_input(input)?;
        let catalog = self.catalog.read().await;

        let Some(skill) = catalog.get(&input.name) else {
            let known: Vec<&str> = catalog.names().collect();
            return Err(ToolError::InvalidInput(if known.is_empty() {
                "There are no skills available.".into()
            } else {
                format!(
                    "No skill named '{}'. Available: {}.",
                    input.name,
                    known.join(", ")
                )
            }));
        };

        // No arguments: a tool call carries its request in the conversation
        // already, unlike a slash command where the user's line is the input.
        Ok(skill.render("").into())
    }
}

// ---------------------------------------------------------- run_skill_script

#[derive(Deserialize, JsonSchema)]
pub struct RunSkillScriptInput {
    /// Name of the skill that owns the script.
    pub skill: String,
    /// Script path as listed in the skill, e.g. `extract.py`.
    pub script: String,
    /// Arguments passed to the script.
    #[serde(default)]
    pub args: Vec<String>,
    /// Seconds before the script is killed. Defaults to 120.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

pub struct RunSkillScript {
    catalog: SharedCatalog,
}

impl RunSkillScript {
    pub fn new(catalog: SharedCatalog) -> Self {
        Self { catalog }
    }
}

#[async_trait]
impl Tool for RunSkillScript {
    fn name(&self) -> &str {
        "run_skill_script"
    }

    fn description(&self) -> &str {
        "Run a script bundled with a skill. Load the skill first to see which scripts it has and \
         what arguments they take."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<RunSkillScriptInput>()
    }

    fn effect(&self) -> Effect {
        Effect::Execute
    }

    /// A skill script is a local program with the workspace in front of it, and
    /// it names the files it will write no more than a shell command does.
    fn touches_unpredictably(&self) -> bool {
        true
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        let get = |k: &str| input.get(k).and_then(|v| v.as_str()).unwrap_or("?");
        let args = input
            .get("args")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        format!("Run {} from skill '{}' {args}", get("script"), get("skill"))
            .trim_end()
            .to_string()
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let input: RunSkillScriptInput = parse_input(input)?;

        let (script_path, interpreter_name, skill_dir) = {
            let catalog = self.catalog.read().await;
            let skill = catalog.get(&input.skill).ok_or_else(|| {
                ToolError::InvalidInput(format!("No skill named '{}'.", input.skill))
            })?;
            let script = skill
                .frontmatter
                .scripts
                .iter()
                .find(|s| s.path == input.script)
                .ok_or_else(|| {
                    let known: Vec<&str> = skill
                        .frontmatter
                        .scripts
                        .iter()
                        .map(|s| s.path.as_str())
                        .collect();
                    ToolError::InvalidInput(if known.is_empty() {
                        format!("Skill '{}' has no scripts.", input.skill)
                    } else {
                        format!(
                            "Skill '{}' has no script '{}'. It has: {}.",
                            input.skill,
                            input.script,
                            known.join(", ")
                        )
                    })
                })?;
            (
                // Not `join`: a discovered script's path is `scripts/run.py`,
                // and joining that whole onto a Windows directory mixes
                // separators in the path handed to the interpreter.
                skill.resource_path(&script.path),
                script.interpreter.clone(),
                skill.dir.clone(),
            )
        };

        if !script_path.is_file() {
            return Err(ToolError::Failed(format!(
                "{} is listed by the skill but missing from disk",
                input.script
            )));
        }

        let interpreter = interpreter::resolve(&interpreter_name).map_err(|reason| {
            ToolError::Failed(format!(
                "{reason}. Follow the skill's written steps instead."
            ))
        })?;

        let mut command = tokio::process::Command::new(&interpreter.program);
        command.args(&interpreter.leading_args);
        command.arg(&script_path);
        command.args(&input.args);
        // Scripts run against the workspace, not their own directory: a skill
        // operates on the user's project. SKILL_DIR gives them a way back to
        // their own bundled resources.
        command.current_dir(&ctx.workspace);
        command.env("TAURUS_SKILL_DIR", &skill_dir);
        command.env("TAURUS_WORKSPACE", &ctx.workspace);
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        command.kill_on_drop(true);
        // A skill's interpreter is a console program like any other, and would
        // otherwise flash up a window on Windows for the length of the script.
        taurus_tools::no_console(&mut command);

        let mut child = command
            .spawn()
            .map_err(|e| ToolError::Failed(format!("cannot start {interpreter_name}: {e}")))?;

        // Both pipes are drained from the moment the script starts, beside the
        // wait rather than after it. A pipe holds 64 KB, and a script that
        // prints more than that blocks on its next write until somebody reads:
        // read only once it has exited, it never exits, and the timeout and
        // Stop would both be stuck inside the same wait.
        let stdout = Drain::start(child.stdout.take());
        let stderr = Drain::start(child.stderr.take());
        let timeout =
            std::time::Duration::from_secs(input.timeout_secs.unwrap_or(120).clamp(1, 600));

        let ended = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => Ended::Canceled,
            result = tokio::time::timeout(timeout, child.wait()) => match result {
                Ok(Ok(status)) => Ended::Exited(status),
                Ok(Err(e)) => Ended::Failed(e),
                Err(_) => Ended::TimedOut,
            },
        };
        if !matches!(ended, Ended::Exited(_)) {
            // Before anything waits on the pipes: they reach end-of-file only
            // once the script is gone.
            let _ = child.start_kill();
        }
        if matches!(ended, Ended::Canceled) {
            stdout.abandon();
            stderr.abandon();
            return Err(ToolError::Canceled);
        }

        let (out, err) = tokio::join!(stdout.finish(), stderr.finish());
        let report = report(&out, &err, &input.script, ctx);

        match ended {
            Ended::Exited(status) => Ok(match status.code() {
                Some(0) => report.into(),
                Some(code) => format!("Exit code {code}\n{report}").into(),
                None => format!("Killed by signal\n{report}").into(),
            }),
            // What it printed is kept: a script that hangs on its last step has
            // usually said why by then, and a run that comes back empty is run
            // again and costs the whole timeout twice.
            Ended::TimedOut => Err(ToolError::Failed(format!(
                "{} timed out after {}s and was stopped. Its output until then:\n{report}",
                input.script,
                timeout.as_secs()
            ))),
            Ended::Failed(e) => Err(ToolError::Failed(e.to_string())),
            Ended::Canceled => Err(ToolError::Canceled),
        }
    }
}

/// How a script run came to an end.
enum Ended {
    Exited(std::process::ExitStatus),
    TimedOut,
    Canceled,
    /// Waiting on the script failed, which is the OS refusing rather than the
    /// script doing anything.
    Failed(std::io::Error),
}

/// How long a script's pipes are read for once it has gone.
///
/// A pipe reaches end-of-file when the last process holding it closes it, and
/// that can be something the script started and left running. What is already
/// in the pipe arrives in well under this; what is still being written by a
/// process nobody is waiting on is not a reason to hold the turn.
const PIPE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// One of a script's pipes, read as it arrives.
///
/// On a task of its own, into a buffer shared with the tool rather than handed
/// back at the end, so a script that is stopped still has what it printed
/// before then, and a pipe that never closes costs [`PIPE_GRACE`] rather than
/// the turn.
struct Drain {
    buf: Arc<std::sync::Mutex<Vec<u8>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drain {
    fn start<R>(pipe: Option<R>) -> Self
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        let buf = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = buf.clone();
        let task = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let Some(mut pipe) = pipe else {
                return;
            };
            let mut chunk = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                sink.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(&chunk[..n]);
            }
        });
        Self { buf, task }
    }

    /// What was read, once the pipe closes or [`PIPE_GRACE`] has passed.
    async fn finish(mut self) -> String {
        if tokio::time::timeout(PIPE_GRACE, &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
        }
        let buf = self.buf.lock().unwrap_or_else(|e| e.into_inner());
        String::from_utf8_lossy(&buf).into_owned()
    }

    fn abandon(self) {
        self.task.abort();
    }
}

/// What share of the window a script's report may take, and its bounds. The
/// same as a command's: a script is one.
const OUTPUT_SHARE: f32 = 0.08;
const MIN_OUTPUT_BYTES: usize = 4 * 1024;
const MAX_OUTPUT_BYTES: usize = 512 * 1024;

/// A script's output as the model reads it: stdout, then stderr under its own
/// heading, cut to what the window can spare.
///
/// The whole of it is written out first where there is anywhere to write it, so
/// the middle of a long report is a `read_file` away rather than gone — the
/// bargain the shell tool and MCP results strike.
fn report(out: &str, err: &str, script: &str, ctx: &ToolContext) -> String {
    let mut report = String::new();
    if !out.trim().is_empty() {
        report.push_str(out);
    }
    if !err.trim().is_empty() {
        if !report.is_empty() {
            report.push('\n');
        }
        report.push_str("[stderr]\n");
        report.push_str(err);
    }
    if report.trim().is_empty() {
        return "(no output)".into();
    }

    let cap = ctx
        .budget
        .bytes(OUTPUT_SHARE, MIN_OUTPUT_BYTES, MAX_OUTPUT_BYTES);
    if report.len() <= cap {
        return report;
    }
    let spilled = taurus_tools::overflow::spill(&report, script, ctx);
    taurus_tools::overflow::cut(&report, cap, |omitted| match &spilled {
        Some(path) => format!(
            "{omitted} bytes omitted; the whole output was written to {} — read_file it",
            path.display()
        ),
        None => format!("{omitted} bytes omitted"),
    })
}

// ------------------------------------------------------------- propose_skill

#[derive(Deserialize, JsonSchema)]
pub struct ProposeSkillInput {
    /// Kebab-case identifier, e.g. `release-notes`.
    pub name: String,
    /// One sentence on what the skill does.
    pub description: String,
    /// The situation in which this skill applies. This is the only text you
    /// will see later when deciding whether to open it, so describe the
    /// trigger, not the capability. Under 200 characters.
    pub when_to_use: String,
    /// The procedure itself, as markdown: numbered steps, specific commands,
    /// and the gotchas you hit. Write it for someone who has never done this.
    pub body: String,
    /// Optional scripts to bundle with the skill.
    #[serde(default)]
    pub scripts: Vec<ProposeScriptInput>,
    /// Why this is worth keeping. Shown to the user, not stored.
    #[serde(default)]
    pub rationale: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ProposeScriptInput {
    /// File name inside the skill directory, e.g. `extract.py`.
    pub path: String,
    /// One of: python3, node, bash, sh, pwsh, deno, ruby.
    pub interpreter: String,
    #[serde(default)]
    pub description: String,
    /// The complete script source.
    pub content: String,
}

pub struct ProposeSkill {
    catalog: SharedCatalog,
    sink: Arc<dyn ProposalSink>,
}

/// Named as a constant because the host registers and unregisters this tool as
/// the skill-synthesis setting changes, and a name typed twice is a name that
/// can be typed differently.
pub const PROPOSE_TOOL: &str = "propose_skill";

impl ProposeSkill {
    pub fn new(catalog: SharedCatalog, sink: Arc<dyn ProposalSink>) -> Self {
        Self { catalog, sink }
    }
}

#[async_trait]
impl Tool for ProposeSkill {
    fn name(&self) -> &str {
        PROPOSE_TOOL
    }

    fn description(&self) -> &str {
        "Write down a procedure you worked out so it can be reused later. Propose a skill when you \
         solved something non-obvious that will come up again — a multi-step workflow, a tool's \
         quirks, a project convention you had to discover. Do not propose one for a single \
         command, for something already covered by an existing skill, or for facts specific to \
         this one task. The user reviews every proposal before it is saved, so you can keep \
         working immediately after calling this."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_for::<ProposeSkillInput>()
    }

    /// Read, not Write: this creates no file. The review card the user sees is
    /// itself the permission prompt, so gating it here would ask twice.
    fn effect(&self) -> Effect {
        Effect::Read
    }

    fn preview(&self, input: &serde_json::Value) -> String {
        format!(
            "Propose skill '{}'",
            input.get("name").and_then(|n| n.as_str()).unwrap_or("?")
        )
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let input: ProposeSkillInput = parse_input(input)?;

        let mut proposal =
            SkillProposal::new(input.name, input.description, input.when_to_use, input.body);
        proposal.rationale = input.rationale;
        proposal.scripts = input
            .scripts
            .into_iter()
            .map(|s| ProposedScript {
                path: s.path,
                interpreter: s.interpreter,
                description: s.description,
                content: s.content,
            })
            .collect();

        {
            let catalog = self.catalog.read().await;
            proposal.replaces_existing = catalog.contains(&proposal.name);
            // Rejections come back as tool errors so the model can fix the
            // proposal rather than assume it succeeded.
            validate_proposal(&proposal, &catalog)
                .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        }

        let name = proposal.name.clone();
        info!(skill = %name, "skill proposed");
        self.sink.submit(proposal).await;

        Ok(format!(
            "Proposed skill '{name}'. It is queued for the user to review; it becomes available \
             once they approve it. Carry on with the current task."
        )
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{SkillSource, SKILL_FILE};
    use crate::proposal::CollectingSink;
    use crate::skill::{SkillOrigin, SkillTier};
    use std::path::Path;
    use taurus_tools::{AllowAll, PermissionEngine};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    struct Fixture {
        ctx: ToolContext,
        catalog: SharedCatalog,
        _workspace: TempDir,
        skills: TempDir,
    }

    fn fixture(skills: &[(&str, &str)]) -> Fixture {
        let skills_dir = TempDir::new().unwrap();
        for (name, extra) in skills {
            let dir = skills_dir.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(SKILL_FILE),
                format!(
                    "---\nname: {name}\ndescription: does {name}\n\
                     when_to_use: when you need {name}\n{extra}---\n\nProcedure for {name}.\n"
                ),
            )
            .unwrap();
        }
        let (catalog, problems) = SkillCatalog::discover(&[SkillSource {
            tier: SkillTier::User,
            origin: SkillOrigin::Taurus,
            dir: skills_dir.path().to_path_buf(),
        }]);
        assert!(problems.is_empty(), "{problems:?}");

        let workspace = TempDir::new().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        let permissions = Arc::new(PermissionEngine::new(
            &root,
            root.join(".taurus"),
            Box::new(AllowAll),
        ));
        Fixture {
            ctx: ToolContext::new(root, permissions, CancellationToken::new()),
            catalog: Arc::new(RwLock::new(catalog)),
            _workspace: workspace,
            skills: skills_dir,
        }
    }

    fn write_script(skills: &Path, skill: &str, name: &str, body: &str) {
        let path = skills.join(skill).join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
    }

    #[tokio::test]
    async fn load_skill_returns_the_body() {
        let f = fixture(&[("alpha", "")]);
        let out = LoadSkill::new(f.catalog.clone())
            .execute(serde_json::json!({"name": "alpha"}), &f.ctx)
            .await
            .unwrap();
        assert!(out.to_text().contains("Procedure for alpha."));
        assert!(out.to_text().contains("# Skill: alpha"));
    }

    #[tokio::test]
    async fn load_skill_names_bundled_files_without_reading_them() {
        let f = fixture(&[("alpha", "")]);
        // One component per `join`. Passing "alpha/references" whole would put
        // a forward slash inside a Windows path, and the expectation below is
        // compared against rendered text rather than used to open a file — so
        // it has to be spelled the way the platform spells it.
        let refs = f.skills.path().join("alpha").join("references");
        std::fs::create_dir_all(&refs).unwrap();
        std::fs::write(refs.join("REFERENCE.md"), "the whole reference text").unwrap();
        reload(&f).await;

        let out = LoadSkill::new(f.catalog.clone())
            .execute(serde_json::json!({"name": "alpha"}), &f.ctx)
            .await
            .unwrap();

        // Absolute: the model resolves a bare relative path against the
        // workspace, and a skill under the home directory is not there.
        let expected = refs.join("REFERENCE.md");
        assert!(
            out.to_text().contains(&expected.display().to_string()),
            "expected {} in:\n{out}",
            expected.display()
        );
        assert!(
            !out.to_text().contains("the whole reference text"),
            "a reference file must cost nothing until it is asked for"
        );
    }

    /// Rediscovers after files are added beside an already-loaded skill.
    async fn reload(f: &Fixture) {
        let (catalog, problems) = SkillCatalog::discover(&[SkillSource {
            tier: SkillTier::User,
            origin: SkillOrigin::Taurus,
            dir: f.skills.path().to_path_buf(),
        }]);
        assert!(problems.is_empty(), "{problems:?}");
        *f.catalog.write().await = catalog;
    }

    #[tokio::test]
    async fn load_skill_lists_alternatives_when_the_name_is_wrong() {
        let f = fixture(&[("alpha", ""), ("beta", "")]);
        let err = LoadSkill::new(f.catalog.clone())
            .execute(serde_json::json!({"name": "gamma"}), &f.ctx)
            .await
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("alpha"));
        assert!(message.contains("beta"));
    }

    #[tokio::test]
    async fn load_skill_warns_when_scripts_cannot_run_here() {
        let f = fixture(&[(
            "needs-tooling",
            "scripts:\n  - path: go.bf\n    interpreter: brainfuck\n",
        )]);
        let out = LoadSkill::new(f.catalog.clone())
            .execute(serde_json::json!({"name": "needs-tooling"}), &f.ctx)
            .await
            .unwrap();
        assert!(out.to_text().contains("cannot run on this machine"));
        assert!(out.to_text().contains("Follow the written steps"));
    }

    #[tokio::test]
    async fn run_skill_script_executes_and_returns_output() {
        let f = fixture(&[(
            "greeter",
            "scripts:\n  - path: hello.sh\n    interpreter: sh\n    description: greets\n",
        )]);
        write_script(
            f.skills.path(),
            "greeter",
            "hello.sh",
            "#!/bin/sh\necho \"hi $1\"\n",
        );

        let out = RunSkillScript::new(f.catalog.clone())
            .execute(
                serde_json::json!({"skill": "greeter", "script": "hello.sh", "args": ["there"]}),
                &f.ctx,
            )
            .await
            .unwrap();
        assert!(out.to_text().contains("hi there"));
    }

    #[tokio::test]
    async fn run_skill_script_runs_a_script_discovered_in_the_scripts_directory() {
        // The path of a discovered script always contains a forward slash,
        // because that is how the catalog writes logical paths. Joining it onto
        // a Windows skill directory whole would mix separators, and no other
        // test here uses a script that lives in a subdirectory at all.
        // Declared rather than discovered, so the interpreter is `sh` — the one
        // every other script test here already proves resolves on all three
        // platforms. What is under test is the path, not the lookup.
        let f = fixture(&[(
            "bundled",
            "scripts:\n  - path: scripts/greet.sh\n    interpreter: sh\n    description: greets\n",
        )]);
        std::fs::create_dir_all(f.skills.path().join("bundled/scripts")).unwrap();
        write_script(
            f.skills.path(),
            "bundled",
            "scripts/greet.sh",
            "#!/bin/sh\necho \"from a subdirectory\"\n",
        );

        let out = RunSkillScript::new(f.catalog.clone())
            .execute(
                serde_json::json!({"skill": "bundled", "script": "scripts/greet.sh"}),
                &f.ctx,
            )
            .await
            .unwrap();
        assert!(out.to_text().contains("from a subdirectory"), "{out}");
    }

    #[tokio::test]
    async fn run_skill_script_exposes_the_skill_directory_to_the_script() {
        let f = fixture(&[(
            "reader",
            "scripts:\n  - path: show.sh\n    interpreter: sh\n    description: reads a resource\n",
        )]);
        std::fs::write(f.skills.path().join("reader/data.txt"), "bundled data").unwrap();
        write_script(
            f.skills.path(),
            "reader",
            "show.sh",
            "#!/bin/sh\ncat \"$TAURUS_SKILL_DIR/data.txt\"\n",
        );

        let out = RunSkillScript::new(f.catalog.clone())
            .execute(
                serde_json::json!({"skill": "reader", "script": "show.sh"}),
                &f.ctx,
            )
            .await
            .unwrap();
        assert!(out.to_text().contains("bundled data"));
    }

    #[tokio::test]
    async fn run_skill_script_runs_in_the_workspace() {
        let f = fixture(&[(
            "pwd-check",
            "scripts:\n  - path: where.sh\n    interpreter: sh\n    description: prints cwd\n",
        )]);
        std::fs::write(f.ctx.workspace.join("marker.txt"), "").unwrap();
        write_script(f.skills.path(), "pwd-check", "where.sh", "#!/bin/sh\nls\n");

        let out = RunSkillScript::new(f.catalog.clone())
            .execute(
                serde_json::json!({"skill": "pwd-check", "script": "where.sh"}),
                &f.ctx,
            )
            .await
            .unwrap();
        assert!(out.to_text().contains("marker.txt"));
    }

    #[tokio::test]
    async fn run_skill_script_that_prints_more_than_a_pipe_holds_still_finishes() {
        // A pipe holds 64 KB, and a script that prints more than that blocks on
        // its next write until somebody reads. Read only once it has exited, it
        // never exits, and neither the timeout nor Stop can reach it.
        let f = fixture(&[(
            "chatty",
            "scripts:\n  - path: flood.sh\n    interpreter: sh\n    description: prints a lot\n",
        )]);
        write_script(
            f.skills.path(),
            "chatty",
            "flood.sh",
            "#!/bin/sh\ni=0\nwhile [ $i -lt 5000 ]; do\n  \
             echo \"line $i of a report long enough to fill a pipe\"\n  \
             i=$((i + 1))\ndone\necho finished >&2\n",
        );

        let tool = RunSkillScript::new(f.catalog.clone());
        let run = tool.execute(
            serde_json::json!({"skill": "chatty", "script": "flood.sh", "timeout_secs": 60}),
            &f.ctx,
        );
        let out = tokio::time::timeout(std::time::Duration::from_secs(30), run)
            .await
            .expect("a script that fills its pipe must still finish")
            .unwrap();
        let text = out.to_text();
        assert!(text.contains("line 0 of"), "the start is kept");
        assert!(text.contains("finished"), "stderr is still read");
        // Five thousand lines is past what one answer may take.
        assert!(text.contains("bytes omitted"), "not cut to the budget");
    }

    #[tokio::test]
    async fn a_script_past_its_timeout_is_stopped_before_its_output_is_read() {
        // Its pipes close only once it is gone. Read first, the tool waits out
        // the whole script rather than the timeout it was given.
        let f = fixture(&[(
            "slow",
            "scripts:\n  - path: slow.sh\n    interpreter: sh\n    description: takes too long\n",
        )]);
        write_script(
            f.skills.path(),
            "slow",
            "slow.sh",
            "#!/bin/sh\necho started\nsleep 30\necho never\n",
        );

        let tool = RunSkillScript::new(f.catalog.clone());
        let run = tool.execute(
            serde_json::json!({"skill": "slow", "script": "slow.sh", "timeout_secs": 1}),
            &f.ctx,
        );
        let message = tokio::time::timeout(std::time::Duration::from_secs(10), run)
            .await
            .expect("the timeout must end the script, not wait it out")
            .unwrap_err()
            .to_string();
        assert!(message.contains("timed out after 1s"), "{message}");
        assert!(
            message.contains("started"),
            "what it printed is kept: {message}"
        );
    }

    #[tokio::test]
    async fn a_script_that_leaves_something_running_is_not_waited_for() {
        // A pipe closes when the last process holding it does, and that can be
        // something the script started and left behind: a server, a watcher.
        let f = fixture(&[(
            "starter",
            "scripts:\n  - path: start.sh\n    interpreter: sh\n    description: starts something\n",
        )]);
        write_script(
            f.skills.path(),
            "starter",
            "start.sh",
            "#!/bin/sh\nsleep 30 &\necho started it\n",
        );

        let tool = RunSkillScript::new(f.catalog.clone());
        let run = tool.execute(
            serde_json::json!({"skill": "starter", "script": "start.sh"}),
            &f.ctx,
        );
        let out = tokio::time::timeout(std::time::Duration::from_secs(10), run)
            .await
            .expect("a finished script must not wait on what it left running")
            .unwrap();
        assert!(out.to_text().contains("started it"), "{out}");
    }

    #[tokio::test]
    async fn run_skill_script_reports_an_unknown_script_with_the_real_list() {
        let f = fixture(&[(
            "greeter",
            "scripts:\n  - path: hello.sh\n    interpreter: sh\n    description: greets\n",
        )]);
        let err = RunSkillScript::new(f.catalog.clone())
            .execute(
                serde_json::json!({"skill": "greeter", "script": "nope.sh"}),
                &f.ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("hello.sh"));
    }

    #[tokio::test]
    async fn run_skill_script_fails_clearly_when_the_file_is_missing() {
        let f = fixture(&[(
            "ghost",
            "scripts:\n  - path: absent.sh\n    interpreter: sh\n    description: missing\n",
        )]);
        let err = RunSkillScript::new(f.catalog.clone())
            .execute(
                serde_json::json!({"skill": "ghost", "script": "absent.sh"}),
                &f.ctx,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("missing from disk"));
    }

    #[tokio::test]
    async fn propose_skill_queues_a_valid_proposal() {
        let f = fixture(&[]);
        let sink = Arc::new(CollectingSink::default());
        let tool = ProposeSkill::new(f.catalog.clone(), sink.clone());

        let out = tool
            .execute(
                serde_json::json!({
                    "name": "release-notes",
                    "description": "Assemble release notes from merged PRs",
                    "when_to_use": "Preparing release notes for a tagged version",
                    "body": "1. List merged PRs since the last tag.\n2. Group by label.\n3. Write it up.",
                    "rationale": "Took three tries to get right"
                }),
                &f.ctx,
            )
            .await
            .unwrap();

        assert!(out.to_text().contains("queued"));
        let queued = sink.proposals.lock().await;
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].name, "release-notes");
        assert!(!queued[0].replaces_existing);
    }

    #[tokio::test]
    async fn propose_skill_rejects_an_invalid_proposal_without_queueing_it() {
        let f = fixture(&[]);
        let sink = Arc::new(CollectingSink::default());
        let tool = ProposeSkill::new(f.catalog.clone(), sink.clone());

        let err = tool
            .execute(
                serde_json::json!({
                    "name": "Bad Name",
                    "description": "d",
                    "when_to_use": "w",
                    "body": "1. Step one that is long enough to pass the length check easily."
                }),
                &f.ctx,
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("kebab-case"));
        assert!(sink.proposals.lock().await.is_empty());
    }

    #[tokio::test]
    async fn propose_skill_flags_a_proposal_that_would_replace_an_existing_one() {
        let f = fixture(&[("alpha", "")]);
        let sink = Arc::new(CollectingSink::default());
        let tool = ProposeSkill::new(f.catalog.clone(), sink.clone());

        tool.execute(
            serde_json::json!({
                "name": "alpha",
                "description": "a better alpha",
                "when_to_use": "when you need alpha done properly this time",
                "body": "1. Do the improved thing.\n2. Then do the other improved thing."
            }),
            &f.ctx,
        )
        .await
        .unwrap();

        assert!(sink.proposals.lock().await[0].replaces_existing);
    }
}

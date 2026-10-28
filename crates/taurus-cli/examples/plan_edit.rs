//! A plan the person edited, carried into a fresh process, and honored.
//!
//! `cargo build -p taurus-cli && cargo run -p taurus-cli --example plan_edit -- <model> [provider]`
//!
//! The desktop app saves an edit to the plan as a `plan` record in the
//! transcript. This writes that same record between two runs of the real
//! `taurus` binary, so it checks the whole path a restart takes:
//!
//! 1. **A real turn writes a plan** of three files. It runs without leave to
//!    write, so however keen the model is, the plan is all it can make.
//! 2. **The edit is recorded**: a fourth file added, the third taken off.
//! 3. **A fresh process resumes the conversation**, rebuilds the board from the
//!    transcript, and the model works the edited plan. The added file exists,
//!    the removed one doesn't, and the model's last list keeps the person's
//!    step, marked done.
//!
//! Needs Ollama, or the provider named after the model. It leaves one
//! conversation behind, filed under the scratch folder (passed with `-w`)
//! rather than any real workspace.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use taurus_host::sessions;

const PLAN: &str = "Plan this with update_plan, using exactly these three steps, worded exactly \
                    like this: 'Create a.txt', 'Create b.txt', 'Create c.txt'. Each file will \
                    hold one line, its own letter. Only make the plan for now: don't create any \
                    files until I say so.";

const GO_ON: &str = "Yes, carry on and finish the plan.";

fn taurus() -> PathBuf {
    let exe = std::env::current_exe().expect("own path");
    let bin = exe
        .parent()
        .and_then(Path::parent)
        .map(|dir| dir.join(format!("taurus{}", std::env::consts::EXE_SUFFIX)))
        .expect("a target directory");
    assert!(
        bin.is_file(),
        "{} isn't built. Run `cargo build -p taurus-cli` first.",
        bin.display()
    );
    bin
}

fn run(bin: &Path, workspace: &str, provider: &str, model: &str, writes: bool, extra: &[&str]) {
    let mut args = vec!["run", "-w", workspace, "-p", provider, "-m", model];
    if writes {
        args.extend(["--allow", "write_file"]);
    }
    args.extend_from_slice(extra);
    let status = Command::new(bin)
        .current_dir(workspace)
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .expect("taurus runs");
    assert!(status.success(), "taurus failed: {status}");
}

fn last_plan(id: &str) -> Vec<(String, String)> {
    let loaded = sessions::load(id).expect("the transcript reads back");
    loaded
        .session
        .messages
        .iter()
        .flat_map(|m| {
            m.tool_uses()
                .map(|(_, name, input)| (name.to_string(), input.clone()))
        })
        .filter(|(name, _)| name == "update_plan")
        .last()
        .map(|(_, input)| {
            input["steps"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| {
                    (
                        s["text"].as_str().unwrap_or_default().to_string(),
                        s["state"].as_str().unwrap_or("todo").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "qwen3.5:9b-mlx".into());
    let provider = std::env::args().nth(2).unwrap_or_else(|| "ollama".into());
    let workspace = tempfile::tempdir()?;
    let root = workspace.path().canonicalize()?;
    let bin = taurus();
    let workspace_arg = root.to_str().expect("a UTF-8 temp path");

    // 1. A plan, and nothing done yet.
    println!("Asking {model} for a three-step plan…");
    run(&bin, workspace_arg, &provider, &model, false, &[PLAN]);
    let id = sessions::latest(&root).expect("a conversation").id;
    let plan = last_plan(&id);
    println!("  its plan: {plan:?}");
    assert_eq!(
        plan.len(),
        3,
        "the model didn't write the plan it was given"
    );

    // 2. The edit the app would have saved: d added, c taken off.
    let steps: Vec<_> = plan
        .iter()
        .filter(|(text, _)| text != "Create c.txt")
        .map(|(text, state)| serde_json::json!({ "text": text, "state": state }))
        .chain([serde_json::json!({ "text": "Create d.txt", "state": "todo" })])
        .collect();
    let record = serde_json::json!({
        "type": "plan",
        "steps": steps,
        "added": ["Create d.txt"],
        "removed": ["Create c.txt"],
        "reordered": false,
        "at": 0,
    });
    let path = sessions::load(&id)?.path;
    let mut file = std::fs::OpenOptions::new().append(true).open(&path)?;
    writeln!(file, "{record}")?;
    println!("  edited: added 'Create d.txt', removed 'Create c.txt'");

    // 3. A fresh process, which only knows the edit from the transcript.
    println!("Resuming in a new process…");
    run(
        &bin,
        workspace_arg,
        &provider,
        &model,
        true,
        &["--resume", &id, GO_ON],
    );

    let plan = last_plan(&id);
    println!("  its last plan: {plan:?}");
    let exists = |name: &str| root.join(name).is_file();
    println!(
        "  files: a={} b={} c={} d={}",
        exists("a.txt"),
        exists("b.txt"),
        exists("c.txt"),
        exists("d.txt")
    );
    assert!(exists("d.txt"), "the step the person added wasn't done");
    assert!(
        !exists("c.txt"),
        "the step the person removed was done anyway"
    );
    assert!(
        plan.iter()
            .any(|(text, state)| text == "Create d.txt" && state == "done"),
        "the model's last list doesn't close the person's step"
    );
    assert!(
        !plan.iter().any(|(text, _)| text == "Create c.txt"),
        "the model's last list brought back the removed step"
    );
    println!("\nThe edit survived the restart, and the model worked the plan as edited.");
    Ok(())
}

//! Finding the program that runs a skill's script.
//!
//! A skill authored on macOS declares `python3`; the same skill on Windows has
//! to reach `py -3` or `python`. Resolving logical names to whatever is
//! actually installed is what keeps generated skills portable instead of
//! silently broken on two thirds of the supported platforms.

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interpreter {
    pub program: PathBuf,
    /// Arguments that precede the script path, e.g. `-3` for the Windows
    /// Python launcher.
    pub leading_args: Vec<String>,
}

/// Candidate programs for a logical interpreter name, best first.
fn candidates(name: &str) -> Vec<(&'static str, Vec<&'static str>)> {
    match name.trim().to_ascii_lowercase().as_str() {
        "python" | "python3" => vec![
            ("python3", vec![]),
            ("python", vec![]),
            // Windows launcher; -3 pins the major version.
            ("py", vec!["-3"]),
        ],
        "node" | "nodejs" => vec![("node", vec![])],
        "deno" => vec![("deno", vec!["run", "-A"])],
        "bash" => vec![("bash", vec![]), ("sh", vec![])],
        "sh" => vec![("sh", vec![]), ("bash", vec![])],
        "pwsh" | "powershell" => vec![
            ("pwsh", vec!["-NoProfile", "-File"]),
            ("powershell", vec!["-NoProfile", "-File"]),
        ],
        "ruby" => vec![("ruby", vec![])],
        _ => vec![],
    }
}

/// Locates an interpreter on PATH, or explains what is missing.
pub fn resolve(name: &str) -> Result<Interpreter, String> {
    let options = candidates(name);
    if options.is_empty() {
        return Err(format!(
            "unsupported interpreter '{name}' (use python3, node, bash, sh, pwsh, deno, or ruby)"
        ));
    }
    for (program, leading) in &options {
        if let Some(path) = find_on_path(program) {
            return Ok(Interpreter {
                program: path,
                leading_args: leading.iter().map(|s| s.to_string()).collect(),
            });
        }
    }
    let tried: Vec<&str> = options.iter().map(|(p, _)| *p).collect();
    Err(format!(
        "no interpreter for '{name}' is installed (looked for {})",
        tried.join(", ")
    ))
}

/// The logical interpreter a file extension implies, for scripts found in a
/// skill's `scripts/` directory rather than declared in its frontmatter.
///
/// Extension only. A shebang would be more accurate, but this runs on every
/// file of every skill at load time, and a wrong guess here is not silent — the
/// script is listed with the interpreter it was guessed to need, and an
/// unsupported guess simply yields nothing.
pub fn for_extension(extension: &str) -> Option<&'static str> {
    Some(match extension.to_ascii_lowercase().as_str() {
        "py" => "python3",
        "js" | "mjs" | "cjs" => "node",
        "ts" => "deno",
        "sh" | "bash" => "bash",
        "ps1" => "pwsh",
        "rb" => "ruby",
        _ => return None,
    })
}

/// Whether every interpreter a set of scripts needs is available here.
pub fn missing_interpreters<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut missing = Vec::new();
    for name in names {
        if let Err(reason) = resolve(name) {
            if !missing.contains(&reason) {
                missing.push(reason);
            }
        }
    }
    missing
}

/// PATH lookup, including Windows' PATHEXT suffixes.
///
/// Shared with the MCP client rather than duplicated, because both are asking
/// the same question of the same PATH and the answer has to be the same one. It
/// is also the PATH `taurus_tools::login_path::adopt` repaired at startup: an
/// interpreter installed by nvm or pyenv is invisible to an app launched from
/// the Dock until that has run, and reporting it as "not installed" sends the
/// user looking for a missing package they already have.
fn find_on_path(program: &str) -> Option<PathBuf> {
    taurus_tools::login_path::which(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_a_shell_that_exists_everywhere() {
        let sh = resolve("sh").expect("sh must resolve on every supported platform");
        assert!(sh.program.exists());
    }

    #[test]
    fn interpreter_names_are_case_insensitive() {
        assert!(resolve("SH").is_ok());
    }

    #[test]
    fn reports_an_unsupported_interpreter_with_the_supported_list() {
        let err = resolve("brainfuck").unwrap_err();
        assert!(err.contains("unsupported interpreter"));
        assert!(err.contains("python3"));
    }

    #[test]
    fn missing_interpreters_reports_only_the_absent_ones() {
        let missing = missing_interpreters(["sh", "brainfuck"].into_iter());
        assert_eq!(missing.len(), 1);
        assert!(missing[0].contains("brainfuck"));
    }

    #[test]
    fn nothing_is_missing_when_everything_resolves() {
        assert!(missing_interpreters(["sh"].into_iter()).is_empty());
    }
}

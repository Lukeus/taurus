//! Confines filesystem access to the workspace.
//!
//! Lexical normalization alone is not enough: `workspace/link -> /etc` passes
//! any string check while resolving outside the boundary. So we canonicalize
//! the deepest ancestor that actually exists (which resolves every symlink on
//! the way) and only then re-attach the not-yet-created tail. That covers both
//! reads of existing paths and writes of new ones.

use std::path::{Component, Path, PathBuf};

use crate::tool::ToolError;

/// Resolves `candidate` against `root` and rejects anything that escapes it.
///
/// Relative paths are taken as relative to `root`. The returned path is
/// absolute and symlink-free up to the first component that does not exist.
pub fn resolve(root: &Path, candidate: &str) -> Result<PathBuf, ToolError> {
    resolve_within(root, &[], candidate)
}

/// Resolves `candidate` against `root`, also accepting anything under one of
/// `also`.
///
/// For read-only tools, whose reach is widened by the skill directories the
/// catalog loaded. Writes never call this: a skill's own files are somebody
/// else's — often another client's — and the workspace stays the only place
/// this agent modifies.
///
/// `root` alone anchors relative paths, so which directory `references/x.md`
/// means never depends on the allowlist. A skill's procedure is given absolute
/// paths for exactly that reason.
pub fn resolve_within(
    root: &Path,
    also: &[PathBuf],
    candidate: &str,
) -> Result<PathBuf, ToolError> {
    if candidate.trim().is_empty() {
        return Err(ToolError::InvalidInput("path must not be empty".into()));
    }

    let root = canonical_root(root)?;
    let raw = Path::new(candidate);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        root.join(raw)
    };

    let lexical = lexically_normalize(&joined);

    // Walk back to the deepest existing ancestor, canonicalize it, then replay
    // the remaining components.
    let mut existing = lexical.as_path();
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    let resolved = loop {
        match existing.canonicalize() {
            Ok(p) => break p,
            Err(_) => match existing.parent() {
                Some(parent) => {
                    if let Some(name) = existing.file_name() {
                        tail.push(name);
                    }
                    existing = parent;
                }
                // Ran out of ancestors without finding one that exists.
                None => {
                    return Err(ToolError::InvalidInput(format!(
                        "cannot resolve path: {candidate}"
                    )))
                }
            },
        }
    };

    let mut full = resolved;
    for name in tail.into_iter().rev() {
        full.push(name);
    }

    // Each allowed root is canonicalized too, or a symlinked skill directory
    // would never match the resolved path it is supposed to permit.
    let permitted = full.starts_with(&root)
        || also
            .iter()
            .filter_map(|dir| dir.canonicalize().ok())
            .any(|dir| full.starts_with(&dir));

    if !permitted {
        return Err(ToolError::OutsideWorkspace {
            path: full.display().to_string(),
            root: root.display().to_string(),
        });
    }
    Ok(full)
}

fn canonical_root(root: &Path) -> Result<PathBuf, ToolError> {
    root.canonicalize().map_err(|_| {
        // Hit on every tool call once the directory goes away, so it has to
        // explain itself rather than surface a bare errno.
        ToolError::Failed(format!(
            "The workspace {} no longer exists. It may have been deleted or unmounted. \
             Choose another workspace to continue.",
            root.display()
        ))
    })
}

/// Removes `.` and resolves `..` textually, without touching the filesystem.
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(Component::ParentDir);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Path shown in prompts and tool output: relative to the workspace when
/// possible, so the model and the user see stable, short names.
///
/// Separators are always `/`, including on Windows. This string is not merely
/// cosmetic: the model reads it and writes it straight back into the next tool
/// call, and `resolve` accepts either separator on Windows. Emitting the native
/// `\` would make every path in a transcript, a skill, and a stored session
/// platform-specific for no gain, and would collide with JSON escaping on the
/// way through.
pub fn display(root: &Path, path: &Path) -> String {
    let shown = root
        .canonicalize()
        .ok()
        .and_then(|r| path.strip_prefix(r).ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| path.display().to_string());
    with_forward_slashes(&shown, std::path::MAIN_SEPARATOR)
}

/// Separator rewriting, with the platform's separator passed in.
///
/// A parameter rather than reading `MAIN_SEPARATOR` directly so the Windows
/// behavior is exercised by the test suite on every platform. A test that can
/// only run on the system that already works proves nothing about the one that
/// does not.
fn with_forward_slashes(shown: &str, separator: char) -> String {
    if separator == '/' {
        shown.to_string()
    } else {
        shown.replace(separator, "/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/file.txt"), "hi").unwrap();
        dir
    }

    #[test]
    fn accepts_a_path_under_an_allowed_root() {
        let ws = workspace();
        let skill = workspace();
        let target = skill.path().join("sub/file.txt");

        // Refused without the allowance, permitted with it. Both halves matter:
        // the first is the boundary still doing its job.
        let candidate = target.to_str().unwrap();
        assert!(matches!(
            resolve(ws.path(), candidate),
            Err(ToolError::OutsideWorkspace { .. })
        ));
        let allowed = vec![skill.path().to_path_buf()];
        assert!(resolve_within(ws.path(), &allowed, candidate).is_ok());
    }

    #[test]
    fn an_allowed_root_does_not_open_its_neighbours() {
        let ws = workspace();
        let skill = workspace();
        let other = workspace();
        let allowed = vec![skill.path().to_path_buf()];

        let outside = other.path().join("sub/file.txt");
        assert!(matches!(
            resolve_within(ws.path(), &allowed, outside.to_str().unwrap()),
            Err(ToolError::OutsideWorkspace { .. })
        ));
    }

    #[test]
    fn a_relative_path_still_means_the_workspace() {
        let ws = workspace();
        let skill = workspace();
        let allowed = vec![skill.path().to_path_buf()];

        // Both roots contain `sub/file.txt`. Which one a bare relative path
        // means must not depend on what happens to be allowlisted.
        let resolved = resolve_within(ws.path(), &allowed, "sub/file.txt").unwrap();
        assert!(resolved.starts_with(ws.path().canonicalize().unwrap()));
    }

    #[test]
    fn accepts_a_relative_path_inside_the_workspace() {
        let ws = workspace();
        let p = resolve(ws.path(), "sub/file.txt").unwrap();
        assert!(p.ends_with("sub/file.txt"));
    }

    #[test]
    fn accepts_a_path_that_does_not_exist_yet() {
        let ws = workspace();
        let p = resolve(ws.path(), "sub/new/deep.txt").unwrap();
        assert!(p.ends_with("sub/new/deep.txt"));
    }

    #[test]
    fn rejects_parent_traversal() {
        let ws = workspace();
        let err = resolve(ws.path(), "../../etc/passwd").unwrap_err();
        assert!(matches!(err, ToolError::OutsideWorkspace { .. }));
    }

    #[test]
    fn rejects_traversal_disguised_by_a_real_subdirectory() {
        let ws = workspace();
        let err = resolve(ws.path(), "sub/../../outside.txt").unwrap_err();
        assert!(matches!(err, ToolError::OutsideWorkspace { .. }));
    }

    #[test]
    fn rejects_an_absolute_path_outside_the_workspace() {
        let ws = workspace();
        let err = resolve(ws.path(), "/etc/passwd").unwrap_err();
        assert!(matches!(err, ToolError::OutsideWorkspace { .. }));
    }

    #[test]
    fn accepts_an_absolute_path_inside_the_workspace() {
        let ws = workspace();
        let inside = ws.path().join("sub/file.txt");
        assert!(resolve(ws.path(), inside.to_str().unwrap()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink_pointing_outside() {
        // The case a purely lexical check cannot catch.
        let ws = workspace();
        std::os::unix::fs::symlink("/etc", ws.path().join("escape")).unwrap();
        let err = resolve(ws.path(), "escape/passwd").unwrap_err();
        assert!(matches!(err, ToolError::OutsideWorkspace { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn follows_a_symlink_that_stays_inside() {
        let ws = workspace();
        std::os::unix::fs::symlink(ws.path().join("sub"), ws.path().join("alias")).unwrap();
        assert!(resolve(ws.path(), "alias/file.txt").is_ok());
    }

    #[test]
    fn a_vanished_workspace_explains_itself() {
        let ws = workspace();
        let root = ws.path().to_path_buf();
        drop(ws);
        let err = resolve(&root, ".").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("no longer exists"), "{message}");
        assert!(message.contains("Choose another workspace"), "{message}");
    }

    #[test]
    fn shown_paths_use_forward_slashes_on_every_platform() {
        // The model reads these and writes them back into the next tool call,
        // so a nested path must look the same on Windows as anywhere else.
        let ws = workspace();
        let nested = resolve(ws.path(), "sub/file.txt").unwrap();
        assert_eq!(display(ws.path(), &nested), "sub/file.txt");
        assert!(
            !display(ws.path(), &nested).contains('\\'),
            "a native separator leaked into model-facing output"
        );
    }

    #[test]
    fn a_path_outside_the_workspace_is_still_normalized() {
        // Falls back to the absolute path; it should not switch separator
        // style halfway through a transcript.
        let ws = workspace();
        let outside = std::env::temp_dir().join("a").join("b");
        assert!(!display(ws.path(), &outside).contains('\\'));
    }

    #[test]
    fn windows_separators_are_rewritten_even_when_the_tests_run_elsewhere() {
        // The actual regression, reproduced on any platform: `glob` returned
        // `src\main.rs` on Windows, which the model then echoed back into the
        // next tool call.
        assert_eq!(with_forward_slashes(r"src\main.rs", '\\'), "src/main.rs");
        assert_eq!(
            with_forward_slashes(r"C:\Users\me\project\a.txt", '\\'),
            "C:/Users/me/project/a.txt"
        );
        // A POSIX path is untouched, including one with a backslash in a
        // filename -- legal on Unix, and not a separator there.
        assert_eq!(with_forward_slashes("src/main.rs", '/'), "src/main.rs");
        assert_eq!(with_forward_slashes(r"odd\name.txt", '/'), r"odd\name.txt");
    }

    #[test]
    fn rejects_an_empty_path() {
        let ws = workspace();
        assert!(matches!(
            resolve(ws.path(), "  ").unwrap_err(),
            ToolError::InvalidInput(_)
        ));
    }
}

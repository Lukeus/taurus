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

    if !full.starts_with(&root) {
        return Err(ToolError::OutsideWorkspace {
            path: full.display().to_string(),
            root: root.display().to_string(),
        });
    }
    Ok(full)
}

fn canonical_root(root: &Path) -> Result<PathBuf, ToolError> {
    root.canonicalize().map_err(|e| {
        ToolError::InvalidInput(format!(
            "workspace root {} is unusable: {e}",
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
pub fn display(root: &Path, path: &Path) -> String {
    root.canonicalize()
        .ok()
        .and_then(|r| path.strip_prefix(r).ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| path.display().to_string())
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
    fn rejects_an_empty_path() {
        let ws = workspace();
        assert!(matches!(
            resolve(ws.path(), "  ").unwrap_err(),
            ToolError::InvalidInput(_)
        ));
    }
}

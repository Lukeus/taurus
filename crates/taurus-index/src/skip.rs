//! What the index leaves out even though git tracks it.
//!
//! Embedding is the slow half of search, and the built-in engine runs it on
//! the CPU, so every passage costs real time. On this repository nearly half
//! of them were somebody else's code: a vendored copy of whisper.cpp (7,785 of
//! 17,390) plus lockfiles (512). Nobody asks a question that a lockfile or a
//! CUDA kernel from a dependency answers, and both crowd the files that do
//! answer out of the results.
//!
//! # The rules
//!
//! Taken from GitHub's linguist, which decides what counts as vendored or
//! generated for every repository on GitHub, so they're rules a project
//! has usually already met:
//!
//! - **By path** ([`DEFAULTS`]): vendored dependency folders (linguist's
//!   `vendor.yml`), lockfiles, minified files and source maps (its
//!   `generated.rb`). Cursor leaves lockfiles out of its index by default
//!   too.
//! - **By content** ([`generated`]): a header that says the file was
//!   generated, the markers code generators write (`@generated`,
//!   `DO NOT EDIT`), and minified JavaScript and CSS by their line length.
//!
//! Left out on purpose: linguist also treats `dist/`, `cache/` and
//! `testdata/` as vendored. The first two are almost always gitignored
//! already, and test fixtures are sometimes the answer to a question.
//!
//! # Changing them
//!
//! `.taurus/index-ignore`, in `.gitignore` syntax, read after the defaults:
//! a line adds a rule, and a `!` line takes a default back (`!vendor/` puts
//! vendored code into the index). As with git, a file inside a folder that's
//! left out can't be put back on its own. This file changes only the index:
//! grep and glob still see everything.
//!
//! # Only this index
//!
//! The search tools walk the whole tree. grep is how the model finds a symbol
//! in a vendored library, and that stays as it was.

use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// Where a project changes the rules, relative to the workspace.
pub const IGNORE_FILE: &str = ".taurus/index-ignore";

/// Bumped when the rules change in a way that would leave out a file an
/// older index already holds, so that index is screened once more. See
/// `Index::screened`.
pub const RULES: u32 = 1;

/// Left out by path, in `.gitignore` syntax.
pub const DEFAULTS: &[&str] = &[
    // Vendored dependencies (linguist's vendor.yml).
    "vendor/",
    "vendors/",
    "third_party/",
    "third-party/",
    "thirdparty/",
    "3rdparty/",
    "3rd_party/",
    "3rd-party/",
    "node_modules/",
    "bower_components/",
    "Pods/",
    "Carthage/",
    "Godeps/_workspace/",
    // Lockfiles (linguist's generated.rb; Cursor's defaults).
    "*.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "bun.lockb",
    "go.sum",
    // Minified files and source maps.
    "*.min.js",
    "*.min.css",
    "*-min.js",
    "*-min.css",
    "*.js.map",
    "*.css.map",
];

/// How far into a file [`generated`] looks for a generator's marker. Tools
/// write theirs at the very top; looking further finds files that only talk
/// about markers, this one among them, which is how the limit was found.
const MARKER_LINES: usize = 5;

/// How far it looks for Go's marker, which the Go tools accept anywhere
/// before the package clause, so after a licence header too. linguist looks
/// forty lines in.
const HEAD_LINES: usize = 40;

/// The rules for one workspace.
pub struct Skip {
    rules: Gitignore,
}

impl Skip {
    /// The defaults plus the workspace's own file, and a problem with that
    /// file when there is one. A file that can't be read or has a bad line
    /// leaves the rest in force: the index still gets built, and the problem
    /// says which line.
    pub fn for_workspace(workspace: &Path) -> (Self, Option<String>) {
        let mut builder = GitignoreBuilder::new(workspace);
        for line in DEFAULTS {
            // The defaults are fixed and tested; a failure is a bug here.
            let _ = builder.add_line(None, line);
        }
        let own: PathBuf = workspace.join(IGNORE_FILE);
        let mut problem = None;
        if own.is_file() {
            problem = builder
                .add(&own)
                .map(|e| format!("{IGNORE_FILE}: {e}. The rest of it applies."));
        }
        let rules = builder.build().unwrap_or_else(|e| {
            problem = Some(format!("{IGNORE_FILE}: {e}. Only the defaults apply."));
            defaults(workspace)
        });
        (Self { rules }, problem)
    }

    /// Whether `path`, inside the workspace, is left out.
    pub fn path(&self, path: &Path, is_dir: bool) -> bool {
        self.rules.matched(path, is_dir).is_ignore()
    }
}

fn defaults(workspace: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(workspace);
    for line in DEFAULTS {
        let _ = builder.add_line(None, line);
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

/// Whether a file says it was generated, from its name and its first lines.
///
/// `head` may be the whole file or only its start: this reads no further
/// than [`HEAD_LINES`] lines.
pub fn generated(name: &str, head: &str) -> bool {
    let lines: Vec<&str> = head.lines().take(HEAD_LINES).collect();
    let marked = lines.iter().take(MARKER_LINES).any(|line| {
        // `@generated` is the convention Cargo, Buck and Meta's tools write;
        // `DO NOT EDIT` is protoc's and many others'; the last is bindgen's.
        line.contains("@generated")
            || line.contains("DO NOT EDIT")
            || line.contains("automatically generated by")
    });
    // Go's, exactly as its tools match it: `^// Code generated .* DO NOT EDIT\.$`.
    let go = lines.iter().any(|line| {
        let line = line.trim_end();
        line.starts_with("// Code generated ") && line.ends_with(" DO NOT EDIT.")
    });
    if marked || go {
        return true;
    }
    // linguist's test for a minified file: JavaScript or CSS whose lines
    // average more than 110 characters.
    let lower = name.to_ascii_lowercase();
    let minifiable = [".js", ".mjs", ".cjs", ".css"]
        .iter()
        .any(|ext| lower.ends_with(ext));
    if minifiable && !lines.is_empty() {
        let total: usize = lines.iter().map(|l| l.len()).sum();
        return total / lines.len() > 110;
    }
    false
}

/// The start of a file, enough for [`generated`], without reading the rest.
/// `None` for a file that can't be read or isn't text.
pub fn head(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut buf = vec![0u8; 8 * 1024];
    let mut file = std::fs::File::open(path).ok()?;
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => return None,
        }
    }
    buf.truncate(filled);
    // A cut through a multi-byte character at the end is not a reason to
    // call the file binary.
    match String::from_utf8(buf) {
        Ok(text) => Some(text),
        Err(e) => {
            let valid = e.utf8_error().valid_up_to();
            if filled - valid < 4 {
                let mut bytes = e.into_bytes();
                bytes.truncate(valid);
                String::from_utf8(bytes).ok()
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn skip(dir: &Path) -> Skip {
        Skip::for_workspace(dir).0
    }

    #[test]
    fn vendored_folders_and_lockfiles_are_left_out() {
        let dir = TempDir::new().unwrap();
        let s = skip(dir.path());
        let root = dir.path();
        assert!(s.path(&root.join("vendor"), true));
        assert!(s.path(&root.join("crates/x/third_party"), true));
        assert!(s.path(&root.join("Cargo.lock"), false));
        assert!(s.path(&root.join("web/pnpm-lock.yaml"), false));
        assert!(s.path(&root.join("app.min.js"), false));
        assert!(!s.path(&root.join("src"), true));
        assert!(!s.path(&root.join("src/vendor.rs"), false));
        assert!(!s.path(&root.join("Cargo.toml"), false));
    }

    #[test]
    fn the_workspace_file_adds_rules_and_takes_defaults_back() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".taurus")).unwrap();
        std::fs::write(dir.path().join(IGNORE_FILE), "fixtures/\n!vendor/\n").unwrap();
        let (s, problem) = Skip::for_workspace(dir.path());
        assert_eq!(problem, None);
        assert!(s.path(&dir.path().join("fixtures"), true));
        assert!(!s.path(&dir.path().join("vendor"), true));
        assert!(s.path(&dir.path().join("Cargo.lock"), false));
    }

    #[test]
    fn a_bad_line_says_where_and_keeps_the_rest() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".taurus")).unwrap();
        std::fs::write(dir.path().join(IGNORE_FILE), "fixtures/\n[z-a]\n").unwrap();
        let (s, problem) = Skip::for_workspace(dir.path());
        assert!(problem.unwrap().contains(IGNORE_FILE));
        assert!(s.path(&dir.path().join("fixtures"), true));
        assert!(s.path(&dir.path().join("vendor"), true));
    }

    #[test]
    fn generator_markers_are_recognized() {
        assert!(generated(
            "Cargo.lock",
            "# This file is automatically @generated by Cargo.\n"
        ));
        assert!(generated(
            "x.go",
            "// Code generated by protoc-gen-go. DO NOT EDIT.\npackage x\n"
        ));
        assert!(generated(
            "bindings.rs",
            "/* automatically generated by rust-bindgen 0.69.4 */\n"
        ));
        assert!(!generated("lib.rs", "//! A library.\nfn main() {}\n"));
    }

    #[test]
    fn a_marker_below_the_top_is_a_mention_not_a_header() {
        let late = format!(
            "{}//! tools write `@generated` or `DO NOT EDIT`\n",
            "//! docs\n".repeat(MARKER_LINES)
        );
        assert!(!generated("skip.rs", &late));
    }

    #[test]
    fn go_s_marker_counts_after_a_licence_header() {
        let licensed = format!(
            "{}// Code generated by stringer -type=Pill; DO NOT EDIT.\n\npackage x\n",
            "// Copyright and licence.\n".repeat(20)
        );
        assert!(generated("pill_string.go", &licensed));
        let past = format!(
            "{}// Code generated by stringer; DO NOT EDIT.\n",
            "// licence\n".repeat(HEAD_LINES)
        );
        assert!(!generated("x.go", &past));
    }

    #[test]
    fn minified_scripts_are_recognized_by_line_length() {
        let long = format!("{}\n", "a=1;".repeat(100));
        assert!(generated("bundle.js", &long));
        assert!(!generated("notes.md", &long));
        assert!(!generated("app.js", "const a = 1;\nconst b = 2;\n"));
    }

    #[test]
    fn head_reads_a_file_cut_inside_a_character() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("x.txt");
        // 8 KiB of ASCII then a three-byte character straddling the cut.
        let mut text = "a".repeat(8 * 1024 - 1);
        text.push('€');
        std::fs::write(&path, &text).unwrap();
        let head = head(&path).unwrap();
        assert_eq!(head.len(), 8 * 1024 - 1);
        std::fs::write(&path, [0xff, 0xfe, 0x00, 0x01, 0xff, 0xff]).unwrap();
        assert!(super::head(&path).is_none());
    }
}

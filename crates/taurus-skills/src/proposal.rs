//! Auto-generated skills: proposal, validation, and the write on approval.
//!
//! Nothing here touches the skill library. A proposal is inert data until the
//! user approves it in the UI, which is the whole point: a model that can
//! silently write its own future instructions can also silently poison them.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::catalog::{SkillCatalog, SKILL_FILE};
use crate::interpreter;
use crate::skill::{validate, SkillFrontmatter, SkillScript};

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProposedScript {
    pub path: String,
    pub interpreter: String,
    #[serde(default)]
    pub description: String,
    /// Full source of the script, shown to the user before approval.
    pub content: String,
}

/// A file other than a script, bundled where the Agent Skills specification
/// puts it: `references/` for documentation the procedure points at, and
/// `assets/` for templates and data. Text only.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProposedFile {
    /// Relative to the skill: `references/API.md`, `assets/template.json`.
    pub path: String,
    /// Shown to the user before approval, like a script's source.
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillProposal {
    pub id: String,
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    /// The procedure, as markdown.
    pub body: String,
    #[serde(default)]
    pub scripts: Vec<ProposedScript>,
    /// References and assets to bundle beside the procedure.
    #[serde(default)]
    pub files: Vec<ProposedFile>,
    /// Why the agent thinks this is worth keeping. Shown in the review card;
    /// never written to disk.
    #[serde(default)]
    pub rationale: String,
    /// True when a skill of this name already exists and would be replaced.
    #[serde(default)]
    pub replaces_existing: bool,
}

impl SkillProposal {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        when_to_use: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            description: description.into(),
            when_to_use: when_to_use.into(),
            body: body.into(),
            scripts: Vec::new(),
            files: Vec::new(),
            rationale: String::new(),
            replaces_existing: false,
        }
    }

    fn frontmatter(&self) -> SkillFrontmatter {
        SkillFrontmatter {
            name: self.name.clone(),
            description: self.description.clone(),
            // Always written, even though the format makes it optional: a
            // skill Taurus authors should carry the sharper trigger line, and
            // other clients ignore the field rather than choke on it.
            when_to_use: Some(self.when_to_use.clone()),
            version: 1,
            compatibility: None,
            // A skill Taurus writes is reachable both ways: the model chose to
            // write it down, and the user can call for it by name.
            disable_model_invocation: false,
            user_invocable: true,
            allowed_tools: Vec::new(),
            scripts: self
                .scripts
                .iter()
                .map(|s| SkillScript {
                    path: s.path.clone(),
                    interpreter: s.interpreter.clone(),
                    description: s.description.clone(),
                })
                .collect(),
            argument_hint: None,
        }
    }
}

/// Where an approved proposal gets written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SaveTarget {
    /// `<workspace>/.taurus/skills` — travels with the project.
    Project,
    /// `~/.taurus/skills` — available in every workspace.
    User,
}

/// Command lines that are never appropriate in a generated skill.
///
/// This is a floor, not a sandbox: the user still reviews the script, and
/// `run_skill_script` still goes through the permission gate. It exists so an
/// obviously destructive proposal never reaches a review card, where a tired
/// user might wave it through.
const DENIED_PATTERNS: &[(&str, &str)] = &[
    ("rm -rf /", "recursive delete from the filesystem root"),
    ("rm -fr /", "recursive delete from the filesystem root"),
    ("mkfs", "filesystem creation"),
    ("dd if=", "raw device write"),
    (":(){", "fork bomb"),
    ("shutdown", "shutting the machine down"),
    ("reboot", "rebooting the machine"),
    ("format c:", "formatting a drive"),
    (
        "remove-item -recurse -force c:\\",
        "recursive delete of the system drive",
    ),
    ("history -c", "clearing shell history"),
    ("chmod -r 777 /", "making the filesystem world-writable"),
];

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ProposalRejected(pub String);

/// Where a script goes when a proposal names only the file.
///
/// The Agent Skills specification keeps a skill's executable code in
/// `scripts/`, and a SKILL.md names it from the skill's root:
/// `scripts/extract.py`. A bare `extract.py` is the commonest way to get that
/// wrong, and the one with an unambiguous fix, so it's placed rather than
/// refused. Any other folder is refused by [`validate_proposal`].
pub fn place_script(path: &str) -> String {
    let path = path.trim().trim_start_matches("./");
    if path.contains('/') || path.contains('\\') {
        path.to_string()
    } else {
        format!("{SCRIPTS_DIR}{path}")
    }
}

const SCRIPTS_DIR: &str = "scripts/";
/// The specification's folders for everything that isn't a script.
const FILE_DIRS: &[&str] = &["references/", "assets/"];

/// A path a skill can hold: relative, inside it, in one of `dirs`, and
/// spelled the way every platform reads the same.
fn check_path(path: &str, dirs: &[&str], what: &str) -> Result<(), ProposalRejected> {
    let refuse = |why: &str| Err(ProposalRejected(format!("{what} '{path}' {why}")));
    if path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return refuse("must be a path inside the skill, written with forward slashes");
    }
    if !dirs
        .iter()
        .any(|dir| path.starts_with(dir) && path.len() > dir.len())
    {
        return refuse(&format!(
            "has to be in {}, where the Agent Skills format keeps it",
            dirs.join(" or ")
        ));
    }
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return refuse("can't step outside the skill's folder or have an empty part");
    }
    Ok(())
}

/// Checks a proposal before it reaches the user.
///
/// Everything here is something a human reviewer would either miss or find
/// tedious to check: schema conformance, name collisions, near-duplicates of
/// skills that already exist, and scripts that do something categorically
/// unacceptable.
pub fn validate_proposal(
    proposal: &SkillProposal,
    catalog: &SkillCatalog,
) -> Result<(), ProposalRejected> {
    let frontmatter = proposal.frontmatter();
    // Loading from disk tolerates these, because a skill someone already has
    // installed is better read than refused. Nothing is already installed here:
    // the file does not exist yet, and writing one that only loads by leniency
    // would be choosing to create the problem the leniency exists to survive.
    let warnings =
        validate(&frontmatter, "proposal").map_err(|e| ProposalRejected(e.to_string()))?;
    if let Some(first) = warnings.first() {
        return Err(ProposalRejected(first.clone()));
    }

    if proposal.body.trim().len() < 40 {
        return Err(ProposalRejected(
            "the procedure is too short to be useful; write the actual steps".into(),
        ));
    }

    // Every declared script must ship its source, and live where the format
    // says. The other direction — the procedure naming a file the proposal
    // didn't include — is `check_bundled`'s.
    let mut seen = std::collections::BTreeSet::new();
    for script in &proposal.scripts {
        check_path(&script.path, &[SCRIPTS_DIR], "script")?;
        if !seen.insert(script.path.as_str()) {
            return Err(ProposalRejected(format!(
                "'{}' is in the proposal twice",
                script.path
            )));
        }
        // What runs a script is read from its extension when the skill loads,
        // since the format has nowhere to declare it. The two have to agree,
        // or the script is run by something it wasn't written for.
        let extension = Path::new(&script.path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        match interpreter::for_extension(extension) {
            None => {
                return Err(ProposalRejected(format!(
                    "script '{}' needs an extension that says what runs it: .py, .js, .ts, .sh, \
                     .ps1 or .rb",
                    script.path
                )))
            }
            Some(runs) => {
                let declared = script.interpreter.trim();
                let same = runs == declared || (runs == "bash" && declared == "sh");
                if !same {
                    return Err(ProposalRejected(format!(
                        "script '{}' is a .{extension} file, which runs with {runs}, not \
                         {declared}. Rename it or change the interpreter",
                        script.path
                    )));
                }
            }
        }
        if script.content.trim().is_empty() {
            return Err(ProposalRejected(format!(
                "script '{}' has no content",
                script.path
            )));
        }
        if let Err(reason) = interpreter::resolve(&script.interpreter) {
            // Unsupported is fatal; merely missing here is not, since the skill
            // may be used on another machine.
            if reason.contains("unsupported") {
                return Err(ProposalRejected(reason));
            }
        }
        let lowered = script.content.to_lowercase();
        for (pattern, why) in DENIED_PATTERNS {
            if lowered.contains(pattern) {
                return Err(ProposalRejected(format!(
                    "script '{}' contains {why}; refusing to propose it",
                    script.path
                )));
            }
        }
    }

    for file in &proposal.files {
        check_path(&file.path, FILE_DIRS, "file")?;
        if !seen.insert(file.path.as_str()) {
            return Err(ProposalRejected(format!(
                "'{}' is in the proposal twice",
                file.path
            )));
        }
        if file.content.trim().is_empty() {
            return Err(ProposalRejected(format!("file '{}' is empty", file.path)));
        }
    }

    if let Some(existing) = near_duplicate(proposal, catalog) {
        return Err(ProposalRejected(format!(
            "'{existing}' already covers this. Extend that skill instead of adding another."
        )));
    }

    Ok(())
}

/// Checks that the procedure carries what it uses.
///
/// A skill is a folder that works wherever it's copied, which is the point of
/// the format. So a path the procedure names under `scripts/`, `references/`
/// or `assets/` has to be in the proposal, and so does any script it runs. A
/// helper the model wrote into the workspace while it worked is exactly the
/// file that isn't there the next time.
///
/// `in_project` is asked about each path before it's refused: a file the
/// project keeps in version control is the project's, and a project skill may
/// lean on it. Everything else is refused, with what to do instead.
pub fn check_bundled(
    proposal: &SkillProposal,
    in_project: impl Fn(&str) -> bool,
) -> Result<(), ProposalRejected> {
    let bundled: std::collections::BTreeSet<&str> = proposal
        .scripts
        .iter()
        .map(|s| s.path.as_str())
        .chain(proposal.files.iter().map(|f| f.path.as_str()))
        .collect();
    for path in named_paths(&proposal.body) {
        if !bundled.contains(path.as_str()) && !in_project(&path) {
            let list = if path.starts_with(SCRIPTS_DIR) {
                "`scripts`"
            } else {
                "`files`"
            };
            return Err(ProposalRejected(format!(
                "the procedure points at `{path}`, which the proposal doesn't include. Add it to \
                 {list} with its contents, or take the reference out"
            )));
        }
    }
    for path in run_scripts(&proposal.body) {
        let local = path.trim_start_matches("./");
        if bundled.contains(local) || in_project(local) {
            continue;
        }
        let name = Path::new(local)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| local.to_string());
        return Err(ProposalRejected(format!(
            "the procedure runs `{path}`, which isn't in the skill. A skill carries its own \
             scripts: add it to `scripts` with its source as scripts/{name}, and run it from \
             there"
        )));
    }
    Ok(())
}

/// Every `scripts/…`, `references/…` and `assets/…` path the text names.
fn named_paths(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for dir in std::iter::once(SCRIPTS_DIR).chain(FILE_DIRS.iter().copied()) {
        let mut from = 0;
        while let Some(at) = text[from..].find(dir).map(|i| i + from) {
            from = at + dir.len();
            // The start of a path, not the middle of one: `src/scripts/x` is
            // a project path that happens to contain the word.
            let before = text[..at].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || "_-/.".contains(c)) {
                continue;
            }
            let rest: String = text[from..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || "._-/".contains(*c))
                .collect();
            let rest = rest.trim_end_matches(['.', '/']);
            if !rest.is_empty() {
                found.push(format!("{dir}{rest}"));
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Script files the text runs: the word after an interpreter, when it ends
/// in a script's extension, and a `./name.sh` on its own.
fn run_scripts(text: &str) -> Vec<String> {
    const RUNNERS: &[&str] = &[
        "python", "python3", "node", "bash", "sh", "zsh", "deno", "ruby", "pwsh", "tsx", "bun",
    ];
    const EXTENSIONS: &[&str] = &["py", "js", "mjs", "cjs", "ts", "sh", "bash", "rb", "ps1"];
    let is_script = |word: &str| {
        Path::new(word)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| EXTENSIONS.contains(&e))
    };
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || "`\"'()[]{};|&".contains(c))
        .filter(|w| !w.is_empty())
        .collect();
    let mut found = Vec::new();
    for (i, word) in words.iter().enumerate() {
        let after_runner = i > 0 && RUNNERS.contains(&words[i - 1]);
        let direct = word.starts_with("./") || word.starts_with('/');
        if is_script(word) && (after_runner || direct) && !word.starts_with(SCRIPTS_DIR) {
            found.push(word.trim_end_matches([',', '.', ':']).to_string());
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Finds an existing skill that covers the same ground.
///
/// Deliberately shallow — token overlap on the trigger line, which is what
/// decides when a skill fires. Two skills with different names and near
/// identical triggers are the failure mode that makes a library useless, and
/// this catches it without an embedding model.
///
/// Compared against the resolved trigger rather than the raw field, so a
/// proposal is also checked against skills borrowed from another client, which
/// carry no `when_to_use` at all.
fn near_duplicate(proposal: &SkillProposal, catalog: &SkillCatalog) -> Option<String> {
    let proposed = token_set(&proposal.when_to_use);
    if proposed.is_empty() {
        return None;
    }
    for name in catalog.names() {
        let skill = catalog.get(name)?;
        if skill.name() == proposal.name {
            // Same name is an update, handled separately by `replaces_existing`.
            continue;
        }
        let existing = token_set(&skill.trigger());
        let shared = proposed.iter().filter(|t| existing.contains(*t)).count();
        let smaller = proposed.len().min(existing.len());
        if smaller >= 3 && shared * 4 >= smaller * 3 {
            return Some(name.to_string());
        }
    }
    None
}

fn token_set(text: &str) -> std::collections::BTreeSet<String> {
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "and", "or", "to", "of", "for", "in", "on", "is", "are", "when", "user",
        "asks", "about", "with", "it", "that", "this", "be", "as", "at", "by",
    ];
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2 && !STOPWORDS.contains(w))
        .map(String::from)
        .collect()
}

/// Writes an approved proposal into a skill directory.
///
/// Called only after the user accepts. Returns the directory created.
pub fn save(proposal: &SkillProposal, skills_root: &Path) -> std::io::Result<PathBuf> {
    let dir = skills_root.join(&proposal.name);
    std::fs::create_dir_all(&dir)?;

    std::fs::write(dir.join(SKILL_FILE), skill_md(proposal))?;

    for file in &proposal.files {
        let rel = Path::new(&file.path);
        if rel.is_absolute() || file.path.contains("..") {
            continue;
        }
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &file.content)?;
    }

    for script in &proposal.scripts {
        // Validation already rejected traversal, but the write is the last
        // place this can go wrong, so re-check rather than trust.
        let rel = Path::new(&script.path);
        if rel.is_absolute() || script.path.contains("..") {
            continue;
        }
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &script.content)?;
        make_executable(&path);
    }

    Ok(dir)
}

/// The SKILL.md an approved proposal becomes.
///
/// Frontmatter the Agent Skills specification allows and nothing else:
/// `name`, `description`, and `metadata` for what only Taurus reads — the
/// trigger line — so `skills-ref validate` passes it and any client reads it.
/// The scripts can't be declared there, so they're described in the body,
/// where every client's model reads them, under the paths they're saved at.
pub fn skill_md(proposal: &SkillProposal) -> String {
    let mut metadata = serde_yaml_ng::Mapping::new();
    metadata.insert("when_to_use".into(), proposal.when_to_use.trim().into());
    let mut front = serde_yaml_ng::Mapping::new();
    front.insert("name".into(), proposal.name.as_str().into());
    front.insert("description".into(), proposal.description.trim().into());
    front.insert("metadata".into(), metadata.into());
    let yaml =
        serde_yaml_ng::to_string(&front).unwrap_or_else(|_| format!("name: {}\n", proposal.name));

    let mut body = proposal.body.trim().to_string();
    if !proposal.scripts.is_empty() && !body.contains("## Scripts") {
        body.push_str("\n\n## Scripts\n\n");
        for script in &proposal.scripts {
            let what = script.description.trim();
            body.push_str(&format!(
                "- `{}` ({})",
                script.path,
                script.interpreter.trim()
            ));
            if !what.is_empty() {
                body.push_str(&format!(": {what}"));
            }
            body.push('\n');
        }
    }
    format!("---\n{yaml}---\n\n{}\n", body.trim_end())
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = std::fs::metadata(path) {
        let mut perms = metadata.permissions();
        perms.set_mode(perms.mode() | 0o755);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// Receives proposals from the agent. Implemented by the app layer, which
/// surfaces them as review cards.
///
/// Submission does not block: the model proposes a skill and keeps working,
/// and the user reviews when they get to it. Blocking the loop on a human
/// decision that is not required for the current task would be the wrong
/// trade.
#[async_trait]
pub trait ProposalSink: Send + Sync {
    async fn submit(&self, proposal: SkillProposal);
}

/// Collects proposals in memory. Used in tests and headless runs.
#[derive(Default)]
pub struct CollectingSink {
    pub proposals: tokio::sync::Mutex<Vec<SkillProposal>>,
}

#[async_trait]
impl ProposalSink for CollectingSink {
    async fn submit(&self, proposal: SkillProposal) {
        self.proposals.lock().await.push(proposal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::SkillSource;
    use crate::skill::{SkillOrigin, SkillTier};
    use tempfile::TempDir;

    fn proposal() -> SkillProposal {
        SkillProposal::new(
            "release-notes",
            "Assemble release notes from merged pull requests",
            "Preparing release notes for a tagged version",
            "1. List merged PRs since the last tag.\n2. Group them by label.\n3. Write the summary.",
        )
    }

    fn catalog_with(entries: &[(&str, &str)]) -> (SkillCatalog, TempDir) {
        let dir = TempDir::new().unwrap();
        for (name, when) in entries {
            let skill_dir = dir.path().join(name);
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join(SKILL_FILE),
                format!("---\nname: {name}\ndescription: d\nwhen_to_use: {when}\n---\nbody here"),
            )
            .unwrap();
        }
        let (catalog, _) = SkillCatalog::discover(&[SkillSource {
            tier: SkillTier::User,
            origin: SkillOrigin::Taurus,
            dir: dir.path().to_path_buf(),
            plugin: None,
        }]);
        (catalog, dir)
    }

    #[test]
    fn accepts_a_well_formed_proposal() {
        let (catalog, _d) = catalog_with(&[]);
        assert!(validate_proposal(&proposal(), &catalog).is_ok());
    }

    #[test]
    fn rejects_a_bad_name() {
        let (catalog, _d) = catalog_with(&[]);
        let mut p = proposal();
        p.name = "Release Notes".into();
        assert!(validate_proposal(&p, &catalog).is_err());
    }

    #[test]
    fn rejects_a_stub_procedure() {
        let (catalog, _d) = catalog_with(&[]);
        let mut p = proposal();
        p.body = "do it".into();
        let err = validate_proposal(&p, &catalog).unwrap_err();
        assert!(err.to_string().contains("too short"));
    }

    #[test]
    fn rejects_a_destructive_script() {
        let (catalog, _d) = catalog_with(&[]);
        let mut p = proposal();
        p.scripts.push(ProposedScript {
            path: "scripts/clean.sh".into(),
            interpreter: "bash".into(),
            description: "tidy up".into(),
            content: "#!/bin/bash\nrm -rf / --no-preserve-root\n".into(),
        });
        let err = validate_proposal(&p, &catalog).unwrap_err();
        assert!(err.to_string().contains("recursive delete"));
    }

    #[test]
    fn rejects_a_script_with_no_source() {
        let (catalog, _d) = catalog_with(&[]);
        let mut p = proposal();
        p.scripts.push(ProposedScript {
            path: "scripts/run.py".into(),
            interpreter: "python3".into(),
            description: String::new(),
            content: "   ".into(),
        });
        assert!(validate_proposal(&p, &catalog).is_err());
    }

    #[test]
    fn rejects_a_near_duplicate_of_an_existing_skill() {
        let (catalog, _d) = catalog_with(&[(
            "changelog-builder",
            "Preparing release notes for a tagged version",
        )]);
        let err = validate_proposal(&proposal(), &catalog).unwrap_err();
        assert!(err.to_string().contains("changelog-builder"));
    }

    #[test]
    fn allows_a_genuinely_different_skill_alongside_existing_ones() {
        let (catalog, _d) =
            catalog_with(&[("pdf-extract", "Reading tables out of a PDF document")]);
        assert!(validate_proposal(&proposal(), &catalog).is_ok());
    }

    #[test]
    fn saving_produces_a_skill_the_catalog_can_load_back() {
        let (catalog, _d) = catalog_with(&[]);
        let mut p = proposal();
        p.scripts.push(ProposedScript {
            path: "scripts/collect.sh".into(),
            interpreter: "bash".into(),
            description: "collect PRs".into(),
            content: "#!/bin/bash\necho collecting\n".into(),
        });
        validate_proposal(&p, &catalog).unwrap();

        let root = TempDir::new().unwrap();
        let dir = save(&p, root.path()).unwrap();
        assert!(dir.join(SKILL_FILE).exists());
        assert!(dir.join("scripts/collect.sh").exists());

        // The round trip is the real assertion: a proposal that saves but does
        // not load back is worse than one that is rejected.
        let (reloaded, problems) = SkillCatalog::discover(&[SkillSource {
            tier: SkillTier::Project,
            origin: SkillOrigin::Taurus,
            dir: root.path().to_path_buf(),
            plugin: None,
        }]);
        assert!(problems.is_empty(), "{problems:?}");
        let skill = reloaded
            .get("release-notes")
            .expect("skill did not load back");
        assert_eq!(
            skill.frontmatter.when_to_use.as_deref(),
            Some(p.when_to_use.as_str())
        );
        assert_eq!(skill.frontmatter.scripts.len(), 1);
        assert!(skill.body.contains("List merged PRs"));
        assert!(
            skill.warnings.is_empty(),
            "a skill Taurus writes must not need leniency to load: {:?}",
            skill.warnings
        );
    }

    #[test]
    fn what_gets_written_is_a_file_other_clients_can_read() {
        let (catalog, _d) = catalog_with(&[]);
        let p = proposal();
        validate_proposal(&p, &catalog).unwrap();

        let root = TempDir::new().unwrap();
        let dir = save(&p, root.path()).unwrap();
        let text = std::fs::read_to_string(dir.join(SKILL_FILE)).unwrap();

        // The two fields the specification requires, and none of the empty
        // ones: a frontmatter full of `null` and `[]` is a worse file to hand
        // somebody than a short one.
        assert!(text.contains("name: release-notes"));
        assert!(text.contains("description:"));
        for empty in ["compatibility:", "allowed_tools:", "scripts:"] {
            assert!(!text.contains(empty), "{empty} should be omitted:\n{text}");
        }
    }

    /// The keys `skills-ref validate` accepts, and it refuses any other.
    const SPEC_KEYS: &[&str] = &[
        "name",
        "description",
        "license",
        "allowed-tools",
        "metadata",
        "compatibility",
    ];

    fn frontmatter_keys(text: &str) -> Vec<String> {
        let yaml = text
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .map(|(yaml, _)| yaml)
            .expect("frontmatter");
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(yaml).unwrap();
        value
            .as_mapping()
            .unwrap()
            .keys()
            .map(|k| k.as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn what_gets_written_passes_the_specifications_validator() {
        let mut p = proposal();
        p.scripts.push(ProposedScript {
            path: "scripts/collect.sh".into(),
            interpreter: "bash".into(),
            description: "collect PRs".into(),
            content: "#!/bin/bash\necho collecting\n".into(),
        });
        let text = skill_md(&p);
        for key in frontmatter_keys(&text) {
            assert!(
                SPEC_KEYS.contains(&key.as_str()),
                "{key} isn't a spec key:\n{text}"
            );
        }
        // The trigger line travels under `metadata`, and reads back as one.
        let parsed = crate::skill::parse_skill_md(&text, Path::new("SKILL.md")).unwrap();
        assert_eq!(
            parsed.frontmatter.when_to_use.as_deref(),
            Some(p.when_to_use.as_str())
        );
        // A script the format can't declare is described where every client
        // reads it, by the path it's saved at.
        assert!(
            text.contains("- `scripts/collect.sh` (bash): collect PRs"),
            "{text}"
        );
    }

    #[test]
    fn a_bare_script_name_is_put_in_scripts() {
        assert_eq!(place_script("extract.py"), "scripts/extract.py");
        assert_eq!(place_script("./extract.py"), "scripts/extract.py");
        assert_eq!(place_script("scripts/extract.py"), "scripts/extract.py");
        assert_eq!(place_script("tools/extract.py"), "tools/extract.py");
    }

    fn with_script(path: &str, interpreter: &str) -> SkillProposal {
        let mut p = proposal();
        p.scripts.push(ProposedScript {
            path: path.into(),
            interpreter: interpreter.into(),
            description: String::new(),
            content: "echo hi\n".into(),
        });
        p
    }

    #[test]
    fn a_script_has_to_be_in_scripts_and_inside_the_skill() {
        let (catalog, _d) = catalog_with(&[]);
        for (path, says) in [
            ("tools/run.sh", "has to be in scripts/"),
            ("scripts/../../run.sh", "inside the skill"),
            ("/tmp/run.sh", "inside the skill"),
            ("scripts/", "has to be in scripts/"),
            ("scripts\\run.sh", "forward slashes"),
        ] {
            let err = validate_proposal(&with_script(path, "bash"), &catalog).unwrap_err();
            assert!(err.to_string().contains(says), "{path}: {err}");
        }
        assert!(validate_proposal(&with_script("scripts/sub/run.sh", "bash"), &catalog).is_ok());
    }

    #[test]
    fn a_scripts_extension_has_to_say_what_runs_it() {
        let (catalog, _d) = catalog_with(&[]);
        let err = validate_proposal(&with_script("scripts/run.py", "node"), &catalog).unwrap_err();
        assert!(
            err.to_string().contains("runs with python3, not node"),
            "{err}"
        );
        let err = validate_proposal(&with_script("scripts/run", "bash"), &catalog).unwrap_err();
        assert!(err.to_string().contains("extension"), "{err}");
        // `sh` and `bash` both run a `.sh`.
        assert!(validate_proposal(&with_script("scripts/run.sh", "sh"), &catalog).is_ok());
    }

    #[test]
    fn a_reference_or_asset_is_bundled_where_the_format_says() {
        let (catalog, _d) = catalog_with(&[]);
        let mut p = proposal();
        p.files.push(ProposedFile {
            path: "notes/API.md".into(),
            content: "The API.".into(),
        });
        let err = validate_proposal(&p, &catalog).unwrap_err();
        assert!(err.to_string().contains("references/ or assets/"), "{err}");

        p.files[0].path = "references/API.md".into();
        validate_proposal(&p, &catalog).unwrap();
        let root = TempDir::new().unwrap();
        let dir = save(&p, root.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("references/API.md")).unwrap(),
            "The API."
        );
    }

    fn with_body(body: &str) -> SkillProposal {
        let mut p = proposal();
        p.body = body.into();
        p
    }

    #[test]
    fn a_path_the_procedure_names_has_to_be_in_the_proposal() {
        let p = with_body("1. Run scripts/extract.py on the file.\n2. See references/FORMAT.md.");
        let err = check_bundled(&p, |_| false).unwrap_err();
        assert!(err.to_string().contains("`references/FORMAT.md`"), "{err}");

        let mut p = p;
        p.files.push(ProposedFile {
            path: "references/FORMAT.md".into(),
            content: "x".into(),
        });
        let err = check_bundled(&p, |_| false).unwrap_err();
        assert!(err.to_string().contains("`scripts/extract.py`"), "{err}");
        assert!(err.to_string().contains("Add it to `scripts`"), "{err}");

        p.scripts.push(ProposedScript {
            path: "scripts/extract.py".into(),
            interpreter: "python3".into(),
            description: String::new(),
            content: "print(1)".into(),
        });
        check_bundled(&p, |_| false).unwrap();
    }

    #[test]
    fn a_script_from_the_workspace_has_to_be_bundled() {
        // The report this fixes: a helper written while working, run by the
        // procedure from where it was written, and left behind.
        for body in [
            "1. Run `python3 extract_tables.py report.pdf`.",
            "1. Run `uv run python tools/extract.py`.",
            "1. Run ./collect.sh and read what it prints.",
            "1. Run `bash /tmp/collect.sh`.",
        ] {
            let err = check_bundled(&with_body(body), |_| false).unwrap_err();
            assert!(
                err.to_string().contains("isn't in the skill"),
                "{body}: {err}"
            );
            assert!(err.to_string().contains("as scripts/"), "{body}: {err}");
        }
    }

    #[test]
    fn a_file_the_project_tracks_may_be_referenced_from_a_project_skill() {
        let p = with_body("1. Run `bash ./scripts/release.sh` from the project root.");
        assert!(check_bundled(&p, |_| false).is_err());
        check_bundled(&p, |path| path == "scripts/release.sh").unwrap();
    }

    #[test]
    fn a_project_path_that_only_contains_the_word_is_not_a_reference() {
        let p = with_body("1. Edit src/scripts/config.rs and crates/x/references/y.md by hand.");
        check_bundled(&p, |_| false).unwrap();
    }

    #[test]
    fn a_proposal_that_would_only_load_by_leniency_is_rejected() {
        // Loading from disk warns and carries on for a name in the wrong case.
        // Writing a new one is a choice, and this is the moment to refuse it.
        let (catalog, _d) = catalog_with(&[]);
        let mut p = proposal();
        p.name = "Release_Notes".into();

        let err = validate_proposal(&p, &catalog).unwrap_err();
        assert!(err.to_string().contains("kebab-case"), "{err}");
    }

    #[tokio::test]
    async fn the_collecting_sink_records_submissions() {
        let sink = CollectingSink::default();
        sink.submit(proposal()).await;
        assert_eq!(sink.proposals.lock().await.len(), 1);
    }
}

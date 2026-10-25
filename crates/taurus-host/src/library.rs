//! Changing the library from the app: creating, copying and deleting skills
//! and sub-agents, starting a brief, and which files outside the workspace the
//! canvas may open.
//!
//! # The file is still the source of truth
//!
//! Nothing here keeps a second copy of a skill or an agent. A skill made in
//! the editor goes through the writer an approved proposal goes through, and
//! changing one afterwards is opening its file in the canvas. So a skill
//! written in the app and one written in a text editor are the same file, and
//! the freshness check that picks up the second picks up the first.
//!
//! Changing an existing skill is the file and not a form for a reason worth
//! stating. A skill's frontmatter carries keys Taurus doesn't model (`license`,
//! `metadata` beyond the trigger line, whatever another client added), and a
//! form that rebuilt the file from the fields it knows would delete them on
//! every save. The text is everything; a form is a subset.
//!
//! # What may be deleted
//!
//! Only what Taurus wrote: a skill folder in a `.taurus/skills`, an agent file
//! in a `.taurus/agents`, and either inside a plugin you made yourself. A skill
//! in `.claude/skills` or `.github/skills` belongs to another tool as much as
//! to this one, and a plugin fetched from somewhere is replaced on its next
//! update. Those are copied instead, into a folder of your own that shadows the
//! original, which is what editing a built-in agent has always done.
//!
//! # The canvas outside the workspace
//!
//! The canvas opens files under the workspace, through the same guard every
//! tool goes through. The library's personal tier lives in the home directory,
//! so a person who wants to change a skill they use everywhere needs the canvas
//! to reach `~/.taurus/skills` too. [`editable_roots`] is that list, and it is
//! deliberately the list of places a library drawer shows you something from:
//! the canvas can open what you were just looking at, and nothing else. The
//! tools never see it. The model's reach is unchanged, because a person typing
//! in an editor they opened is not the model writing a file — the argument
//! [`crate::document`] makes for why saving asks no permission.

use std::path::{Path, PathBuf};

use taurus_agents::{AgentDefinition, AgentTier};
use taurus_skills::proposal::{self, SkillProposal};
use taurus_skills::{Skill, SkillOrigin, SkillTier};

use crate::config::{self, Scope};
use crate::instructions::{self, InstructionsTier};
use crate::plugins;

/// Where the canvas may open a file outside the workspace.
///
/// Every personal-tier skill folder and agent folder Taurus reads, every
/// personal instructions file it looks for, and the folder of every personal
/// plugin you made yourself. The project tier is not listed: it is inside the
/// workspace, which the canvas can already open.
///
/// A plugin fetched with `taurus plugin add` is left out on purpose. Its files
/// are replaced on the next update, and an editor that let you change them
/// would be an editor that loses your work later, at a moment you won't connect
/// with this one.
pub fn editable_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for source in config::skill_sources(None) {
        if source.plugin.is_none() && source.tier == SkillTier::User {
            roots.push(source.dir);
        }
    }
    for source in config::agent_sources(None) {
        if source.plugin.is_none() && source.tier == AgentTier::User {
            roots.push(source.dir);
        }
    }
    for source in instructions::sources(None) {
        if source.tier == InstructionsTier::User {
            roots.push(source.path);
        }
    }
    // The folder Copilot's scoped files are listed from, so a file added there
    // after the list above was made is reachable too.
    roots.extend(instructions::scoped_dirs(None));
    for plugin in plugins::installed(None) {
        if plugin.scope() == Scope::Global && plugin.summary.source.is_none() {
            roots.push(plugin.root().to_path_buf());
        }
    }
    roots
}

/// Whether a plugin's files are replaced when it's updated.
///
/// True for a plugin installed from somewhere, which `taurus plugin update`
/// fetches again. False for one you made, which nothing overwrites, so its
/// skills and agents can be changed and deleted like your own.
pub fn managed_plugins(workspace: Option<&Path>) -> Vec<String> {
    plugins::installed(workspace)
        .into_iter()
        .filter(|plugin| plugin.summary.source.is_some())
        .map(|plugin| plugin.name().to_string())
        .collect()
}

/// A personal plugin's folder for skills or agents, for the editors' third
/// save target. Refused for a plugin that is fetched from somewhere, for the
/// reason [`editable_roots`] leaves those out.
pub fn plugin_part_dir(
    name: &str,
    part: PluginPart,
    workspace: Option<&Path>,
) -> Result<PathBuf, String> {
    let installed = plugins::installed(workspace);
    // A project plugin replaces a personal one of the same name, so it is the
    // one a save should land in.
    let plugin = installed
        .iter()
        .filter(|plugin| plugin.name() == name)
        .max_by_key(|plugin| plugin.scope() == Scope::Workspace)
        .ok_or_else(|| format!("there's no plugin named \"{name}\" installed"))?;
    if let Some(source) = &plugin.summary.source {
        return Err(format!(
            "the plugin \"{name}\" was added from {}, and its files are replaced the next time \
             it's updated. Save this to your own skills or agents instead, or make a plugin of \
             your own in the Plugins panel.",
            source.from
        ));
    }
    let dirs = match part {
        PluginPart::Skills => plugin.skill_dirs(),
        PluginPart::Agents => plugin.agent_dirs(),
    };
    Ok(dirs
        .first()
        .cloned()
        .unwrap_or_else(|| plugin.root().join(part.folder())))
}

/// Which of a plugin's folders [`plugin_part_dir`] is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginPart {
    Skills,
    Agents,
}

impl PluginPart {
    fn folder(self) -> &'static str {
        match self {
            PluginPart::Skills => "skills",
            PluginPart::Agents => "agents",
        }
    }
}

/// Writes a skill made in the editor into `root`, refusing a name already
/// there.
///
/// An approved proposal may replace a skill of the same name, and its card says
/// so before anyone approves it. The editor is for making a new one, and a form
/// that quietly overwrote a folder of scripts because its name was reused is a
/// form that deletes work. To change a skill, open it.
pub fn create_skill(draft: &SkillProposal, root: &Path) -> Result<PathBuf, String> {
    let dir = root.join(&draft.name);
    if dir.exists() {
        return Err(format!(
            "a skill named \"{}\" is already at {}. Open that one to change it, or choose \
             another name.",
            draft.name,
            dir.display()
        ));
    }
    proposal::save(draft, root).map_err(|e| format!("could not save the skill: {e}"))
}

/// Deletes a skill Taurus wrote, and answers with the folder that went.
///
/// `owned` is every folder a skill may be deleted from: the two `.taurus/skills`
/// and the skill folders of personal plugins. The skill's own folder has to sit
/// directly inside one of them, which is checked against the disk rather than
/// trusted from the catalog — this is a recursive delete, and it is the last
/// place a wrong path could be caught.
pub fn delete_skill(skill: &Skill, owned: &[PathBuf]) -> Result<PathBuf, String> {
    if skill.origin == SkillOrigin::PluginCommand {
        return Err(format!(
            "\"{}\" is a command file in its plugin, not a skill folder. Delete the file in \
             the plugin, or switch the plugin off.",
            skill.name()
        ));
    }
    let inside = |dir: &Path| {
        let (Ok(dir), Some(parent)) = (dir.canonicalize(), skill.dir.parent()) else {
            return false;
        };
        parent.canonicalize().is_ok_and(|parent| parent == dir)
    };
    if !owned.iter().any(|dir| inside(dir)) {
        return Err(match (&skill.plugin, skill.origin) {
            (Some(plugin), _) => format!(
                "\"{}\" comes with the plugin \"{plugin}\", which is replaced when it's \
                 updated. Switch the plugin off, or copy the skill to your own and change that.",
                skill.name()
            ),
            (None, origin) => format!(
                "\"{}\" is in {}, which {} reads too. Taurus deletes only skills it wrote; \
                 remove the folder there if you want it gone.",
                skill.name(),
                skill.dir.display(),
                reader(origin)
            ),
        });
    }
    std::fs::remove_dir_all(&skill.dir)
        .map_err(|e| format!("could not delete {}: {e}", skill.dir.display()))?;
    Ok(skill.dir.clone())
}

/// Which other tool reads an origin's folder, for a refusal to name.
fn reader(origin: SkillOrigin) -> &'static str {
    match origin {
        SkillOrigin::Claude => "Claude Code",
        SkillOrigin::Copilot => "GitHub Copilot",
        SkillOrigin::Agents => "every client that follows the Agent Skills convention",
        SkillOrigin::Taurus | SkillOrigin::Plugin | SkillOrigin::PluginCommand => "another tool",
    }
}

/// Copies a skill into `root`, a `.taurus/skills` of the same tier, and
/// answers with the new folder.
///
/// For a skill that can't be changed where it is: one from a plugin fetched
/// from somewhere, or one in another tool's folder that you'd rather not touch.
/// Within a tier the `.taurus` folder is read last, so a copy of a borrowed
/// skill shadows it. A plugin's skill is named `plugin:skill` and the copy
/// `skill`, so both stay, and the copy is the one to change.
///
/// A plugin's command file is one Markdown file without a folder, and may have
/// no frontmatter at all, so a straight copy wouldn't load as a skill. It is
/// written out as one instead, from what was loaded.
pub fn copy_skill(skill: &Skill, root: &Path) -> Result<PathBuf, String> {
    let folder = match skill.origin {
        SkillOrigin::PluginCommand => skill
            .name()
            .rsplit(':')
            .next()
            .unwrap_or(skill.name())
            .to_string(),
        _ => skill
            .dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| format!("{} has no folder name", skill.dir.display()))?,
    };
    let target = root.join(&folder);
    if target.exists() {
        return Err(format!(
            "there's already a skill folder at {}. Open that one, or delete it first.",
            target.display()
        ));
    }
    if same_place(&skill.dir, &target) {
        return Err(format!("\"{}\" is already yours to change.", skill.name()));
    }

    if skill.origin == SkillOrigin::PluginCommand {
        let draft = SkillProposal::new(
            folder.clone(),
            skill.frontmatter.description.clone(),
            skill.trigger(),
            skill.body.clone(),
        );
        std::fs::create_dir_all(&target)
            .map_err(|e| format!("could not create {}: {e}", target.display()))?;
        std::fs::write(
            target.join(taurus_skills::catalog::SKILL_FILE),
            proposal::skill_md(&draft),
        )
        .map_err(|e| format!("could not write {}: {e}", target.display()))?;
        return Ok(target);
    }

    plugins::copy_dir(&skill.dir, &target).inspect_err(|_| {
        // Half a copy is a skill that loads with files missing. Nothing was
        // there before, so taking it back out loses nothing.
        let _ = std::fs::remove_dir_all(&target);
    })?;
    Ok(target)
}

fn same_place(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Where an edit to `agent` is written, and whether that means writing a copy.
///
/// A file Taurus owns is changed where it is. A built-in has no file, a file in
/// another tool's folder isn't Taurus's to rewrite (its writer knows only the
/// keys Taurus models), and a fetched plugin's file is replaced on update — so
/// those three fork into a `.taurus/agents` of the *same tier*. A user-tier
/// copy of a project agent would sit underneath the file it meant to override
/// and change nothing; within a tier the `.taurus` folder is read last, so a
/// copy beside a borrowed file wins.
///
/// `managed` says whether the agent's plugin, if it has one, is fetched from
/// somewhere. One you made is yours, and its file is changed in place.
pub fn agent_edit_target(
    agent: &AgentDefinition,
    workspace: &Path,
    managed: bool,
) -> (PathBuf, bool) {
    let in_place = match (&agent.path, &agent.plugin) {
        (Some(path), Some(_)) if !managed => Some(path.clone()),
        (Some(path), None) if !agent.borrowed => Some(path.clone()),
        _ => None,
    };
    if let Some(path) = in_place {
        return (path, false);
    }
    let file = format!("{}.md", own_name(agent));
    let path = match agent.tier {
        AgentTier::Project => config::workspace_agents_dir(workspace).join(file),
        AgentTier::User | AgentTier::Builtin => config::user_agents_dir().join(file),
    };
    (path, true)
}

/// The name a copy of an agent goes by: a plugin's `ops:reviewer` becomes
/// `reviewer`, because a colon can't be in a file name and the copy is yours.
pub fn own_name(agent: &AgentDefinition) -> &str {
    let name = agent.name();
    match &agent.plugin {
        Some(_) => name.rsplit(':').next().unwrap_or(name),
        None => name,
    }
}

/// Writes the copy [`agent_edit_target`] decided on, unless one is there.
///
/// Answers with the file to open. When a copy is already on disk it is the one
/// to open, and writing over it would throw away the edit the person made
/// last time — which is the case when a project copy exists but the catalog
/// still shows a user-tier one above it.
pub fn fork_agent(agent: &AgentDefinition, target: &Path) -> Result<PathBuf, String> {
    if target.exists() {
        return Ok(target.to_path_buf());
    }
    let mut copy = agent.clone();
    copy.frontmatter.name = own_name(agent).to_string();
    copy.write_to(target)
        .map_err(|e| format!("could not write {}: {e}", target.display()))?;
    Ok(target.to_path_buf())
}

/// Deletes an agent file Taurus owns, and answers with the file that went.
///
/// Same rule as [`delete_skill`]: only from a folder in `owned`, checked on
/// disk. A built-in has no file and can't be deleted; a copy that shadowed one
/// can, and the built-in is back on the next rescan.
pub fn delete_agent(agent: &AgentDefinition, owned: &[PathBuf]) -> Result<PathBuf, String> {
    let Some(path) = &agent.path else {
        return Err(format!(
            "\"{}\" is built in, so there's no file to delete. Write a copy and change that \
             instead.",
            agent.name()
        ));
    };
    let inside = |dir: &Path| {
        let (Ok(dir), Some(parent)) = (dir.canonicalize(), path.parent()) else {
            return false;
        };
        parent.canonicalize().is_ok_and(|parent| parent == dir)
    };
    if !owned.iter().any(|dir| inside(dir)) {
        return Err(match &agent.plugin {
            Some(plugin) => format!(
                "\"{}\" comes with the plugin \"{plugin}\", which is replaced when it's \
                 updated. Switch the plugin off instead.",
                agent.name()
            ),
            None => format!(
                "\"{}\" is {}, which another tool reads too. Taurus deletes only agents it \
                 wrote; remove the file there if you want it gone.",
                agent.name(),
                path.display()
            ),
        });
    }
    std::fs::remove_file(path).map_err(|e| format!("could not delete {}: {e}", path.display()))?;
    Ok(path.clone())
}

/// The `TAURUS.md` for a tier, created empty if it isn't there, so the canvas
/// has a file to open. Answers with its path.
///
/// Empty rather than a template. Every byte of a brief is read into every
/// request of every turn, and a starter paragraph nobody deleted would be paid
/// for indefinitely. An empty file reads as no brief at all, which is true.
pub fn start_brief(tier: InstructionsTier, workspace: &Path) -> Result<PathBuf, String> {
    let path = match tier {
        InstructionsTier::User => config::home_dir().join("TAURUS.md"),
        InstructionsTier::Project => config::workspace_dir(workspace).join("TAURUS.md"),
    };
    if path.exists() {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, "").map_err(|e| format!("could not create {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::isolated_home;
    use taurus_agents::catalog::{AgentCatalog, AgentSource};
    use taurus_skills::catalog::{SkillCatalog, SkillSource};
    use tempfile::TempDir;

    const BODY: &str = "Run the release script, then tag the commit it printed and push the tag.";

    fn draft(name: &str) -> SkillProposal {
        SkillProposal::new(name, "Cuts a release", "When a release is asked for", BODY)
    }

    fn write_skill(dir: &Path, name: &str) {
        let folder = dir.join(name);
        std::fs::create_dir_all(folder.join("scripts")).unwrap();
        std::fs::write(
            folder.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Does {name}\nlicense: MIT\n---\n{BODY}\n"),
        )
        .unwrap();
        std::fs::write(folder.join("scripts/run.sh"), "echo hi\n").unwrap();
    }

    fn load_skill(dir: &Path, origin: SkillOrigin, plugin: Option<&str>) -> Skill {
        let (catalog, problems) = SkillCatalog::discover(&[SkillSource {
            tier: SkillTier::User,
            origin,
            dir: dir.to_path_buf(),
            plugin: plugin.map(String::from),
        }]);
        assert!(problems.is_empty(), "{problems:?}");
        let name = catalog.names().next().expect("one skill").to_string();
        catalog.get(&name).unwrap().clone()
    }

    #[test]
    fn a_new_skill_is_written_where_it_was_asked_for() {
        let root = TempDir::new().unwrap();
        let dir = create_skill(&draft("cut-release"), root.path()).unwrap();
        let text = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
        assert!(text.contains("name: cut-release"), "{text}");
        assert!(text.contains(BODY));
    }

    #[test]
    fn a_new_skill_never_replaces_one_already_there() {
        let root = TempDir::new().unwrap();
        write_skill(root.path(), "cut-release");
        let err = create_skill(&draft("cut-release"), root.path()).unwrap_err();
        assert!(err.contains("already at"), "{err}");
        // And the scripts of the one that was there are untouched.
        assert!(root.path().join("cut-release/scripts/run.sh").exists());
    }

    #[test]
    fn a_skill_taurus_wrote_is_deleted_with_its_folder() {
        let root = TempDir::new().unwrap();
        write_skill(root.path(), "cut-release");
        let skill = load_skill(root.path(), SkillOrigin::Taurus, None);
        delete_skill(&skill, &[root.path().to_path_buf()]).unwrap();
        assert!(!root.path().join("cut-release").exists());
    }

    #[test]
    fn a_borrowed_skill_is_not_deleted_and_the_refusal_says_whose_it_is() {
        let claude = TempDir::new().unwrap();
        let ours = TempDir::new().unwrap();
        write_skill(claude.path(), "cut-release");
        let skill = load_skill(claude.path(), SkillOrigin::Claude, None);
        let err = delete_skill(&skill, &[ours.path().to_path_buf()]).unwrap_err();
        assert!(err.contains("Claude Code"), "{err}");
        assert!(claude.path().join("cut-release/SKILL.md").exists());
    }

    #[test]
    fn a_fetched_plugins_skill_is_not_deleted() {
        let plugin = TempDir::new().unwrap();
        write_skill(plugin.path(), "deploy");
        let skill = load_skill(plugin.path(), SkillOrigin::Plugin, Some("ops"));
        let err = delete_skill(&skill, &[]).unwrap_err();
        assert!(err.contains("plugin \"ops\""), "{err}");
        assert!(plugin.path().join("deploy").exists());
    }

    #[test]
    fn a_copy_keeps_every_file_and_every_key() {
        let claude = TempDir::new().unwrap();
        let ours = TempDir::new().unwrap();
        write_skill(claude.path(), "cut-release");
        let skill = load_skill(claude.path(), SkillOrigin::Claude, None);
        let copy = copy_skill(&skill, ours.path()).unwrap();
        assert_eq!(copy, ours.path().join("cut-release"));
        assert!(copy.join("scripts/run.sh").exists());
        // A key Taurus doesn't model survives, because the folder is copied,
        // not rebuilt.
        let text = std::fs::read_to_string(copy.join("SKILL.md")).unwrap();
        assert!(text.contains("license: MIT"), "{text}");
    }

    #[test]
    fn a_plugins_skill_is_copied_under_its_own_name() {
        let plugin = TempDir::new().unwrap();
        let ours = TempDir::new().unwrap();
        write_skill(plugin.path(), "deploy");
        let skill = load_skill(plugin.path(), SkillOrigin::Plugin, Some("ops"));
        assert_eq!(skill.name(), "ops:deploy");
        let copy = copy_skill(&skill, ours.path()).unwrap();
        let mine = load_skill(ours.path(), SkillOrigin::Taurus, None);
        assert_eq!(copy, ours.path().join("deploy"));
        assert_eq!(mine.name(), "deploy");
    }

    #[test]
    fn a_copy_never_lands_on_a_folder_already_there() {
        let claude = TempDir::new().unwrap();
        let ours = TempDir::new().unwrap();
        write_skill(claude.path(), "cut-release");
        write_skill(ours.path(), "cut-release");
        let skill = load_skill(claude.path(), SkillOrigin::Claude, None);
        let err = copy_skill(&skill, ours.path()).unwrap_err();
        assert!(err.contains("already a skill folder"), "{err}");
    }

    #[test]
    fn a_plugin_command_is_copied_as_a_skill_that_loads() {
        let commands = TempDir::new().unwrap();
        let ours = TempDir::new().unwrap();
        std::fs::write(
            commands.path().join("review.md"),
            "Review the diff for $ARGUMENTS and report each bug with its line.\n",
        )
        .unwrap();
        let skill = load_skill(commands.path(), SkillOrigin::PluginCommand, Some("ops"));
        let copy = copy_skill(&skill, ours.path()).unwrap();
        let mine = load_skill(ours.path(), SkillOrigin::Taurus, None);
        assert_eq!(copy, ours.path().join("review"));
        assert_eq!(mine.name(), "review");
        assert!(mine.body.contains("$ARGUMENTS"));
    }

    fn load_agent(
        dir: &Path,
        tier: AgentTier,
        borrowed: bool,
        plugin: Option<&str>,
    ) -> AgentDefinition {
        let (catalog, problems) = AgentCatalog::discover(&[AgentSource {
            tier,
            dir: dir.to_path_buf(),
            borrowed,
            plugin: plugin.map(String::from),
        }]);
        assert!(problems.is_empty(), "{problems:?}");
        let found = catalog
            .iter()
            .find(|agent| agent.path.is_some())
            .expect("one agent from disk")
            .clone();
        found
    }

    fn write_agent(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join(format!("{name}.md")),
            format!(
                "---\nname: {name}\ndescription: Reviews things\nmodel: small\n---\nYou review a \
                 diff for bugs and report each with its line.\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn an_agent_taurus_owns_is_edited_in_place() {
        let dir = TempDir::new().unwrap();
        write_agent(dir.path(), "reviewer");
        let agent = load_agent(dir.path(), AgentTier::Project, false, None);
        let (path, forks) = agent_edit_target(&agent, dir.path(), false);
        assert!(!forks);
        assert_eq!(path, dir.path().join("reviewer.md"));
    }

    #[test]
    fn a_borrowed_project_agent_forks_into_the_project() {
        let _home = isolated_home();
        let workspace = TempDir::new().unwrap();
        let github = workspace.path().join(".github/agents");
        write_agent(&github, "reviewer");
        let agent = load_agent(&github, AgentTier::Project, true, None);
        let (path, forks) = agent_edit_target(&agent, workspace.path(), false);
        assert!(forks);
        assert_eq!(
            path,
            config::workspace_agents_dir(workspace.path()).join("reviewer.md")
        );
        let written = fork_agent(&agent, &path).unwrap();
        // The copy keeps the key a proposal would have dropped.
        let text = std::fs::read_to_string(written).unwrap();
        assert!(text.contains("model: small"), "{text}");
    }

    #[test]
    fn a_fork_already_on_disk_is_opened_not_overwritten() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source");
        write_agent(&source, "reviewer");
        let agent = load_agent(&source, AgentTier::User, true, None);
        let target = dir.path().join("mine/reviewer.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "my edit").unwrap();
        fork_agent(&agent, &target).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "my edit");
    }

    #[test]
    fn a_fetched_plugins_agent_forks_under_its_own_name() {
        let _home = isolated_home();
        let plugin = TempDir::new().unwrap();
        write_agent(plugin.path(), "reviewer");
        let agent = load_agent(plugin.path(), AgentTier::User, true, Some("ops"));
        assert_eq!(agent.name(), "ops:reviewer");
        let (path, forks) = agent_edit_target(&agent, plugin.path(), true);
        assert!(forks);
        assert_eq!(path, config::user_agents_dir().join("reviewer.md"));
        fork_agent(&agent, &path).unwrap();
        let mine = load_agent(&config::user_agents_dir(), AgentTier::User, false, None);
        assert_eq!(mine.name(), "reviewer");
    }

    #[test]
    fn a_personal_plugins_agent_is_edited_in_place() {
        let plugin = TempDir::new().unwrap();
        write_agent(plugin.path(), "reviewer");
        let agent = load_agent(plugin.path(), AgentTier::User, true, Some("mine"));
        let (path, forks) = agent_edit_target(&agent, plugin.path(), false);
        assert!(!forks);
        assert_eq!(path, plugin.path().join("reviewer.md"));
    }

    #[test]
    fn an_owned_agent_is_deleted_and_a_borrowed_one_is_not() {
        let ours = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        write_agent(ours.path(), "reviewer");
        write_agent(claude.path(), "checker");
        let owned = [ours.path().to_path_buf()];

        let mine = load_agent(ours.path(), AgentTier::User, false, None);
        delete_agent(&mine, &owned).unwrap();
        assert!(!ours.path().join("reviewer.md").exists());

        let theirs = load_agent(claude.path(), AgentTier::User, true, None);
        let err = delete_agent(&theirs, &owned).unwrap_err();
        assert!(err.contains("another tool"), "{err}");
        assert!(claude.path().join("checker.md").exists());
    }

    #[test]
    fn a_built_in_agent_has_no_file_to_delete() {
        let (catalog, _) = AgentCatalog::discover(&[]);
        let builtin = catalog.iter().next().expect("a built-in").clone();
        let err = delete_agent(&builtin, &[]).unwrap_err();
        assert!(err.contains("built in"), "{err}");
    }

    #[test]
    fn a_brief_is_started_empty_and_never_overwritten() {
        let _home = isolated_home();
        let workspace = TempDir::new().unwrap();
        let path = start_brief(InstructionsTier::Project, workspace.path()).unwrap();
        assert_eq!(path, workspace.path().join(".taurus/TAURUS.md"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        std::fs::write(&path, "Prefer small commits.").unwrap();
        start_brief(InstructionsTier::Project, workspace.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "Prefer small commits."
        );
    }

    #[test]
    fn the_canvas_reaches_personal_skills_but_not_a_fetched_plugin() {
        let home = isolated_home();
        let roots = editable_roots();
        assert!(roots.contains(&config::user_skills_dir()), "{roots:?}");
        assert!(roots.contains(&config::user_agents_dir()), "{roots:?}");
        assert!(
            roots.contains(&config::home_dir().join("TAURUS.md")),
            "{roots:?}"
        );
        assert!(
            !roots
                .iter()
                .any(|root| root.starts_with(home.path().join("plugins"))),
            "no plugin was made here, so none is listed: {roots:?}"
        );
    }
}

//! Plugins: skills, sub-agents, MCP servers and hooks under one name.
//!
//! A plugin is a folder in `~/.taurus/plugins/` or a project's
//! `.taurus/plugins/`, laid out the way Claude Code lays one out, so a plugin
//! written for Claude works here unchanged where Taurus has the same part:
//!
//! ```text
//! my-plugin/
//!   .claude-plugin/plugin.json   name, version, description, component paths
//!   skills/<skill>/SKILL.md
//!   agents/<agent>.md
//!   .mcp.json                    {"mcpServers": {...}}
//!   hooks/hooks.json             Taurus's hook format
//! ```
//!
//! Nothing here loads a component. It finds plugins and says where their
//! parts are, and each part goes through the loader it would have gone
//! through anyway: a plugin's skill directory is one more skill source, its
//! `.mcp.json` one more MCP layer, its hooks one more hooks layer. So a
//! plugin's skill is checked, shown and refreshed exactly as any skill is, and
//! there is no second path to keep in step.
//!
//! # Names
//!
//! Every part is named under its plugin, as Claude Code names them: skill
//! `deploy` of plugin `ops` is `ops:deploy`, and so is its agent. An MCP
//! server's tools can't carry a colon, so a plugin's server `db` is keyed
//! `plugin_ops_db`, which makes its tools `mcp__plugin_ops_db__query`, the
//! names Claude gives them. Two plugins never collide with each other or with
//! anything of your own, so a plugin can't quietly replace a skill you wrote.
//!
//! # What isn't run
//!
//! Some parts of Claude's format have nothing to run them here: `commands/`,
//! LSP servers, output styles, workflows, themes, monitors, `bin/`, a
//! plugin's own `settings.json`, `userConfig` and dependencies. They're
//! listed by name as unsupported, with why, rather than skipped in silence. A
//! plugin's hooks must be in Taurus's own format; a hooks file in Claude's is
//! named as one, and none of it runs.
//!
//! # Trust and switching off
//!
//! A project's plugins are project config and wait for the workspace to be
//! trusted, like the rest of `.taurus/`. A plugin is on unless `settings.json`
//! says otherwise in either layer (see [`crate::config::Settings::plugins`]) or
//! its manifest says `defaultEnabled: false`. A project plugin with the same
//! name as one of yours replaces it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::config::{self, Scope};

/// Where a scope keeps its plugins, under its config directory.
pub const PLUGINS_DIR: &str = "plugins";

/// The manifest, relative to a plugin's root. Claude Code's path.
pub const MANIFEST: &str = ".claude-plugin/plugin.json";

/// What `taurus plugin add` writes beside a plugin it installed, saying
/// where it came from.
pub const SOURCE_FILE: &str = ".taurus-plugin-source.json";

/// One plugin, as a listing shows it.
#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct PluginSummary {
    pub name: String,
    /// Yours, or the project's.
    pub scope: Scope,
    /// Its folder.
    pub root: String,
    #[ts(optional)]
    pub version: Option<String>,
    #[ts(optional)]
    pub description: Option<String>,
    /// Switched on. A plugin with problems, or one replaced by a project
    /// plugin of the same name, loads nothing even when this is true.
    pub enabled: bool,
    /// A project plugin of the same name replaces this one.
    pub shadowed: bool,
    /// Its skills' folder names, as the plugin has them. Each is loaded as
    /// `plugin:name`.
    pub skills: Vec<String>,
    /// Its agents' file names, the same way.
    pub agents: Vec<String>,
    /// Its MCP servers, by the names the plugin gives them.
    pub mcp_servers: Vec<String>,
    /// Its hooks, by name.
    pub hooks: Vec<String>,
    /// Parts Taurus can't run, each with why.
    pub unsupported: Vec<Unsupported>,
    /// What stops it loading at all. Empty for a plugin that loads.
    pub problems: Vec<String>,
    /// What's wrong but survivable, like a manifest key nobody knows.
    pub warnings: Vec<String>,
    /// Where `taurus plugin add` got it, when it did.
    #[ts(optional)]
    pub source: Option<PluginSource>,
}

/// A part of a plugin that has nothing here to run it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct Unsupported {
    /// What it is, as the plugin has it: `commands/`, `lspServers`.
    pub part: String,
    pub reason: String,
}

/// Where an installed plugin came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PluginSource {
    /// The folder or git URL it was added from.
    pub from: String,
    /// The commit it was cloned at, for one from git. A plugin isn't updated
    /// behind your back: it stays at this commit until it's updated on
    /// purpose.
    #[ts(optional)]
    pub commit: Option<String>,
}

/// A plugin and where its parts are.
#[derive(Debug)]
pub struct Plugin {
    pub summary: PluginSummary,
    root: PathBuf,
    skill_dirs: Vec<PathBuf>,
    agent_dirs: Vec<PathBuf>,
    mcp: taurus_mcp::McpConfig,
    hooks: taurus_hooks::HookConfig,
    hook_files: Vec<PathBuf>,
}

impl Plugin {
    pub fn name(&self) -> &str {
        &self.summary.name
    }

    pub fn scope(&self) -> Scope {
        self.summary.scope
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Loads something: switched on, not replaced, and nothing wrong with it.
    pub fn is_active(&self) -> bool {
        self.summary.enabled && !self.summary.shadowed && self.summary.problems.is_empty()
    }

    /// Directories of skills, each `<skill>/SKILL.md`.
    pub fn skill_dirs(&self) -> &[PathBuf] {
        &self.skill_dirs
    }

    /// Directories of agents, each `<agent>.md`.
    pub fn agent_dirs(&self) -> &[PathBuf] {
        &self.agent_dirs
    }

    /// Its MCP servers, keyed by [`server_key`], with its paths filled in.
    pub fn mcp(&self) -> &taurus_mcp::McpConfig {
        &self.mcp
    }

    /// Its MCP servers as a layer of their own, for merging under yours.
    pub fn into_mcp(self) -> taurus_mcp::McpConfig {
        self.mcp
    }

    /// Its hooks, named `plugin:hook`, with its paths filled in.
    pub fn hooks(&self) -> &taurus_hooks::HookConfig {
        &self.hooks
    }

    /// The hooks files it reads, for noticing when one changes.
    pub fn hook_files(&self) -> &[PathBuf] {
        &self.hook_files
    }
}

/// The key a plugin's MCP server is registered under. Its tools are then
/// `mcp__plugin_<plugin>_<server>__<tool>`, which is what Claude Code names
/// them, and a key with no colon in it is one every provider accepts in a
/// tool name.
///
/// The server's own name is narrowed to the characters a tool name may hold,
/// so a plugin's "google calendar" is keyed `plugin_x_google_calendar`.
pub fn server_key(plugin: &str, server: &str) -> String {
    let server: String = server
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("plugin_{plugin}_{server}")
}

/// Where a scope's plugins are.
pub fn dir(scope: Scope, workspace: Option<&Path>) -> Option<PathBuf> {
    config::scope_dir(scope, workspace).map(|dir| dir.join(PLUGINS_DIR))
}

/// Every plugin in your folder and, when `workspace` is given, the project's,
/// whether or not it loads. Not gated by trust: pass a workspace through
/// [`crate::trust::for_reading`] first to see only what may be read.
pub fn installed(workspace: Option<&Path>) -> Vec<Plugin> {
    let switches = switches(workspace);
    let mut found: Vec<Plugin> = Vec::new();
    for scope in [Scope::Global, Scope::Workspace] {
        let Some(dir) = dir(scope, workspace) else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut roots: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .filter(|path| {
                !path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            })
            .collect();
        roots.sort();
        for root in roots {
            let mut plugin = load(&root, scope, workspace);
            plugin.summary.enabled = switches
                .get(plugin.name())
                .copied()
                .unwrap_or(plugin.summary.enabled);
            found.push(plugin);
        }
    }
    // A project plugin replaces one of yours with the same name.
    let project: Vec<String> = found
        .iter()
        .filter(|p| p.scope() == Scope::Workspace)
        .map(|p| p.name().to_string())
        .collect();
    for plugin in &mut found {
        if plugin.scope() == Scope::Global && project.iter().any(|name| name == plugin.name()) {
            plugin.summary.shadowed = true;
        }
    }
    found
}

/// The plugins that load: [`installed`], less the ones switched off,
/// replaced, or broken. Pass a workspace through the trust gate first.
pub fn active(workspace: Option<&Path>) -> Vec<Plugin> {
    installed(workspace)
        .into_iter()
        .filter(Plugin::is_active)
        .collect()
}

/// Which plugins `settings.json` switches on or off, both layers merged name
/// by name. Read directly rather than through [`config::load_settings`],
/// which gates the project layer: [`installed`] is told which workspace to
/// read, and its caller has already decided whether it may.
fn switches(workspace: Option<&Path>) -> BTreeMap<String, bool> {
    let mut merged = config::read_settings(Scope::Global, None)
        .plugins
        .unwrap_or_default();
    if workspace.is_some() {
        merged.extend(
            config::read_settings(Scope::Workspace, workspace)
                .plugins
                .unwrap_or_default(),
        );
    }
    merged
}

/// The manifest, as much of it as Taurus reads. Everything else is kept in
/// `rest` to be named, as unsupported or as unknown.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    default_enabled: Option<bool>,
    skills: Option<serde_json::Value>,
    agents: Option<serde_json::Value>,
    hooks: Option<serde_json::Value>,
    mcp_servers: Option<serde_json::Value>,
    #[serde(flatten)]
    rest: BTreeMap<String, serde_json::Value>,
}

/// Manifest keys Claude Code documents that Taurus reads nothing from,
/// because they describe the plugin rather than add to it.
const DESCRIPTIVE: &[&str] = &[
    "author",
    "homepage",
    "repository",
    "license",
    "keywords",
    "displayName",
    "$schema",
];

/// Manifest keys Claude Code documents that name a part Taurus can't run.
const UNSUPPORTED_KEYS: &[(&str, &str)] = &[
    (
        "commands",
        "Taurus has no command files; a skill marked user-invocable is its /command",
    ),
    ("lspServers", "Taurus doesn't run language servers"),
    ("outputStyles", "Taurus has no output styles"),
    ("workflows", "Taurus has no workflow scripts"),
    (
        "experimental",
        "Taurus has no themes, monitors or evals from plugins",
    ),
    (
        "userConfig",
        "Taurus doesn't ask for plugin settings, so `${user_config.…}` stays unfilled",
    ),
    (
        "dependencies",
        "Taurus doesn't install other plugins for this one; add them yourself",
    ),
    ("settings", "a plugin can't change Taurus's settings"),
];

/// Folders and files at a plugin's root that name a part Taurus can't run.
const UNSUPPORTED_PATHS: &[(&str, &str)] = &[
    (
        "commands",
        "Taurus has no command files; a skill marked user-invocable is its /command",
    ),
    (".lsp.json", "Taurus doesn't run language servers"),
    ("output-styles", "Taurus has no output styles"),
    ("workflows", "Taurus has no workflow scripts"),
    ("themes", "Taurus has no themes from plugins"),
    ("monitors", "Taurus has no monitors"),
    (
        "bin",
        "Taurus doesn't put a plugin's programs on PATH; its MCP servers and hooks can still name them by path",
    ),
    ("settings.json", "a plugin can't change Taurus's settings"),
    (
        "SKILL.md",
        "a skill at the plugin's root isn't read; put it in skills/<name>/SKILL.md",
    ),
];

/// The events Claude Code's hooks file is keyed by, which is how a hooks file
/// in its format is told apart from one in Taurus's.
const CLAUDE_HOOK_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "Stop",
    "SubagentStop",
    "SessionStart",
    "SessionEnd",
    "Notification",
    "PreCompact",
];

/// Reads one plugin folder. Never fails: what's wrong goes in the summary,
/// so a broken plugin is listed with why rather than missing.
pub fn load(root: &Path, scope: Scope, workspace: Option<&Path>) -> Plugin {
    let folder = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut problems = Vec::new();
    let mut warnings = Vec::new();
    let mut unsupported = Vec::new();

    let manifest_path = root.join(MANIFEST);
    let manifest = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => match serde_json::from_str::<Manifest>(&text) {
            Ok(manifest) => {
                if manifest.name.is_none() {
                    problems.push(format!(
                        "{} has no \"name\", which is the one key a manifest must have",
                        manifest_path.display()
                    ));
                }
                manifest
            }
            Err(e) => {
                problems.push(format!("{} doesn't parse: {e}", manifest_path.display()));
                Manifest::default()
            }
        },
        // Optional, as in Claude Code: a folder of parts is named after itself.
        Err(_) => Manifest::default(),
    };
    let name = manifest.name.clone().unwrap_or_else(|| folder.clone());
    if let Err(e) = check_name(&name) {
        problems.push(e);
    }

    for (key, value) in &manifest.rest {
        if DESCRIPTIVE.contains(&key.as_str()) {
            continue;
        }
        if let Some((_, reason)) = UNSUPPORTED_KEYS.iter().find(|(k, _)| k == key) {
            if !value.is_null() {
                unsupported.push(Unsupported {
                    part: key.clone(),
                    reason: (*reason).to_string(),
                });
            }
        } else {
            warnings.push(format!(
                "the manifest's \"{key}\" isn't a key Claude Code or Taurus knows, so it's ignored"
            ));
        }
    }
    for (path, reason) in UNSUPPORTED_PATHS {
        let full = root.join(path);
        if full.exists() && !unsupported.iter().any(|u| u.part == *path) {
            let shown = if full.is_dir() {
                format!("{path}/")
            } else {
                (*path).to_string()
            };
            // The manifest key and the folder are the same part.
            if !unsupported.iter().any(|u| shown.starts_with(&u.part)) {
                unsupported.push(Unsupported {
                    part: shown,
                    reason: (*reason).to_string(),
                });
            }
        }
    }

    // Skills: the default folder, plus any the manifest adds.
    let mut skill_dirs = Vec::new();
    if root.join("skills").is_dir() {
        skill_dirs.push(root.join("skills"));
    }
    for path in paths(root, manifest.skills.as_ref(), "skills", &mut problems) {
        if !skill_dirs.contains(&path) {
            skill_dirs.push(path);
        }
    }

    // Agents: the manifest's list replaces the default, as in Claude Code.
    let agent_dirs = match &manifest.agents {
        Some(value) => paths(root, Some(value), "agents", &mut problems)
            .into_iter()
            .map(|path| {
                // A single agent file names its folder.
                if path.is_file() {
                    path.parent().map(Path::to_path_buf).unwrap_or(path)
                } else {
                    path
                }
            })
            .collect(),
        None if root.join("agents").is_dir() => vec![root.join("agents")],
        None => Vec::new(),
    };

    let vars = Vars::new(root, &name, workspace);

    // MCP: `.mcp.json`, plus any the manifest names or holds inline.
    let mut mcp = taurus_mcp::McpConfig::default();
    let mut server_names = Vec::new();
    let mut mcp_files = Vec::new();
    if root.join(".mcp.json").is_file() {
        mcp_files.push(root.join(".mcp.json"));
    }
    match &manifest.mcp_servers {
        Some(serde_json::Value::Object(inline)) => {
            let text = serde_json::json!({ "mcpServers": inline }).to_string();
            note_oauth(&text, &mut warnings);
            merge_mcp(
                &mut mcp,
                taurus_mcp::parse(&text),
                &name,
                &vars,
                &mut problems,
                &mut server_names,
                &mut warnings,
            );
        }
        other => mcp_files.extend(paths(root, other.as_ref(), "mcpServers", &mut problems)),
    }
    for file in &mcp_files {
        let parsed = std::fs::read_to_string(file)
            .map_err(|e| e.to_string())
            .and_then(|text| {
                note_oauth(&text, &mut warnings);
                taurus_mcp::parse(&text)
            })
            .map_err(|e| format!("{}: {e}", file.display()));
        merge_mcp(
            &mut mcp,
            parsed,
            &name,
            &vars,
            &mut problems,
            &mut server_names,
            &mut warnings,
        );
    }

    // Hooks: `hooks/hooks.json`, plus any the manifest names, in Taurus's
    // format. Inline hooks in the manifest are Claude's shape, so they're
    // named as such.
    let mut hooks = taurus_hooks::HookConfig::default();
    let mut hook_files = Vec::new();
    if root.join("hooks/hooks.json").is_file() {
        hook_files.push(root.join("hooks/hooks.json"));
    }
    match &manifest.hooks {
        Some(serde_json::Value::Object(_)) => unsupported.push(Unsupported {
            part: "hooks (in the manifest)".into(),
            reason: "hooks written inline are in Claude Code's format, which Taurus doesn't run \
                     yet; give Taurus-format hooks in hooks/hooks.json"
                .into(),
        }),
        other => hook_files.extend(paths(root, other.as_ref(), "hooks", &mut problems)),
    }
    let mut taurus_hook_files = Vec::new();
    for file in hook_files {
        if is_claude_hooks(&file) {
            unsupported.push(Unsupported {
                part: relative(root, &file),
                reason: "it's in Claude Code's hook format, which Taurus doesn't run yet; \
                         none of its hooks run"
                    .into(),
            });
            continue;
        }
        match taurus_hooks::load_file(&file) {
            Ok(layer) => {
                for (hook, problem) in &layer.invalid {
                    problems.push(format!("{}: hook \"{hook}\": {problem}", file.display()));
                }
                for (hook, entry) in layer.hooks {
                    hooks
                        .hooks
                        .insert(format!("{name}:{hook}"), vars.hook(entry));
                }
            }
            Err(e) => problems.push(e),
        }
        taurus_hook_files.push(file);
    }

    let source = std::fs::read_to_string(root.join(SOURCE_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());

    let summary = PluginSummary {
        skills: entries(&skill_dirs, |path| {
            path.is_dir() && path.join("SKILL.md").is_file()
        }),
        agents: entries(&agent_dirs, |path| {
            path.is_file() && path.extension().is_some_and(|e| e == "md")
        })
        .into_iter()
        .map(|file| file.trim_end_matches(".md").to_string())
        .collect(),
        mcp_servers: server_names,
        hooks: hooks
            .hooks
            .keys()
            .map(|key| key.trim_start_matches(&format!("{name}:")).to_string())
            .collect(),
        name,
        scope,
        root: root.display().to_string(),
        version: manifest.version,
        description: manifest.description,
        enabled: manifest.default_enabled.unwrap_or(true),
        shadowed: false,
        unsupported,
        problems,
        warnings,
        source,
    };
    Plugin {
        summary,
        root: root.to_path_buf(),
        skill_dirs,
        agent_dirs,
        mcp,
        hooks,
        hook_files: taurus_hook_files,
    }
}

/// Switches a plugin on or off in one scope's `settings.json`.
///
/// Said for a plugin that isn't installed, rather than written: a switch for
/// a name nothing has is a line that does nothing until something takes the
/// name, which is a surprise waiting to happen.
pub fn set_enabled(
    name: &str,
    scope: Scope,
    workspace: Option<&Path>,
    enabled: bool,
) -> Result<(), String> {
    if !installed(workspace).iter().any(|p| p.name() == name) {
        return Err(format!(
            "no plugin named \"{name}\" is installed. `taurus plugin list` shows the ones \
             that are."
        ));
    }
    if scope == Scope::Workspace && workspace.is_none() {
        return Err("there's no project open to switch it in".into());
    }
    config::edit_settings(scope, workspace, |settings| {
        settings
            .plugins
            .get_or_insert_with(BTreeMap::new)
            .insert(name.to_string(), enabled);
    });
    Ok(())
}

/// Installs a plugin from a folder or a git URL into one scope, and returns
/// what was installed.
///
/// It goes in under the name its manifest gives, in a folder of that name.
/// It's checked before it lands: one that wouldn't load isn't installed, and
/// the reasons are the error. A clone is pinned to the commit it was made at,
/// and its `.git` is dropped so a project's `.taurus/plugins/` holds plain
/// files rather than a repository inside a repository.
pub fn add(
    from: &str,
    git_ref: Option<&str>,
    scope: Scope,
    workspace: Option<&Path>,
) -> Result<PluginSummary, String> {
    let plugins = dir(scope, workspace).ok_or("there's no project open to add it to")?;
    std::fs::create_dir_all(&plugins).map_err(|e| format!("{}: {e}", plugins.display()))?;
    let staged = Staged::fetch(from, git_ref, &plugins)?;
    let plugin = load(staged.path(), scope, workspace);
    if !plugin.summary.problems.is_empty() {
        return Err(format!(
            "{from} isn't a plugin Taurus can load, so nothing was installed:\n  {}",
            plugin.summary.problems.join("\n  ")
        ));
    }
    let target = plugins.join(plugin.name());
    if target.exists() {
        return Err(format!(
            "a plugin named \"{}\" is already installed at {}. Update it with `taurus plugin \
             update {}`, or remove it first.",
            plugin.name(),
            target.display(),
            plugin.name()
        ));
    }
    staged.install(&target)?;
    Ok(load(&target, scope, workspace).summary)
}

/// Fetches an installed plugin again from where it came from, and puts the
/// new copy in place of the old one. The old one stays if anything fails.
pub fn update(name: &str, scope: Scope, workspace: Option<&Path>) -> Result<PluginSummary, String> {
    check_name(name)?;
    let plugins = dir(scope, workspace).ok_or("there's no project open")?;
    let target = plugins.join(name);
    let source: PluginSource = std::fs::read_to_string(target.join(SOURCE_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .ok_or_else(|| {
            format!(
                "{} wasn't installed by `taurus plugin add`, so there's nowhere recorded to \
                 update it from",
                target.display()
            )
        })?;
    let staged = Staged::fetch(&source.from, None, &plugins)?;
    let plugin = load(staged.path(), scope, workspace);
    if !plugin.summary.problems.is_empty() {
        return Err(format!(
            "the new copy of {name} wouldn't load, so the installed one is kept:\n  {}",
            plugin.summary.problems.join("\n  ")
        ));
    }
    if plugin.name() != name {
        return Err(format!(
            "{} now names itself \"{}\", not \"{name}\", so the installed one is kept. Add \
             it under its new name and remove this one.",
            source.from,
            plugin.name()
        ));
    }
    let old = plugins.join(format!(".old-{}", uuid::Uuid::new_v4()));
    std::fs::rename(&target, &old).map_err(|e| format!("{}: {e}", target.display()))?;
    if let Err(e) = staged.install(&target) {
        let _ = std::fs::rename(&old, &target);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(load(&target, scope, workspace).summary)
}

/// Deletes an installed plugin's folder, and its switch in that scope's
/// settings.
pub fn remove(name: &str, scope: Scope, workspace: Option<&Path>) -> Result<(), String> {
    check_name(name)?;
    let target = dir(scope, workspace)
        .ok_or("there's no project open")?
        .join(name);
    if !target.is_dir() {
        return Err(format!("no plugin is installed at {}", target.display()));
    }
    std::fs::remove_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    config::edit_settings(scope, workspace, |settings| {
        if let Some(switches) = settings.plugins.as_mut() {
            switches.remove(name);
        }
        if settings.plugins.as_ref().is_some_and(BTreeMap::is_empty) {
            settings.plugins = None;
        }
    });
    Ok(())
}

/// Whether `from` names a git repository rather than a folder.
fn is_git(from: &str) -> bool {
    ["https://", "http://", "ssh://", "git://", "git@"]
        .iter()
        .any(|prefix| from.starts_with(prefix))
        || from.ends_with(".git")
}

/// A plugin fetched into a hidden folder beside the others, not yet in
/// place. Removed when dropped unless it was installed.
struct Staged {
    path: PathBuf,
    source: PluginSource,
    kept: bool,
}

impl Staged {
    fn fetch(from: &str, git_ref: Option<&str>, plugins: &Path) -> Result<Self, String> {
        let path = plugins.join(format!(".staging-{}", uuid::Uuid::new_v4()));
        let mut staged = Self {
            path,
            source: PluginSource {
                from: from.to_string(),
                commit: None,
            },
            kept: false,
        };
        if is_git(from) {
            staged.source.commit = Some(clone(from, git_ref, &staged.path)?);
            let _ = std::fs::remove_dir_all(staged.path.join(".git"));
        } else {
            if git_ref.is_some() {
                return Err(format!(
                    "{from} is a folder, so there's no git ref to check out"
                ));
            }
            let folder = Path::new(from)
                .canonicalize()
                .map_err(|e| format!("{from}: {e}"))?;
            if !folder.is_dir() {
                return Err(format!("{from} isn't a folder or a git URL"));
            }
            copy_dir(&folder, &staged.path)?;
            staged.source.from = folder.display().to_string();
        }
        Ok(staged)
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Records where it came from, and moves it into place.
    fn install(mut self, target: &Path) -> Result<(), String> {
        let record = serde_json::to_string_pretty(&self.source).map_err(|e| e.to_string())?;
        std::fs::write(self.path.join(SOURCE_FILE), record)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        std::fs::rename(&self.path, target).map_err(|e| format!("{}: {e}", target.display()))?;
        self.kept = true;
        Ok(())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.kept {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Clones `url` into `into` at `git_ref` (the default branch if `None`), and
/// returns the commit it's at.
///
/// Never prompts: a repository that needs a password says so as an error
/// rather than waiting on a terminal nobody is looking at.
fn clone(url: &str, git_ref: Option<&str>, into: &Path) -> Result<String, String> {
    let mut command = std::process::Command::new("git");
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["clone", "--quiet", "--depth", "1"]);
    if let Some(git_ref) = git_ref {
        command.args(["--branch", git_ref]);
    }
    command.arg("--").arg(url).arg(into);
    let output = command
        .output()
        .map_err(|e| format!("couldn't run git to fetch {url}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git couldn't fetch {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let head = std::process::Command::new("git")
        .arg("-C")
        .arg(into)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&head.stdout).trim().to_string())
}

/// Copies a folder's files, leaving out `.git` and links. A link in a plugin
/// could point anywhere on the machine it was made on, and copying through
/// one would install whatever it points at on this one.
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if name == ".git" || kind.is_symlink() {
            continue;
        }
        let target = to.join(&name);
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)
                .map_err(|e| format!("{}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Whether a plugin name can name its parts. Claude Code's rule, narrowed to
/// what Taurus can put in a `/command` and an MCP tool name: lowercase
/// letters, digits and hyphens, starting with a letter, as a `/command` does.
pub fn check_name(name: &str) -> Result<(), String> {
    let shaped = !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !shaped {
        return Err(format!(
            "the plugin name \"{name}\" isn't usable: it names every part as \"{name}:part\", \
             so it has to be lowercase letters, digits and hyphens, starting with a letter"
        ));
    }
    let reserved = ["claude", "anthropic", "claude-code"].contains(&name)
        || ["claude-", "anthropic-", "anthropics-"]
            .iter()
            .any(|prefix| name.starts_with(prefix));
    if reserved {
        return Err(format!(
            "the plugin name \"{name}\" is reserved by Claude Code, so a plugin can't use it"
        ));
    }
    Ok(())
}

/// The paths a manifest key gives: a string or a list of them, each
/// `./`-relative and inside the plugin. Anything else is a problem, said.
fn paths(
    root: &Path,
    value: Option<&serde_json::Value>,
    key: &str,
    problems: &mut Vec<String>,
) -> Vec<PathBuf> {
    let listed: Vec<&serde_json::Value> = match value {
        None | Some(serde_json::Value::Null) => return Vec::new(),
        Some(serde_json::Value::Array(items)) => items.iter().collect(),
        Some(one) => vec![one],
    };
    let mut out = Vec::new();
    for item in listed {
        let Some(text) = item.as_str() else {
            problems.push(format!(
                "the manifest's \"{key}\" has to be a path or a list of paths"
            ));
            continue;
        };
        match inside(root, text) {
            Ok(path) => out.push(path),
            Err(e) => problems.push(format!("the manifest's \"{key}\": {e}")),
        }
    }
    out
}

/// `text` as a path inside `root`, or why not. A plugin's paths can't reach
/// outside it: a skill folder of `../../.ssh` would hand the model your keys
/// as readable "skill files".
fn inside(root: &Path, text: &str) -> Result<PathBuf, String> {
    let Some(relative) = text.strip_prefix("./") else {
        return Err(format!(
            "\"{text}\" has to start with \"./\", relative to the plugin's folder"
        ));
    };
    let joined = root.join(relative);
    let resolved = joined
        .canonicalize()
        .map_err(|_| format!("\"{text}\" doesn't exist in the plugin"))?;
    let base = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !resolved.starts_with(&base) {
        return Err(format!("\"{text}\" points outside the plugin's folder"));
    }
    Ok(resolved)
}

/// The names of the entries in `dirs` that pass `keep`, sorted.
fn entries(dirs: &[PathBuf], keep: impl Fn(&Path) -> bool) -> Vec<String> {
    let mut names: Vec<String> = dirs
        .iter()
        .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| keep(path))
        .filter_map(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    names.dedup();
    names
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

/// Adds one parsed MCP file to a plugin's servers, keyed and filled in.
fn merge_mcp(
    into: &mut taurus_mcp::McpConfig,
    parsed: Result<taurus_mcp::McpConfig, String>,
    plugin: &str,
    vars: &Vars,
    problems: &mut Vec<String>,
    names: &mut Vec<String>,
    skipped: &mut Vec<String>,
) {
    match parsed {
        Ok(layer) => {
            for (server, problem) in layer.invalid {
                problems.push(format!("MCP server \"{server}\": {problem}"));
            }
            for (server, config) in layer.servers {
                // A placeholder Claude Code fills from its own connectors.
                // Started, it fails with an error about building a URL.
                let empty = match &config {
                    taurus_mcp::ServerConfig::Http { url, .. } => url.trim().is_empty(),
                    taurus_mcp::ServerConfig::Stdio { command, .. } => command.trim().is_empty(),
                    taurus_mcp::ServerConfig::Toggle(_) => false,
                };
                if empty {
                    skipped.push(format!(
                        "MCP server \"{server}\" has no address in the plugin (Claude Code fills \
                         it in from its own connectors), so it isn't started"
                    ));
                    continue;
                }
                let key = server_key(plugin, &server);
                if into
                    .servers
                    .insert(key.clone(), vars.server(config))
                    .is_some()
                {
                    problems.push(format!(
                        "two of its MCP servers are both keyed {key}; rename one"
                    ));
                }
                if !names.contains(&server) {
                    names.push(server);
                }
            }
        }
        Err(e) => problems.push(e),
    }
}

/// Says which servers name an OAuth client of their own. Claude Code signs in
/// with it; Taurus registers its own client with the server, which a server
/// that only accepts the named one will refuse.
fn note_oauth(text: &str, warnings: &mut Vec<String>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let Some(servers) = value.get("mcpServers").and_then(|s| s.as_object()) else {
        return;
    };
    for (server, config) in servers {
        if config.get("oauth").is_some() {
            warnings.push(format!(
                "MCP server \"{server}\" names its own OAuth client, which Taurus doesn't use; \
                 if signing in fails, the server only accepts that client"
            ));
        }
    }
}

/// Whether a hooks file is in Claude Code's format: keyed by its event names,
/// each holding a list.
fn is_claude_hooks(file: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(file) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value
        .get("hooks")
        .and_then(|hooks| hooks.as_object())
        .is_some_and(|hooks| {
            hooks
                .iter()
                .any(|(key, value)| CLAUDE_HOOK_EVENTS.contains(&key.as_str()) && value.is_array())
        })
}

/// The paths a plugin's config may name by variable, as Claude Code names
/// them, with Taurus's spellings beside them.
struct Vars {
    pairs: Vec<(&'static str, String)>,
    data: PathBuf,
}

impl Vars {
    fn new(root: &Path, name: &str, workspace: Option<&Path>) -> Self {
        let data = config::home_dir().join("plugin-data").join(name);
        let mut pairs = vec![
            ("${CLAUDE_PLUGIN_ROOT}", root.display().to_string()),
            ("${TAURUS_PLUGIN_ROOT}", root.display().to_string()),
            ("${CLAUDE_PLUGIN_DATA}", data.display().to_string()),
            ("${TAURUS_PLUGIN_DATA}", data.display().to_string()),
        ];
        if let Some(workspace) = workspace {
            pairs.push(("${CLAUDE_PROJECT_DIR}", workspace.display().to_string()));
        }
        Self { pairs, data }
    }

    /// `text` with the variables filled in. The data folder is made the first
    /// time something names it, as Claude Code makes it.
    fn fill(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (var, value) in &self.pairs {
            if out.contains(var) {
                if var.ends_with("_DATA}") {
                    let _ = std::fs::create_dir_all(&self.data);
                }
                out = out.replace(var, value);
            }
        }
        out
    }

    fn server(&self, config: taurus_mcp::ServerConfig) -> taurus_mcp::ServerConfig {
        use taurus_mcp::ServerConfig;
        match config {
            ServerConfig::Stdio {
                command,
                args,
                env,
                disabled,
            } => ServerConfig::Stdio {
                command: self.fill(&command),
                args: args.iter().map(|a| self.fill(a)).collect(),
                env: env.into_iter().map(|(k, v)| (k, self.fill(&v))).collect(),
                disabled,
            },
            ServerConfig::Http {
                url,
                headers,
                disabled,
            } => ServerConfig::Http {
                url: self.fill(&url),
                headers: headers
                    .into_iter()
                    .map(|(k, v)| (k, self.fill(&v)))
                    .collect(),
                disabled,
            },
            toggle => toggle,
        }
    }

    fn hook(&self, entry: taurus_hooks::HookEntry) -> taurus_hooks::HookEntry {
        match entry {
            taurus_hooks::HookEntry::Hook(mut hook) => {
                hook.command = self.fill(&hook.command);
                hook.args = hook.args.iter().map(|a| self.fill(a)).collect();
                taurus_hooks::HookEntry::Hook(hook)
            }
            toggle => toggle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::isolated_home;

    /// Writes `files` under `root`, making folders as needed.
    fn write(root: &Path, files: &[(&str, &str)]) {
        for (path, text) in files {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        }
    }

    /// A plugin laid out as Claude Code lays one out, with one of each part.
    fn ops(root: &Path) {
        write(
            root,
            &[
                (
                    MANIFEST,
                    r#"{"name": "ops", "version": "1.2.0", "description": "Deploys things",
                        "author": {"name": "Ops"}, "commands": "./commands"}"#,
                ),
                (
                    "skills/deploy/SKILL.md",
                    "---\nname: deploy\ndescription: Deploys a service\n---\n\nDeploy it.\n",
                ),
                (
                    "agents/reviewer.md",
                    "---\nname: reviewer\ndescription: Reviews a deploy\n---\n\nReview it.\n",
                ),
                (
                    ".mcp.json",
                    r#"{"mcpServers": {"db": {"command": "${CLAUDE_PLUGIN_ROOT}/bin/db",
                        "args": ["--data", "${CLAUDE_PLUGIN_DATA}"]}}}"#,
                ),
                (
                    "hooks/hooks.json",
                    r#"{"hooks": {"lint": {"on": "pre_tool_use",
                        "command": "${CLAUDE_PLUGIN_ROOT}/lint.sh"}}}"#,
                ),
                ("commands/status.md", "Say the status."),
            ],
        );
    }

    #[test]
    fn a_plugin_in_claude_codes_layout_is_read_part_by_part() {
        let _home = isolated_home();
        let root = dir(Scope::Global, None).unwrap().join("ops");
        ops(&root);

        let found = installed(None);
        let [plugin] = found.as_slice() else {
            panic!("{found:?}")
        };
        let summary = &plugin.summary;
        assert_eq!(summary.name, "ops");
        assert_eq!(summary.version.as_deref(), Some("1.2.0"));
        assert!(summary.problems.is_empty(), "{:?}", summary.problems);
        assert!(summary.warnings.is_empty(), "{:?}", summary.warnings);
        assert!(plugin.is_active());
        assert_eq!(summary.skills, ["deploy"]);
        assert_eq!(summary.agents, ["reviewer"]);
        assert_eq!(summary.mcp_servers, ["db"]);
        assert_eq!(summary.hooks, ["lint"]);
        // The manifest key and the folder are one part, named once.
        assert_eq!(
            summary
                .unsupported
                .iter()
                .map(|u| u.part.as_str())
                .collect::<Vec<_>>(),
            ["commands"]
        );

        // Named under the plugin, with its paths filled in.
        let root = plugin.root().display().to_string();
        let Some(taurus_mcp::ServerConfig::Stdio { command, args, .. }) =
            plugin.mcp().servers.get("plugin_ops_db")
        else {
            panic!("{:?}", plugin.mcp().servers)
        };
        assert_eq!(command, &format!("{root}/bin/db"));
        assert!(args[1].ends_with("plugin-data/ops"), "{args:?}");
        assert!(config::home_dir().join("plugin-data/ops").is_dir());
        let Some(taurus_hooks::HookEntry::Hook(hook)) = plugin.hooks().hooks.get("ops:lint") else {
            panic!("{:?}", plugin.hooks().hooks)
        };
        assert_eq!(hook.command, format!("{root}/lint.sh"));
    }

    #[test]
    fn a_server_with_no_address_is_named_and_not_started() {
        let _home = isolated_home();
        let root = dir(Scope::Global, None).unwrap().join("mail");
        write(
            &root,
            &[(
                ".mcp.json",
                r#"{"mcpServers": {"gmail": {"type": "http", "url": ""},
                    "docs": {"type": "http", "url": "https://example.invalid/mcp"}}}"#,
            )],
        );
        let found = installed(None);
        let plugin = &found[0];
        assert_eq!(plugin.summary.mcp_servers, ["docs"]);
        assert_eq!(
            plugin.mcp().servers.keys().collect::<Vec<_>>(),
            ["plugin_mail_docs"]
        );
        assert!(plugin.summary.warnings[0].contains("\"gmail\" has no address"));
    }

    #[test]
    fn a_servers_key_holds_only_what_a_tool_name_can() {
        assert_eq!(
            server_key("eng", "google calendar"),
            "plugin_eng_google_calendar"
        );
        assert_eq!(server_key("eng", "db-1_x"), "plugin_eng_db-1_x");
    }

    #[test]
    fn its_skills_and_agents_are_named_under_it() {
        let _home = isolated_home();
        ops(&dir(Scope::Global, None).unwrap().join("ops"));

        let (skills, problems) =
            taurus_skills::SkillCatalog::discover(&config::skill_sources(None));
        assert!(problems.is_empty(), "{problems:?}");
        let skill = skills.get("ops:deploy").expect("the plugin's skill");
        assert_eq!(skill.plugin.as_deref(), Some("ops"));
        assert!(!skills.contains("deploy"));

        let (agents, problems) =
            taurus_agents::AgentCatalog::discover(&config::agent_sources(None));
        assert!(problems.is_empty(), "{problems:?}");
        let agent = agents.get("ops:reviewer").expect("the plugin's agent");
        assert!(
            agent.borrowed,
            "a plugin's agent file is never written back"
        );
    }

    #[test]
    fn a_folder_with_no_manifest_is_named_after_itself() {
        let _home = isolated_home();
        let root = dir(Scope::Global, None).unwrap().join("notes");
        write(
            &root,
            &[(
                "skills/jot/SKILL.md",
                "---\nname: jot\ndescription: Jots\n---\n\nJot.\n",
            )],
        );
        let found = installed(None);
        assert_eq!(found[0].name(), "notes");
        assert!(found[0].is_active());
    }

    #[test]
    fn a_name_that_cant_name_its_parts_stops_it_loading() {
        assert!(check_name("ops-tools2").is_ok());
        for bad in ["Ops", "ops_tools", "2ops", "ops.tools", ""] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        let reserved = check_name("claude-helpers").unwrap_err();
        assert!(reserved.contains("reserved"), "{reserved}");

        let _home = isolated_home();
        let root = dir(Scope::Global, None).unwrap().join("x");
        write(&root, &[(MANIFEST, r#"{"name": "My Plugin"}"#)]);
        let found = installed(None);
        assert!(!found[0].is_active());
        assert!(found[0].summary.problems[0].contains("isn't usable"));
    }

    #[test]
    fn a_path_outside_the_plugin_is_refused() {
        let _home = isolated_home();
        let base = dir(Scope::Global, None).unwrap();
        write(
            &base,
            &[(
                "secrets/key/SKILL.md",
                "---\nname: key\ndescription: k\n---\nk\n",
            )],
        );
        write(
            &base.join("sly"),
            &[(
                MANIFEST,
                r#"{"name": "sly", "skills": ["./../secrets", "skills"]}"#,
            )],
        );
        let found = installed(None);
        let sly = found.iter().find(|p| p.name() == "sly").unwrap();
        let problems = sly.summary.problems.join("\n");
        assert!(
            problems.contains("points outside the plugin's folder"),
            "{problems}"
        );
        assert!(problems.contains("has to start with \"./\""), "{problems}");
        assert!(!sly.is_active());
        assert!(sly.skill_dirs().iter().all(|d| d.starts_with(sly.root())));
    }

    #[test]
    fn hooks_in_claude_codes_format_are_named_and_none_run() {
        let _home = isolated_home();
        let root = dir(Scope::Global, None).unwrap().join("guard");
        write(
            &root,
            &[(
                "hooks/hooks.json",
                r#"{"hooks": {"PreToolUse": [{"matcher": "Bash",
                    "hooks": [{"type": "command", "command": "check"}]}]}}"#,
            )],
        );
        let found = installed(None);
        let plugin = &found[0];
        assert!(plugin.hooks().hooks.is_empty());
        assert!(
            plugin.summary.problems.is_empty(),
            "{:?}",
            plugin.summary.problems
        );
        assert_eq!(plugin.summary.unsupported[0].part, "hooks/hooks.json");
        assert!(plugin.summary.unsupported[0]
            .reason
            .contains("Claude Code's hook format"));
    }

    #[test]
    fn settings_switch_a_plugin_off_and_a_project_overrides_you_by_name() {
        let _home = isolated_home();
        let workspace = tempfile::tempdir().unwrap();
        let ws = workspace.path();
        ops(&dir(Scope::Global, None).unwrap().join("ops"));
        write(
            &dir(Scope::Global, None).unwrap().join("quiet"),
            &[(MANIFEST, r#"{"name": "quiet", "defaultEnabled": false}"#)],
        );
        let on = |workspace| -> Vec<String> {
            active(workspace)
                .iter()
                .map(|p| p.name().to_string())
                .collect()
        };
        assert_eq!(on(None), ["ops"]);

        config::edit_settings(Scope::Global, None, |s| {
            s.plugins = Some(BTreeMap::from([("ops".into(), false)]))
        });
        assert!(on(None).is_empty());
        config::edit_settings(Scope::Workspace, Some(ws), |s| {
            s.plugins = Some(BTreeMap::from([("quiet".into(), true)]))
        });
        // The project switched one on and said nothing of the other, which
        // stays as you left it.
        assert_eq!(on(Some(ws)), ["quiet"]);
    }

    #[test]
    fn a_project_plugin_replaces_yours_of_the_same_name() {
        let _home = isolated_home();
        let workspace = tempfile::tempdir().unwrap();
        ops(&dir(Scope::Global, None).unwrap().join("ops"));
        ops(&dir(Scope::Workspace, Some(workspace.path()))
            .unwrap()
            .join("ops"));
        let found = installed(Some(workspace.path()));
        let yours = found.iter().find(|p| p.scope() == Scope::Global).unwrap();
        assert!(yours.summary.shadowed && !yours.is_active());
        let active = active(Some(workspace.path()));
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].scope(), Scope::Workspace);
    }

    #[test]
    fn a_projects_plugins_wait_for_trust_and_are_named_when_asked() {
        let _home = isolated_home();
        let workspace = tempfile::tempdir().unwrap();
        let ws = workspace.path();
        ops(&dir(Scope::Workspace, Some(ws)).unwrap().join("ops"));

        assert!(!config::skill_sources(Some(ws))
            .iter()
            .any(|s| s.plugin.is_some()));
        let status = crate::trust::status(ws);
        assert!(status.decision_needed);
        assert_eq!(status.pending.plugins, ["ops"]);
        assert_eq!(status.pending.skills, 1);
        assert!(status.pending.mcp_commands[0].starts_with("plugin_ops_db: "));
        assert!(status.pending.hook_commands[0].starts_with("ops:lint: "));

        crate::trust::trust(ws).unwrap();
        assert!(config::skill_sources(Some(ws))
            .iter()
            .any(|s| s.plugin.as_deref() == Some("ops")));
    }

    #[test]
    fn adding_from_a_folder_copies_it_in_and_records_where_from() {
        let _home = isolated_home();
        let source = tempfile::tempdir().unwrap();
        ops(source.path());
        std::fs::create_dir_all(source.path().join(".git")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hosts", source.path().join("skills/hosts")).unwrap();

        let added = add(source.path().to_str().unwrap(), None, Scope::Global, None).unwrap();
        assert_eq!(added.name, "ops");
        let root = dir(Scope::Global, None).unwrap().join("ops");
        assert!(root.join("skills/deploy/SKILL.md").is_file());
        assert!(!root.join(".git").exists());
        assert!(
            !root.join("skills/hosts").exists(),
            "a link is never followed in"
        );
        assert_eq!(
            added.source.unwrap().from,
            source.path().canonicalize().unwrap().display().to_string()
        );

        let again = add(source.path().to_str().unwrap(), None, Scope::Global, None).unwrap_err();
        assert!(again.contains("already installed"), "{again}");

        set_enabled("ops", Scope::Global, None, false).unwrap();
        assert!(active(None).is_empty());
        remove("ops", Scope::Global, None).unwrap();
        assert!(installed(None).is_empty());
        assert_eq!(config::read_settings(Scope::Global, None).plugins, None);
        let unknown = set_enabled("ops", Scope::Global, None, true).unwrap_err();
        assert!(unknown.contains("no plugin named"), "{unknown}");
    }

    #[test]
    fn a_plugin_that_wouldnt_load_isnt_installed_and_leaves_nothing() {
        let _home = isolated_home();
        let source = tempfile::tempdir().unwrap();
        write(source.path(), &[(MANIFEST, r#"{"name": "Bad Name"}"#)]);
        let refused = add(source.path().to_str().unwrap(), None, Scope::Global, None).unwrap_err();
        assert!(refused.contains("nothing was installed"), "{refused}");
        let left: Vec<_> = std::fs::read_dir(dir(Scope::Global, None).unwrap())
            .unwrap()
            .flatten()
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    /// A git repository holding [`ops`], at one commit.
    fn repo() -> tempfile::TempDir {
        let parent = tempfile::tempdir().unwrap();
        let repo = parent.path().join("ops.git");
        ops(&repo);
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(status.status.success(), "{status:?}");
        };
        git(&["init", "--quiet"]);
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "one"]);
        parent
    }

    #[test]
    fn adding_from_git_pins_the_commit_and_update_moves_it_on_purpose() {
        let _home = isolated_home();
        let parent = repo();
        let url = parent.path().join("ops.git").display().to_string();

        let added = add(&url, None, Scope::Global, None).unwrap();
        let first = added.source.unwrap().commit.unwrap();
        assert_eq!(first.len(), 40, "{first}");
        let root = dir(Scope::Global, None).unwrap().join("ops");
        assert!(
            !root.join(".git").exists(),
            "a plain folder, not a nested repository"
        );

        // A new commit upstream changes nothing here until it's asked for.
        let repo = parent.path().join("ops.git");
        std::fs::write(
            repo.join("skills/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: Deploys v2\n---\n\nDeploy it.\n",
        )
        .unwrap();
        let commit = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["commit", "--quiet", "-am", "two"])
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(commit.status.success());
        let skill = || std::fs::read_to_string(root.join("skills/deploy/SKILL.md")).unwrap();
        assert!(skill().contains("Deploys a service"));

        let updated = update("ops", Scope::Global, None).unwrap();
        assert_ne!(updated.source.unwrap().commit.unwrap(), first);
        assert!(skill().contains("Deploys v2"));
    }

    #[test]
    fn a_fetch_that_fails_says_what_git_said() {
        let _home = isolated_home();
        let missing = tempfile::tempdir().unwrap().path().join("nothing.git");
        let refused = add(missing.to_str().unwrap(), None, Scope::Global, None).unwrap_err();
        assert!(refused.contains("git couldn't fetch"), "{refused}");
    }
}

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
//!   hooks/hooks.json             Taurus's hook format, or Claude Code's
//! ```
//!
//! Codex and GitHub Copilot plugins are the same layout with the manifest
//! somewhere else (see [`MANIFESTS`]), so they load the same way. A hooks
//! file in Claude Code's format (which Codex uses too) or Copilot's is
//! translated by [`taurus_hooks::dialect`].
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
//! listed by name as unsupported, with why, rather than skipped in silence,
//! and so is any hook event or hook type a translated hooks file has that
//! Taurus can't run.
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

/// Where a plugin's manifest may be, first found wins: Claude Code's, then
/// Codex's, then the places GitHub Copilot looks. One plugin has one
/// manifest, so the order only matters for a folder that carries several,
/// and those are copies of one another.
pub const MANIFESTS: &[&str] = &[
    MANIFEST,
    ".codex-plugin/plugin.json",
    "plugin.json",
    ".plugin/plugin.json",
    ".github/plugin/plugin.json",
];

/// A marketplace's catalog of plugins, relative to its root: Claude Code's
/// path, and Copilot's. Taurus doesn't read marketplaces, but says when it's
/// handed one.
const MARKETPLACES: &[&str] = &[
    ".claude-plugin/marketplace.json",
    ".github/plugin/marketplace.json",
];

/// Where a hooks file is found without the manifest naming one: Claude
/// Code's and Codex's place, then Copilot's two.
const HOOK_FILES: &[&str] = &[
    "hooks/hooks.json",
    "hooks.json",
    "com.github.copilot/hooks/hooks.json",
];

/// The manifest a plugin folder has, if any.
fn manifest_path(root: &Path) -> Option<PathBuf> {
    MANIFESTS
        .iter()
        .map(|m| root.join(m))
        .find(|path| path.is_file())
}

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
    // Codex's: how its app shows the plugin.
    "interface",
    // Copilot's: names other formats the plugin also ships.
    "extensions",
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
    ("apps", "Taurus has no Codex apps"),
    (
        "rules",
        "Taurus has no rule files from plugins; put them in a skill",
    ),
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
        "com.github.copilot/commands",
        "Taurus has no command files; a skill marked user-invocable is its /command",
    ),
    (
        "com.github.copilot/rules",
        "Taurus has no rule files from plugins; put them in a skill",
    ),
    ("com.github.copilot/lsp.json", "Taurus doesn't run language servers"),
    (
        "SKILL.md",
        "a skill at the plugin's root isn't read; put it in skills/<name>/SKILL.md",
    ),
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

    let manifest_path = manifest_path(root).unwrap_or_else(|| root.join(MANIFEST));
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
        skill_dirs.push(canonical(root.join("skills")));
    }
    for path in paths(root, manifest.skills.as_ref(), "skills", &mut problems) {
        if !skill_dirs.contains(&path) {
            skill_dirs.push(path);
        }
    }

    // Agents: the manifest's list replaces the default, as in Claude Code.
    let agent_dirs: Vec<PathBuf> = match &manifest.agents {
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
        // Claude Code's and Codex's folder, and Copilot's.
        None => ["agents", "com.github.copilot/agents"]
            .iter()
            .map(|dir| root.join(dir))
            .filter(|dir| dir.is_dir())
            .collect(),
    };

    let vars = Vars::new(root, &name, workspace);

    // MCP: `.mcp.json`, plus any the manifest names or holds inline.
    let mut mcp = taurus_mcp::McpConfig::default();
    let mut server_names = Vec::new();
    let mut mcp_files = Vec::new();
    if root.join(".mcp.json").is_file() {
        mcp_files.push(canonical(root.join(".mcp.json")));
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
        // Codex's manifests name `./.mcp.json`, which is read already.
        other => {
            for path in paths(root, other.as_ref(), "mcpServers", &mut problems) {
                if !mcp_files.contains(&path) {
                    mcp_files.push(path);
                }
            }
        }
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

    // Hooks: the default files, plus any the manifest names or holds inline,
    // each in Taurus's format or translated from another agent's.
    let mut hooks = taurus_hooks::HookConfig::default();
    let mut hook_files: Vec<PathBuf> = HOOK_FILES
        .iter()
        .map(|file| root.join(file))
        .filter(|path| path.is_file())
        .map(canonical)
        .collect();
    let mut add_hooks = |layer: BTreeMap<String, taurus_hooks::HookEntry>| {
        for (hook, entry) in layer {
            hooks
                .hooks
                .insert(format!("{name}:{hook}"), vars.hook(entry));
        }
    };
    match &manifest.hooks {
        Some(inline @ serde_json::Value::Object(_)) => {
            let translated = taurus_hooks::dialect::translate(inline);
            note_translated(
                "hooks (in the manifest)",
                &translated,
                &mut unsupported,
                &mut problems,
            );
            add_hooks(translated.hooks);
        }
        other => {
            for path in paths(root, other.as_ref(), "hooks", &mut problems) {
                if !hook_files.contains(&path) {
                    hook_files.push(path);
                }
            }
        }
    }
    let mut taurus_hook_files = Vec::new();
    for file in hook_files {
        let parsed = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        match parsed {
            Some(value) if taurus_hooks::dialect::is_foreign(&value) => {
                let translated = taurus_hooks::dialect::translate(&value);
                note_translated(
                    &relative(root, &file),
                    &translated,
                    &mut unsupported,
                    &mut problems,
                );
                add_hooks(translated.hooks);
            }
            _ => match taurus_hooks::load_file(&file) {
                Ok(layer) => {
                    for (hook, problem) in &layer.invalid {
                        problems.push(format!("{}: hook \"{hook}\": {problem}", file.display()));
                    }
                    add_hooks(layer.hooks);
                }
                Err(e) => problems.push(e),
            },
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
        // `reviewer.agent.md` is Copilot's spelling of `reviewer.md`.
        .map(|file| {
            let stem = file.trim_end_matches(".md");
            stem.strip_suffix(".agent").unwrap_or(stem).to_string()
        })
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
    if let Some(catalog) = staged.marketplace() {
        return Err(catalog);
    }
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

/// A folder in a repository on GitHub or GitLab, as its web page addresses
/// it: `https://github.com/o/r/tree/<ref>/<path>`. The ref is one segment;
/// a branch with a slash in it is added by the repository URL and `--ref`.
struct TreeUrl {
    repo: String,
    git_ref: String,
    path: String,
}

impl TreeUrl {
    fn parse(from: &str) -> Option<Self> {
        if !from.starts_with("https://") && !from.starts_with("http://") {
            return None;
        }
        let (repo, rest) = from
            .split_once("/-/tree/")
            .or_else(|| from.split_once("/tree/"))?;
        let (git_ref, path) = rest.trim_end_matches('/').split_once('/')?;
        if git_ref.is_empty() || path.is_empty() {
            return None;
        }
        Some(Self {
            repo: repo.to_string(),
            git_ref: git_ref.to_string(),
            path: path.to_string(),
        })
    }
}

/// The name a fetched folder is staged under: what it's called where it came
/// from. A plugin with no manifest is named after its folder, so it has to
/// keep that folder's name while it's checked.
fn source_name(from: &str) -> String {
    let last = from
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\', ':'])
        .next()
        .unwrap_or_default();
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

/// `~` and `~/…` as a shell reads them, since the desktop's box is not a shell.
fn expand_home(from: &str) -> PathBuf {
    let home = || directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    match from.strip_prefix('~') {
        Some("") => home().unwrap_or_else(|| from.into()),
        Some(rest) if rest.starts_with(['/', '\\']) => home()
            .map(|h| h.join(&rest[1..]))
            .unwrap_or_else(|| from.into()),
        _ => PathBuf::from(from),
    }
}

/// A plugin fetched into a hidden folder beside the others, not yet in
/// place. The plugin is `path`, somewhere inside `root`; `root` is removed
/// when dropped, and so is the plugin unless it was installed.
struct Staged {
    root: PathBuf,
    path: PathBuf,
    source: PluginSource,
    /// The branch a clone landed on, to name a marketplace's plugins by URL.
    branch: Option<String>,
    kept: bool,
}

impl Staged {
    fn fetch(from: &str, git_ref: Option<&str>, plugins: &Path) -> Result<Self, String> {
        let root = plugins.join(format!(".staging-{}", uuid::Uuid::new_v4()));
        let mut staged = Self {
            path: root.clone(),
            root,
            source: PluginSource {
                from: from.to_string(),
                commit: None,
            },
            branch: None,
            kept: false,
        };
        if let Some(tree) = TreeUrl::parse(from) {
            if git_ref.is_some() {
                return Err(format!(
                    "{from} already names its ref ({}), so leave the ref empty",
                    tree.git_ref
                ));
            }
            let clone_dir = staged.root.join(".clone");
            let (commit, branch) = clone(&tree.repo, Some(&tree.git_ref), &clone_dir)?;
            staged.source.commit = Some(commit);
            staged.branch = branch;
            let sub = inside(&clone_dir, &format!("./{}", tree.path))
                .ok()
                .filter(|p| p.is_dir())
                .ok_or_else(|| {
                    format!(
                        "{} has no folder {} at {}",
                        tree.repo, tree.path, tree.git_ref
                    )
                })?;
            staged.path = sub;
        } else if is_git(from) {
            staged.path = staged.root.join(source_name(from));
            let (commit, branch) = clone(from, git_ref, &staged.path)?;
            staged.source.commit = Some(commit);
            staged.branch = branch;
            let _ = std::fs::remove_dir_all(staged.path.join(".git"));
        } else {
            if git_ref.is_some() {
                return Err(format!(
                    "{from} is a folder, so there's no git ref to check out"
                ));
            }
            let folder = expand_home(from)
                .canonicalize()
                .map_err(|e| format!("{from} isn't a folder here, or a git URL ({e})"))?;
            if !folder.is_dir() {
                return Err(format!("{from} isn't a folder or a git URL"));
            }
            staged.path = staged.root.join(source_name(&folder.display().to_string()));
            copy_dir(&folder, &staged.path)?;
            staged.source.from = folder.display().to_string();
        }
        Ok(staged)
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// If what was fetched is a marketplace (a catalog of plugins) rather
    /// than a plugin, says so, and names the plugins in it that can be added
    /// and how. `None` for anything with a plugin manifest.
    fn marketplace(&self) -> Option<String> {
        if manifest_path(&self.path).is_some() {
            return None;
        }
        let (catalog_path, text) = MARKETPLACES.iter().find_map(|m| {
            std::fs::read_to_string(self.path.join(m))
                .ok()
                .map(|text| (*m, text))
        })?;
        let catalog: serde_json::Value = serde_json::from_str(&text).ok()?;
        let entries = catalog.get("plugins")?.as_array()?;
        // Only plugins kept in the catalog's own repository can be named by a
        // path in it; the rest live in other repositories.
        let local: Vec<(&str, &str)> = entries
            .iter()
            .filter_map(|p| {
                let name = p.get("name")?.as_str()?;
                let path = p.get("source")?.as_str()?;
                Some((name, path.trim_start_matches("./").trim_end_matches('/')))
            })
            .collect();
        let from = &self.source.from;
        let mut text = format!(
            "{from} is a marketplace, a catalog of {} plugins, not one plugin. Taurus \
             doesn't read marketplaces yet, so add the plugin you want by itself",
            entries.len()
        );
        let base = from.trim_end_matches('/');
        let address = |path: &str| -> Option<String> {
            if TreeUrl::parse(from).is_some() {
                Some(format!("{base}/{path}"))
            } else if is_git(from) {
                let web = base.strip_suffix(".git").unwrap_or(base);
                let branch = self.branch.as_deref()?;
                (web.starts_with("https://") || web.starts_with("http://"))
                    .then(|| format!("{web}/tree/{branch}/{path}"))
            } else {
                Some(Path::new(base).join(path).display().to_string())
            }
        };
        match local.first().and_then(|(_, path)| address(path)) {
            Some(_) => {
                text.push_str(", by its folder:");
                const SHOWN: usize = 8;
                for (name, path) in local.iter().take(SHOWN) {
                    if let Some(at) = address(path) {
                        text.push_str(&format!("\n  {name}: {at}"));
                    }
                }
                if local.len() > SHOWN {
                    text.push_str(&format!(
                        "\n  …and {} more listed in {catalog_path}",
                        local.len() - SHOWN
                    ));
                }
            }
            None => text.push_str(&format!(
                ": its folder or repository is named in its {catalog_path}."
            )),
        }
        Some(text)
    }

    /// Records where it came from, and moves it into place.
    fn install(mut self, target: &Path) -> Result<(), String> {
        let record = serde_json::to_string_pretty(&self.source).map_err(|e| e.to_string())?;
        std::fs::write(self.path.join(SOURCE_FILE), record)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        let _ = std::fs::remove_dir_all(self.path.join(".git"));
        std::fs::rename(&self.path, target).map_err(|e| format!("{}: {e}", target.display()))?;
        self.kept = true;
        Ok(())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        // The plugin has been moved out of `root` if it was installed, so
        // this removes only what's left: the rest of a clone, or everything.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Clones `url` into `into` at `git_ref` (the default branch if `None`), and
/// returns the commit it's at and the branch, if it's on one.
///
/// Never prompts: a repository that needs a password says so as an error
/// rather than waiting on a terminal nobody is looking at.
fn clone(
    url: &str,
    git_ref: Option<&str>,
    into: &Path,
) -> Result<(String, Option<String>), String> {
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
    let rev_parse = |args: &[&str]| -> Result<String, String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(into)
            .arg("rev-parse")
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let commit = rev_parse(&["HEAD"])?;
    let branch = rev_parse(&["--abbrev-ref", "HEAD"])?;
    let branch = (!branch.is_empty() && branch != "HEAD").then_some(branch);
    Ok((commit, branch))
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

/// `path` as [`paths`] gives one, so a default and the same file named by
/// the manifest are seen to be one file and read once.
fn canonical(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
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
    let base = canonical(root.to_path_buf());
    path.strip_prefix(&base)
        .or_else(|_| path.strip_prefix(root))
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

/// Records what a translated hooks file couldn't bring, under the file's
/// name.
fn note_translated(
    file: &str,
    translated: &taurus_hooks::dialect::Translated,
    unsupported: &mut Vec<Unsupported>,
    problems: &mut Vec<String>,
) {
    for (what, why) in &translated.unsupported {
        unsupported.push(Unsupported {
            part: format!("{file}: {what}"),
            reason: why.clone(),
        });
    }
    for problem in &translated.problems {
        problems.push(format!("{file}: {problem}"));
    }
}

/// The paths a plugin's config may name by variable, as Claude Code names
/// them, with Codex's, Copilot's and Taurus's spellings beside them.
struct Vars {
    pairs: Vec<(&'static str, String)>,
    root: PathBuf,
    data: PathBuf,
}

impl Vars {
    fn new(root: &Path, name: &str, workspace: Option<&Path>) -> Self {
        let data = config::home_dir().join("plugin-data").join(name);
        let mut pairs = Vec::new();
        for var in [
            "${CLAUDE_PLUGIN_ROOT}",
            "${TAURUS_PLUGIN_ROOT}",
            "${PLUGIN_ROOT}",
            "${COPILOT_PLUGIN_ROOT}",
        ] {
            pairs.push((var, root.display().to_string()));
        }
        for var in [
            "${CLAUDE_PLUGIN_DATA}",
            "${TAURUS_PLUGIN_DATA}",
            "${PLUGIN_DATA}",
            "${COPILOT_PLUGIN_DATA}",
        ] {
            pairs.push((var, data.display().to_string()));
        }
        if let Some(workspace) = workspace {
            pairs.push(("${CLAUDE_PROJECT_DIR}", workspace.display().to_string()));
        }
        Self {
            pairs,
            root: root.to_path_buf(),
            data,
        }
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
                // A translated hook's script runs under a shell, which expands
                // these from its environment, quoting and all: filled in as
                // text, a folder with a `$` or a quote in its name would be
                // read as shell. `cmd` doesn't expand `${…}`, so on Windows
                // it's filled, as everything else is.
                let shell = cfg!(not(windows))
                    && hook.foreign.is_some()
                    && hook.args.first().is_some_and(|a| a == "-c");
                if let Some(foreign) = hook.foreign.as_mut() {
                    for (var, value) in &self.pairs {
                        let key = var.trim_start_matches("${").trim_end_matches('}');
                        foreign
                            .env
                            .entry(key.to_string())
                            .or_insert_with(|| value.clone());
                    }
                    foreign.env = std::mem::take(&mut foreign.env)
                        .into_iter()
                        .map(|(k, v)| (k, self.fill(&v)))
                        .collect();
                    foreign.cwd = foreign.cwd.as_deref().map(|cwd| self.fill(cwd));
                }
                // A command that starts `./scripts/check.sh` means the
                // plugin's own script, as Codex runs it: hooks run in the
                // workspace, where that path names nothing, and a guard that
                // can't start refuses every call.
                if hook.foreign.is_some() {
                    if let Some(script) = hook.args.get_mut(1) {
                        if script.starts_with("./") || script.starts_with("../") {
                            let root = if shell {
                                "\"$PLUGIN_ROOT\"".to_string()
                            } else {
                                format!("\"{}\"", self.root.display())
                            };
                            *script = format!("{root}/{script}");
                        }
                    }
                    if hook.command.starts_with("./") || hook.command.starts_with("../") {
                        hook.command = self.root.join(&hook.command).display().to_string();
                    }
                }
                if shell {
                    if hook.args.iter().any(|a| a.contains("PLUGIN_DATA")) {
                        let _ = std::fs::create_dir_all(&self.data);
                    }
                } else {
                    hook.command = self.fill(&hook.command);
                    hook.args = hook.args.iter().map(|a| self.fill(a)).collect();
                }
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
    fn hooks_in_claude_codes_format_are_translated_and_what_cant_run_is_named() {
        let _home = isolated_home();
        let root = dir(Scope::Global, None).unwrap().join("guard");
        write(
            &root,
            &[(
                "hooks/hooks.json",
                r#"{"hooks": {
                    "PreToolUse": [{"matcher": "Bash",
                        "hooks": [{"type": "command", "command": "\"${CLAUDE_PLUGIN_ROOT}/check.sh\""}]}],
                    "SessionStart": [{"hooks": [{"type": "command", "command": "hello"}]}]
                }}"#,
            )],
        );
        let found = installed(None);
        let plugin = &found[0];
        assert!(
            plugin.summary.problems.is_empty(),
            "{:?}",
            plugin.summary.problems
        );
        assert_eq!(plugin.summary.hooks, ["PreToolUse#1"]);
        let taurus_hooks::HookEntry::Hook(hook) = &plugin.hooks().hooks["guard:PreToolUse#1"]
        else {
            panic!("not a hook");
        };
        let foreign = hook.foreign.as_ref().unwrap();
        assert_eq!(
            foreign.env["CLAUDE_PLUGIN_ROOT"],
            plugin.root().display().to_string()
        );
        assert_eq!(
            foreign.env["PLUGIN_ROOT"],
            foreign.env["CLAUDE_PLUGIN_ROOT"]
        );
        assert_eq!(plugin.summary.unsupported.len(), 1);
        assert_eq!(
            plugin.summary.unsupported[0].part,
            "hooks/hooks.json: SessionStart"
        );
    }

    #[test]
    fn a_codex_plugin_and_a_copilot_plugin_load_from_their_own_manifests() {
        let _home = isolated_home();
        let plugins = dir(Scope::Global, None).unwrap();
        write(
            &plugins.join("codex-one"),
            &[
                (
                    ".codex-plugin/plugin.json",
                    r#"{"name": "codex-one", "version": "1.0.0", "skills": "./skills/",
                        "interface": {"displayName": "Codex One"}, "apps": "./.app.json"}"#,
                ),
                (
                    "skills/lint/SKILL.md",
                    "---\nname: lint\ndescription: Lints\n---\nLint.\n",
                ),
            ],
        );
        write(
            &plugins.join("copilot-one"),
            &[
                (
                    "plugin.json",
                    r#"{"name": "copilot-one", "description": "From Copilot"}"#,
                ),
                (
                    "hooks.json",
                    r#"{"version": 1, "hooks": {"preToolUse": [
                        {"type": "command", "bash": "./deny.sh", "powershell": "./deny.ps1"}]}}"#,
                ),
                (
                    "com.github.copilot/agents/reviewer.agent.md",
                    "---\nname: reviewer\ndescription: Reviews\n---\nReview.\n",
                ),
            ],
        );
        let found = installed(None);
        let codex = found.iter().find(|p| p.name() == "codex-one").unwrap();
        assert!(
            codex.summary.problems.is_empty(),
            "{:?}",
            codex.summary.problems
        );
        assert!(
            codex.summary.warnings.is_empty(),
            "{:?}",
            codex.summary.warnings
        );
        assert_eq!(codex.summary.version.as_deref(), Some("1.0.0"));
        assert_eq!(codex.summary.skills, ["lint"]);
        assert!(codex.summary.unsupported.iter().any(|u| u.part == "apps"));

        let copilot = found.iter().find(|p| p.name() == "copilot-one").unwrap();
        assert!(
            copilot.summary.problems.is_empty(),
            "{:?}",
            copilot.summary.problems
        );
        assert_eq!(copilot.summary.description.as_deref(), Some("From Copilot"));
        assert_eq!(copilot.summary.agents, ["reviewer"]);
        assert_eq!(copilot.summary.hooks, ["preToolUse#1"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_codex_hook_naming_its_script_relatively_runs_the_plugins_script() {
        let _home = isolated_home();
        let root = dir(Scope::Global, None).unwrap().join("replay");
        write(
            &root,
            &[(
                "hooks.json",
                r#"{"hooks": {"Stop": [{"hooks": [
                    {"type": "command", "command": "./scripts/upload.sh --now"}]}]}}"#,
            )],
        );
        let found = installed(None);
        let taurus_hooks::HookEntry::Hook(hook) = &found[0].hooks().hooks["replay:Stop#1"] else {
            panic!("not a hook");
        };
        assert_eq!(hook.args[1], "\"$PLUGIN_ROOT\"/./scripts/upload.sh --now");
    }

    #[test]
    fn a_file_the_manifest_names_that_is_read_by_default_is_read_once() {
        let _home = isolated_home();
        let root = dir(Scope::Global, None).unwrap().join("twice");
        write(
            &root,
            &[
                (
                    ".codex-plugin/plugin.json",
                    r#"{"name": "twice", "mcpServers": "./.mcp.json", "hooks": "./hooks/hooks.json"}"#,
                ),
                (".mcp.json", r#"{"mcpServers": {"db": {"command": "db"}}}"#),
                (
                    "hooks/hooks.json",
                    r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "x"}]}]}}"#,
                ),
            ],
        );
        let found = installed(None);
        assert!(
            found[0].summary.problems.is_empty(),
            "{:?}",
            found[0].summary.problems
        );
        assert_eq!(found[0].summary.mcp_servers, ["db"]);
        assert_eq!(found[0].hook_files().len(), 1);
    }

    #[test]
    fn hooks_held_inline_in_a_manifest_are_translated() {
        let _home = isolated_home();
        let root = dir(Scope::Global, None).unwrap().join("inline");
        write(
            &root,
            &[(
                MANIFEST,
                r#"{"name": "inline", "hooks": {"PostToolUse": [{"matcher": "Edit|Write",
                    "hooks": [{"type": "command", "command": "fmt"}]}]}}"#,
            )],
        );
        let found = installed(None);
        assert!(
            found[0].summary.problems.is_empty(),
            "{:?}",
            found[0].summary.problems
        );
        assert_eq!(found[0].summary.hooks, ["PostToolUse#1"]);
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
    fn adding_a_plugin_with_no_manifest_names_it_after_its_folder() {
        let _home = isolated_home();
        let parent = tempfile::tempdir().unwrap();
        let source = parent.path().join("notes");
        write(
            &source,
            &[(
                "skills/jot/SKILL.md",
                "---\nname: jot\ndescription: Jots\n---\n\nJot.\n",
            )],
        );
        let added = add(source.to_str().unwrap(), None, Scope::Global, None).unwrap();
        assert_eq!(added.name, "notes");
        assert!(dir(Scope::Global, None)
            .unwrap()
            .join("notes/skills/jot/SKILL.md")
            .is_file());

        // And from git, named after the repository.
        let repo = parent.path().join("jots.git");
        write(
            &repo,
            &[(
                "skills/jot/SKILL.md",
                "---\nname: jot\ndescription: Jots\n---\n\nJot.\n",
            )],
        );
        for args in [
            &["init", "--quiet"][..],
            &["add", "."],
            &["commit", "--quiet", "-m", "one"],
        ] {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
        }
        let added = add(repo.to_str().unwrap(), None, Scope::Global, None).unwrap();
        assert_eq!(added.name, "jots");
        let left: Vec<_> = std::fs::read_dir(dir(Scope::Global, None).unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left.len(), 2, "no staging folder is left: {left:?}");
    }

    #[test]
    fn a_marketplace_is_refused_with_the_plugins_it_names() {
        let _home = isolated_home();
        let source = tempfile::tempdir().unwrap();
        write(
            source.path(),
            &[
                (
                    MARKETPLACES[0],
                    r#"{"name": "shop", "plugins": [
                        {"name": "notes", "source": "./plugins/notes"},
                        {"name": "far", "source": {"source": "url", "url": "https://x/far.git"}}]}"#,
                ),
                (
                    "plugins/notes/skills/jot/SKILL.md",
                    "---\nname: jot\ndescription: J\n---\n",
                ),
            ],
        );
        let refused = add(source.path().to_str().unwrap(), None, Scope::Global, None).unwrap_err();
        assert!(
            refused.contains("is a marketplace, a catalog of 2 plugins"),
            "{refused}"
        );
        let notes = source.path().canonicalize().unwrap().join("plugins/notes");
        assert!(
            refused.contains(&format!("notes: {}", notes.display())),
            "{refused}"
        );
        assert!(!refused.contains("far:"), "{refused}");
        assert!(installed(None).is_empty());
        assert_eq!(
            std::fs::read_dir(dir(Scope::Global, None).unwrap())
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn a_web_page_for_a_folder_in_a_repository_is_read_as_one() {
        let tree = TreeUrl::parse("https://github.com/o/r/tree/main/plugins/notes/").unwrap();
        assert_eq!(
            (
                tree.repo.as_str(),
                tree.git_ref.as_str(),
                tree.path.as_str()
            ),
            ("https://github.com/o/r", "main", "plugins/notes")
        );
        let lab = TreeUrl::parse("https://gitlab.com/g/r/-/tree/v2/notes").unwrap();
        assert_eq!(
            (lab.repo.as_str(), lab.git_ref.as_str()),
            ("https://gitlab.com/g/r", "v2")
        );
        assert!(TreeUrl::parse("https://github.com/o/r").is_none());
        assert!(TreeUrl::parse("https://github.com/o/r/tree/main").is_none());
        assert_eq!(source_name("https://github.com/o/notes.git"), "notes");
        assert_eq!(source_name("git@github.com:o/notes.git"), "notes");
        assert_eq!(source_name("/a/b/notes/"), "notes");
        let home = directories::BaseDirs::new()
            .unwrap()
            .home_dir()
            .to_path_buf();
        assert_eq!(expand_home("~/x"), home.join("x"));
        assert_eq!(expand_home("a/~/x"), PathBuf::from("a/~/x"));
    }

    #[test]
    fn a_fetch_that_fails_says_what_git_said() {
        let _home = isolated_home();
        let missing = tempfile::tempdir().unwrap().path().join("nothing.git");
        let refused = add(missing.to_str().unwrap(), None, Scope::Global, None).unwrap_err();
        assert!(refused.contains("git couldn't fetch"), "{refused}");
    }
}

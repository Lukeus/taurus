//! Creating, copying and deleting skills and sub-agents from the app. The
//! rules are in [`crate::library`]; this is where they meet the live catalogs,
//! which are rescanned after every change so the drawers and the next turn see
//! it.

use super::*;

use crate::instructions::InstructionsTier;
use crate::library::{self, PluginPart};
use taurus_skills::proposal::{validate_proposal, SaveTarget, SkillProposal};

impl Host {
    /// Marks which plugin skills and agents come from a plugin fetched from
    /// somewhere. See [`SkillSummary::managed`].
    pub(super) async fn managed_plugins(&self) -> Vec<String> {
        let workspace = self.workspace().await;
        off_runtime_or_default(move || library::managed_plugins(Some(&workspace))).await
    }

    /// Where a new skill or agent is written: one of the two `.taurus` folders,
    /// or a plugin you made.
    async fn save_root(
        &self,
        target: SaveTarget,
        plugin: Option<&str>,
        part: PluginPart,
    ) -> Result<PathBuf, String> {
        let workspace = self.workspace().await;
        if let Some(plugin) = plugin {
            let plugin = plugin.to_string();
            return off_runtime("finding the plugin", move || {
                library::plugin_part_dir(&plugin, part, Some(&workspace))
            })
            .await;
        }
        Ok(match (part, target) {
            (PluginPart::Skills, SaveTarget::Project) => config::workspace_skills_dir(&workspace),
            (PluginPart::Skills, SaveTarget::User) => config::user_skills_dir(),
            (PluginPart::Agents, SaveTarget::Project) => config::workspace_agents_dir(&workspace),
            (PluginPart::Agents, SaveTarget::User) => config::user_agents_dir(),
        })
    }

    /// The folder a new agent is written to. For the app's agent editor, which
    /// validates the draft itself — see `validated_agent_root` in `src-tauri`.
    pub async fn agent_save_root(
        &self,
        target: SaveTarget,
        plugin: Option<&str>,
    ) -> Result<PathBuf, String> {
        self.save_root(target, plugin, PluginPart::Agents).await
    }

    /// Every folder a skill or an agent may be deleted from: the two `.taurus`
    /// folders for that part, and that part's folders in plugins you made.
    async fn owned_dirs(&self, part: PluginPart) -> Vec<PathBuf> {
        let workspace = self.workspace().await;
        off_runtime_or_default(move || {
            let mut dirs = match part {
                PluginPart::Skills => vec![
                    config::user_skills_dir(),
                    config::workspace_skills_dir(&workspace),
                ],
                PluginPart::Agents => vec![
                    config::user_agents_dir(),
                    config::workspace_agents_dir(&workspace),
                ],
            };
            for plugin in crate::plugins::installed(Some(&workspace)) {
                if plugin.summary.source.is_none() {
                    dirs.extend(match part {
                        PluginPart::Skills => plugin.skill_dirs().to_vec(),
                        PluginPart::Agents => plugin.agent_dirs().to_vec(),
                    });
                }
            }
            dirs
        })
        .await
    }

    /// Writes a skill made in the editor and rescans, so it is usable on the
    /// next message. Answers with its folder.
    ///
    /// Checked as a proposal is — the format, a trigger line that fits, and no
    /// near-duplicate of a skill already here — because a skill written by
    /// hand and one written by the model are the same file and are read the
    /// same way. The bundling check a proposal also gets is not run: that one
    /// catches a model naming a script it didn't include, and a person who
    /// names `scripts/run.sh` is about to add it.
    pub async fn create_skill(
        &self,
        draft: &SkillProposal,
        target: SaveTarget,
        plugin: Option<&str>,
    ) -> Result<String, String> {
        validate_proposal(draft, &*self.catalog.read().await)
            .map_err(|e| format!("this skill can't be saved as written: {e}"))?;
        let root = self.save_root(target, plugin, PluginPart::Skills).await?;
        let draft = draft.clone();
        let dir = off_runtime("saving the skill", move || {
            library::create_skill(&draft, &root)
        })
        .await?;
        info!(dir = %dir.display(), "skill saved from the editor");
        self.rescan_skills().await;
        Ok(dir.display().to_string())
    }

    /// Deletes a skill Taurus wrote and rescans. Answers with the folder that
    /// went. See [`library::delete_skill`] for what is refused.
    pub async fn delete_skill(&self, name: &str) -> Result<String, String> {
        let skill = self.skill(name).await?;
        let owned = self.owned_dirs(PluginPart::Skills).await;
        let dir = off_runtime("deleting the skill", move || {
            library::delete_skill(&skill, &owned)
        })
        .await?;
        info!(skill = name, dir = %dir.display(), "skill deleted");
        self.rescan_skills().await;
        Ok(dir.display().to_string())
    }

    /// Copies a skill into the `.taurus/skills` of its own tier and rescans.
    /// Answers with the copy's folder. See [`library::copy_skill`].
    pub async fn copy_skill(&self, name: &str) -> Result<String, String> {
        let skill = self.skill(name).await?;
        let workspace = self.workspace().await;
        let root = match skill.tier {
            taurus_skills::SkillTier::Project => config::workspace_skills_dir(&workspace),
            taurus_skills::SkillTier::User => config::user_skills_dir(),
        };
        let dir = off_runtime("copying the skill", move || {
            library::copy_skill(&skill, &root)
        })
        .await?;
        info!(skill = name, dir = %dir.display(), "skill copied");
        self.rescan_skills().await;
        Ok(dir.display().to_string())
    }

    async fn skill(&self, name: &str) -> Result<taurus_skills::Skill, String> {
        self.catalog.read().await.get(name).cloned().ok_or_else(|| {
            format!("there's no skill named \"{name}\" any more. Rescan to see what's there.")
        })
    }

    /// The file to open to change an agent, writing a copy of your own first
    /// when the agent can't be changed where it is. Rescans when it wrote one,
    /// so the copy is the agent the drawer shows. See
    /// [`library::agent_edit_target`].
    pub async fn agent_file(&self, name: &str) -> Result<String, String> {
        let agent = self.agent(name).await?;
        let workspace = self.workspace().await;
        let managed = match &agent.plugin {
            Some(plugin) => self.managed_plugins().await.contains(plugin),
            None => false,
        };
        let (target, forks) = library::agent_edit_target(&agent, &workspace, managed);
        if !forks {
            return Ok(target.display().to_string());
        }
        let path = off_runtime("writing your copy of the agent", move || {
            library::fork_agent(&agent, &target)
        })
        .await?;
        info!(agent = name, path = %path.display(), "agent copied to edit");
        self.rescan_agents().await;
        Ok(path.display().to_string())
    }

    /// Deletes an agent file Taurus owns and rescans. Answers with the file
    /// that went. See [`library::delete_agent`].
    pub async fn delete_agent(&self, name: &str) -> Result<String, String> {
        let agent = self.agent(name).await?;
        let owned = self.owned_dirs(PluginPart::Agents).await;
        let path = off_runtime("deleting the agent", move || {
            library::delete_agent(&agent, &owned)
        })
        .await?;
        info!(agent = name, path = %path.display(), "agent deleted");
        self.rescan_agents().await;
        Ok(path.display().to_string())
    }

    async fn agent(&self, name: &str) -> Result<AgentDefinition, String> {
        self.agents.read().await.get(name).cloned().ok_or_else(|| {
            format!("there's no agent named \"{name}\" any more. Rescan to see what's there.")
        })
    }

    /// The `TAURUS.md` for a tier, created empty if it isn't there. Answers
    /// with its path, for the canvas to open. See [`library::start_brief`].
    pub async fn start_brief(&self, tier: InstructionsTier) -> Result<String, String> {
        let workspace = self.workspace().await;
        let path = off_runtime("starting the brief", move || {
            library::start_brief(tier, &workspace)
        })
        .await?;
        Ok(path.display().to_string())
    }
}

//! Which model each kind of job runs on. See [`config::ModelRoutes`].

use super::*;

use config::{ModelRoute, ModelRoutes};

/// A job routed when it's asked for, rather than when the roster is scanned
/// the way a delegate is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    /// Drafting an agent or a skill in its editor.
    Utility,
    /// Reviewing a turn's changes.
    Review,
}

impl Job {
    fn name(self) -> &'static str {
        match self {
            Self::Utility => "utility",
            Self::Review => "review",
        }
    }

    fn route(self, routes: &ModelRoutes) -> Option<&ModelRoute> {
        match self {
            Self::Utility => routes.utility.as_ref(),
            Self::Review => routes.review.as_ref(),
        }
    }
}

impl Host {
    /// The routes as Settings shows them.
    pub async fn routes(&self) -> ModelRoutes {
        self.settings.read().await.routes.clone()
    }

    /// Saves the routes and re-resolves the roster, so a delegate routed here
    /// runs on its new model from the next message rather than the next launch.
    ///
    /// Global only. See [`config::Settings::routes`].
    ///
    /// A provider that isn't configured is refused rather than saved, because
    /// it's a typo or a stale choice and Settings is where it can be fixed on
    /// the spot. One that's configured but has no key is saved: the key may be
    /// about to be added, and until it is the route reports a problem and the
    /// job runs on the conversation's model.
    pub async fn set_routes(&self, routes: ModelRoutes) -> Result<(), String> {
        let routes = routes.normalized();
        let providers = self.providers().await;
        for (job, route) in routes.named() {
            if !route.provider.is_empty() && !providers.iter().any(|p| p.id == route.provider) {
                return Err(format!(
                    "the {job} route names provider '{}', which isn't configured. Add it under \
                     Settings › Models first, or leave the provider empty to use the \
                     conversation's.",
                    route.provider
                ));
            }
        }

        self.edit_global_setting(|s| s.routes = Some(routes))
            .await?;
        let found = self.route_problems().await;
        self.replace_problems(ProblemSource::Routes, found).await;
        self.rescan_agents().await;
        Ok(())
    }

    /// The provider and model `job` runs on: its route when it has a usable
    /// one, or else the conversation's, which the caller passes in.
    ///
    /// Falls back rather than failing. A route that can't be used is already
    /// on the problem list, and refusing a review over it would make a cheaper
    /// model the reason nothing got reviewed.
    pub async fn routed(
        &self,
        job: Job,
        provider: Arc<dyn Provider>,
        model: &str,
    ) -> (Arc<dyn Provider>, String) {
        let route = job.route(&self.settings.read().await.routes).cloned();
        let Some(route) = route else {
            return (provider, model.to_string());
        };
        if route.provider.is_empty() {
            return (provider, route.model);
        }
        // Built and kept, unlike a delegate's: somebody is waiting on this one,
        // and the next draft or review reuses it.
        let built = match self.check_provider(&route.provider).await {
            Ok(_) => self.provider(&route.provider).await,
            Err(e) => Err(e),
        };
        match built {
            Ok(routed) => (routed, route.model),
            Err(e) => {
                tracing::warn!(
                    job = job.name(),
                    route = %route.label(),
                    error = %e,
                    "route not usable; running on the conversation's model"
                );
                (provider, model.to_string())
            }
        }
    }

    /// The route one sub-agent runs on, by name or else for every delegate,
    /// with its label. `None` when neither is set or neither can be used.
    ///
    /// A route that can't be used is skipped for the next one rather than
    /// reported here: [`Self::route_problems`] reports it once, not once per
    /// agent it would have covered.
    pub(super) async fn agent_route(
        &self,
        routes: &ModelRoutes,
        name: &str,
    ) -> Option<(String, AgentModel)> {
        for route in [routes.agents.get(name), routes.delegate.as_ref()]
            .into_iter()
            .flatten()
        {
            if route.provider.is_empty() {
                return Some((
                    route.label(),
                    AgentModel {
                        provider: None,
                        model: route.model.clone(),
                    },
                ));
            }
            if let Ok(provider) = self.usable_provider(&route.provider).await {
                return Some((
                    route.label(),
                    AgentModel {
                        provider: Some(provider),
                        model: route.model.clone(),
                    },
                ));
            }
        }
        None
    }

    /// Every route that names a provider that can't be used, and what runs
    /// instead.
    pub(super) async fn route_problems(&self) -> Vec<Problem> {
        let routes = self.settings.read().await.routes.clone();
        let mut problems = Vec::new();
        for (job, route) in routes.named() {
            if route.provider.is_empty() {
                continue;
            }
            if let Err(e) = self.check_provider(&route.provider).await {
                let instead = if job.starts_with("agents.") && routes.delegate.is_some() {
                    "the delegate route"
                } else {
                    "the conversation's model"
                };
                problems.push(Problem::new(
                    ProblemSource::Routes,
                    format!(
                        "the {job} route sends work to {}, but {e}. It runs on {instead} until \
                         that's fixed.",
                        route.label()
                    ),
                ));
            }
        }
        problems
    }

    /// The provider configured under `id`, built on first use, if it can be
    /// used at all.
    ///
    /// For the roster, which a reload resolves and a window waits on: deferred
    /// so that resolving reads no key. See [`Self::deferred_provider`].
    pub(super) async fn usable_provider(&self, id: &str) -> Result<Arc<dyn Provider>, String> {
        self.check_provider(id).await?;
        self.deferred_provider(id)
            .await
            .ok_or_else(|| format!("no provider configured with id '{id}'"))
    }

    /// Whether the provider configured under `id` can be used: it exists, and
    /// it has a key if its backend won't answer without one.
    ///
    /// Asks whether a key is stored, not what it is, so it never raises a
    /// keychain dialog. A provider that's configured but unreachable — an
    /// Ollama that isn't running — passes, and fails when it's used.
    async fn check_provider(&self, id: &str) -> Result<ProviderConfig, String> {
        let config = self
            .provider_config(id)
            .await
            .ok_or_else(|| format!("no provider configured with id '{id}'"))?;
        if !needs_key(&config) {
            return Ok(config);
        }
        let asked = config.clone();
        let status = tokio::task::spawn_blocking(move || asked.key_status())
            .await
            .map_err(|e| format!("checking the key for '{id}' failed: {e}"))?;
        if status == secrets::KeyStatus::Missing {
            return Err(format!(
                "'{id}' has no API key. Add one under Settings › Models"
            ));
        }
        Ok(config)
    }
}

/// Whether this backend refuses every request without a key. A local
/// OpenAI-compatible server usually has none, so only OpenAI's own API counts
/// among those.
fn needs_key(config: &ProviderConfig) -> bool {
    match config.kind {
        ProviderKind::Anthropic | ProviderKind::Gemini => true,
        ProviderKind::OpenAiCompatible => {
            taurus_provider_openai::is_openai_itself(&config.base_url)
        }
        ProviderKind::Ollama => false,
    }
}

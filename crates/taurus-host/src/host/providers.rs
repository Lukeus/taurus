//! Model backends: building them, their keys, and which model a session is on.

use super::*;

impl Host {
    /// The provider configured under `id`, built on first use and kept.
    ///
    /// Kept rather than built per call, because building one reads its API key
    /// and that is a call into the OS credential store — see [`Self::built`].
    /// An edited base URL or a new key still takes effect without a restart:
    /// everything that changes either forgets what was built from the old one.
    pub async fn provider(&self, id: &str) -> Result<Arc<dyn Provider>, String> {
        let generation = {
            let built = self.built();
            if let Some(provider) = built.providers.get(id) {
                return Ok(provider.clone());
            }
            built.generation
        };

        let config = self
            .provider_config(id)
            .await
            .ok_or_else(|| format!("no provider configured with id '{id}'"))?;
        // The keychain read itself, and so off the runtime: it blocks for as
        // long as the OS takes to answer, which with a locked keychain is as
        // long as its dialog stays open.
        let lookup = config.clone();
        let key = tokio::task::spawn_blocking(move || lookup.api_key())
            .await
            .map_err(|e| format!("reading the API key for '{id}' failed: {e}"))?;
        let provider = Self::build_provider(config, key);

        let mut built = self.built();
        // Kept only if nothing was forgotten while it was being built: one
        // built from a config a reload has just replaced must not outlive it.
        if built.generation == generation {
            built.providers.insert(id.to_string(), provider.clone());
        }
        Ok(provider)
    }

    /// Forgets every provider built so far, so the next call builds afresh.
    ///
    /// Called wherever what they were built from can change: the provider
    /// list, and any provider's key.
    pub(super) fn forget_providers(&self) {
        let mut built = self.built();
        built.generation += 1;
        built.providers.clear();
    }

    pub(super) fn built(&self) -> std::sync::MutexGuard<'_, BuiltProviders> {
        // A panic while this was held leaves a map, not a broken invariant.
        self.built
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What [`Self::provider`] builds, from a config and the key it resolved.
    pub(super) fn build_provider(config: ProviderConfig, key: Option<String>) -> Arc<dyn Provider> {
        match config.kind {
            ProviderKind::Ollama => Arc::new(
                OllamaProvider::new(config.base_url).with_context_limit(config.context_length),
            ),
            ProviderKind::OpenAiCompatible => {
                let defaults = OpenAiCapabilities::default();
                Arc::new(
                    OpenAiProvider::new(
                        config.id.clone(),
                        config.base_url.clone(),
                        key,
                        OpenAiCapabilities {
                            native_tools: config.native_tools.unwrap_or(defaults.native_tools),
                            vision: config.vision.unwrap_or(defaults.vision),
                            context_length: config
                                .context_length
                                .unwrap_or(defaults.context_length),
                        },
                    )
                    .with_api_prefix(config.api_prefix.clone())
                    .with_api_key_header(config.api_key_header.clone())
                    .with_models(
                        config
                            .models
                            .iter()
                            .map(|m| ModelSpec {
                                id: m.id.clone(),
                                display_name: m.display_name.clone(),
                                context_length: m.context_length,
                                native_tools: m.native_tools,
                                vision: m.vision,
                            })
                            .collect(),
                    ),
                )
            }

            // Neither of the next two takes `native_tools`: both back model
            // families that call tools natively, and a prompted fallback there
            // would be a worse implementation of something that works. Both
            // take `context_length` only as a fallback — each can ask its own
            // backend, and a configured value that disagrees with the model is
            // how a conversation compacts at the wrong moment.
            ProviderKind::Anthropic => Arc::new(
                AnthropicProvider::new(config.id.clone(), config.base_url.clone(), key)
                    // Both of these were read from config and handed only to the
                    // OpenAI adapter, which made this API unusable through a
                    // gateway: the subscription key had nowhere to ride but
                    // `x-api-key`, and the path was forced to `/v1` whatever the
                    // route was published under. The fields always parsed, so
                    // setting them was silently ignored rather than refused.
                    .with_api_prefix(config.api_prefix.clone())
                    .with_api_key_header(config.api_key_header.clone())
                    .with_thinking(
                        config
                            .thinking
                            .as_deref()
                            .map(AnthropicThinking::parse)
                            .unwrap_or_default(),
                    )
                    .with_fallback_capabilities(AnthropicCapabilities {
                        vision: AnthropicCapabilities::default().vision,
                        context_length: config
                            .context_length
                            .unwrap_or(AnthropicCapabilities::default().context_length),
                    })
                    .with_models(config.models.iter().map(|m| m.id.clone()).collect()),
            ),

            ProviderKind::Gemini => Arc::new(
                GeminiProvider::new(config.id.clone(), config.base_url.clone(), key)
                    .with_fallback_capabilities(GeminiCapabilities {
                        vision: GeminiCapabilities::default().vision,
                        context_length: config
                            .context_length
                            .unwrap_or(GeminiCapabilities::default().context_length),
                    })
                    .with_models(config.models.iter().map(|m| m.id.clone()).collect()),
            ),
        }
    }

    pub async fn providers(&self) -> Vec<ProviderConfig> {
        self.providers.read().await.clone()
    }

    /// The global provider layer alone, as an editor must see it.
    ///
    /// [`Self::providers`] returns the effective list with this workspace's
    /// overrides already applied. Editing that and saving it would write every
    /// inherited and overridden value into the global file, so a setting made
    /// for one project would silently follow the user into all the others.
    pub async fn global_providers(&self) -> Vec<ProviderConfig> {
        config::load_providers(None).0
    }

    pub async fn provider_config(&self, id: &str) -> Option<ProviderConfig> {
        self.providers
            .read()
            .await
            .iter()
            .find(|p| p.id == id)
            .cloned()
    }

    /// Persists an edited provider list to the global layer.
    ///
    /// The effective list is then re-resolved rather than assumed to equal what
    /// was passed in: if this workspace overrides one of these providers, the
    /// override still wins, and the UI must show what will actually be used.
    pub async fn set_providers(&self, providers: Vec<ProviderConfig>) {
        config::save_providers(&providers);
        let workspace = self.workspace.read().await.clone();
        let (effective, _) = config::load_providers(Some(&workspace));
        *self.providers.write().await = effective;
        self.forget_providers();
    }

    /// Stores a provider's API key in the OS credential store.
    ///
    /// Takes the id rather than a whole config because the secret is not part
    /// of the config: `providers.json` is written on every settings save, and a
    /// key that travelled with it would eventually be written into it.
    pub async fn set_provider_key(&self, provider_id: &str, key: &str) -> Result<(), String> {
        if !self
            .providers
            .read()
            .await
            .iter()
            .any(|p| p.id == provider_id)
        {
            return Err(format!("no provider configured with id '{provider_id}'"));
        }
        secrets::store(provider_id, key)?;
        // The provider built with the old key would go on sending it.
        self.forget_providers();
        Ok(())
    }

    pub async fn clear_provider_key(&self, provider_id: &str) -> Result<(), String> {
        secrets::clear(provider_id)?;
        self.forget_providers();
        Ok(())
    }

    /// Where each configured provider's key is coming from.
    ///
    /// Returned for the whole list at once because that is how the settings
    /// screen draws it, and asking per provider would mean one credential-store
    /// round trip per row.
    pub async fn key_statuses(&self) -> Vec<(String, secrets::KeyStatus)> {
        let providers = self.providers.read().await.clone();
        let statuses = |providers: &[ProviderConfig]| -> Vec<(String, secrets::KeyStatus)> {
            providers
                .iter()
                .map(|p| (p.id.clone(), p.key_status()))
                .collect()
        };
        // On a blocking thread: each status is a read of the OS keychain, which
        // with a locked keychain waits on its dialog.
        let asked = providers.clone();
        tokio::task::spawn_blocking(move || statuses(&asked))
            .await
            .unwrap_or_else(|_| statuses(&providers))
    }

    /// Whether this machine can store keys at all, so a frontend can offer the
    /// field or explain its absence instead of failing on save.
    pub fn keychain_available() -> bool {
        secrets::available()
    }

    /// Picks a provider and model, preferring the caller's choice, then what
    /// was used last, then whatever the backend offers first.
    pub async fn resolve_model(
        &self,
        provider_id: Option<&str>,
        model: Option<&str>,
    ) -> Result<(String, String), String> {
        let providers = self.providers().await;
        let settings = self.settings().await;

        let chosen = provider_id
            .map(str::to_string)
            .or(settings.last_provider)
            .filter(|id| providers.iter().any(|p| &p.id == id))
            .or_else(|| providers.first().map(|p| p.id.clone()))
            .ok_or_else(|| "no providers are configured".to_string())?;

        if let Some(model) = model {
            return Ok((chosen, model.to_string()));
        }

        let configured = providers
            .iter()
            .find(|p| p.id == chosen)
            .and_then(|p| p.default_model.clone())
            .filter(|m| !m.trim().is_empty());

        let provider = self.provider(&chosen).await?;
        let available = match provider.models().await {
            Ok(available) => available,
            // A listing is not something every backend has. An Azure APIM
            // route often exposes the chat endpoint and nothing else, and that
            // is no reason to be unusable when the config already says which
            // model to talk to.
            Err(e) => {
                let Some(default) = configured else {
                    return Err(format!(
                        "could not list models from '{chosen}': {e}. If this backend has no \
                         model listing, give it a `default_model` in providers.json or name \
                         one with --model."
                    ));
                };
                return Ok((chosen, default));
            }
        };

        let preferred = settings
            .last_model
            .filter(|m| available.iter().any(|a| &a.id == m))
            // Ahead of "whatever came first" but behind the model this
            // workspace was last worked in, which is a decision the user made
            // more recently than the config file.
            .or(configured)
            .or_else(|| available.first().map(|m| m.id.clone()))
            .ok_or_else(|| format!("provider '{chosen}' has no models available"))?;

        Ok((chosen, preferred))
    }
}

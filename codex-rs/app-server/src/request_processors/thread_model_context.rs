//! Resolve typed model launch choices without changing the server defaults.

use super::*;
use crate::model_context::PendingModelContext;
use crate::model_context::context_error;
use codex_app_server_protocol::ModelContextInfo;
use codex_app_server_protocol::ModelContextIntent;
use codex_app_server_protocol::ModelContextResolveParams;
use codex_app_server_protocol::ModelContextResolveResponse;
use codex_core::model_context::ResolvedModelContext;
use codex_models_manager::manager::RefreshStrategy;

impl ThreadRequestProcessor {
    pub(crate) fn model_context_owner_generation(&self) -> u64 {
        self.auth_manager
            .auth_change_state_receiver()
            .borrow()
            .owner_generation
    }

    pub(crate) async fn resolve_model_context(
        &self,
        connection_id: ConnectionId,
        local: bool,
        params: ModelContextResolveParams,
    ) -> Result<ModelContextResolveResponse, JSONRPCErrorError> {
        if !local && params.model_catalog_json.is_some() {
            return Err(context_error(
                "modelContextUnavailable",
                "catalog paths require a local connection",
            ));
        }
        if (params.intent == ModelContextIntent::Start) != params.thread_id.is_none() {
            return Err(invalid_params(
                "resume/fork require threadId; start must omit threadId",
            ));
        }
        let revision = self.model_context_owner_generation();
        let mut saved_config = None;
        let mut saved_snapshot = None;
        let mut saved_model = None;
        let mut saved_provider = None;
        let mut saved_cwd = None;
        if let Some(id) = &params.thread_id {
            let thread_id =
                ThreadId::from_string(id).map_err(|_| invalid_params("invalid threadId"))?;
            if let Ok(thread) = self.thread_manager.get_thread(thread_id).await {
                saved_model = Some(thread.config_snapshot().await.model);
                saved_config = Some(thread.config().await);
                saved_snapshot = saved_config
                    .as_ref()
                    .and_then(|config| config.resolved_model_context.as_ref())
                    .map(|context| context.snapshot.clone());
                saved_provider = saved_config
                    .as_ref()
                    .map(|config| config.model_provider_id.clone());
                saved_cwd = saved_config.as_ref().map(|config| config.cwd.to_path_buf());
            } else {
                let stored = self.read_stored_thread_for_resume(id, None, false).await?;
                saved_provider = Some(stored.model_provider.clone());
                saved_model = stored.model.clone();
                saved_cwd = Some(stored.cwd.clone());
                let (history, _) = self
                    .load_resume_initial_history_from_stored_thread(stored)
                    .await?;
                if saved_model.is_none() {
                    saved_model =
                        history
                            .get_rollout_items()
                            .iter()
                            .rev()
                            .find_map(|item| match item {
                                RolloutItem::TurnContext(context) => Some(context.model.clone()),
                                _ => None,
                            });
                }
                for item in history.get_rollout_items().iter().rev() {
                    if let RolloutItem::EventMsg(EventMsg::ThreadSettingsApplied(event)) = item
                        && event.thread_id == Some(thread_id)
                    {
                        saved_model = Some(event.thread_settings.model.clone());
                        saved_provider = Some(event.thread_settings.model_provider_id.clone());
                        saved_cwd = Some(event.thread_settings.cwd.to_path_buf());
                        saved_snapshot = event.thread_settings.model_context.clone();
                        break;
                    }
                }
            }
        }
        let loaded_model = saved_model.clone();
        let source_provider_id = saved_provider.clone();
        let provider_changed = params.model_provider.as_ref().is_some_and(|provider| {
            saved_provider
                .as_ref()
                .is_some_and(|saved| saved != provider)
        });
        let mut overrides = ConfigOverrides {
            defer_model_catalog: true,
            cwd: params.cwd.as_ref().map(PathBuf::from).or(saved_cwd),
            model: params
                .model
                .clone()
                .or_else(|| (!provider_changed).then_some(saved_model).flatten()),
            model_provider: params.model_provider.clone().or(saved_provider),
            ..Default::default()
        };
        let requested_catalog = params
            .model_catalog_json
            .as_ref()
            .map(|path| {
                let path = AbsolutePathBuf::resolve_path_against_base(
                    path,
                    self.config.codex_home.as_path(),
                );
                let catalog =
                    codex_core::model_context::load_catalog(path.as_path()).map_err(|_| {
                        context_error(
                            "modelCatalogInvalid",
                            "cannot read or validate model catalog",
                        )
                    })?;
                Ok::<_, JSONRPCErrorError>((path, catalog))
            })
            .transpose()?;
        if let Some((_, catalog)) = &requested_catalog {
            overrides.model_catalog_snapshot = Some(Some(catalog.clone()));
        } else if !provider_changed && let Some(snapshot) = &saved_snapshot {
            overrides.model_catalog_snapshot = Some(
                codex_core::model_context::restore_catalog_snapshot(
                    self.config.codex_home.as_path(),
                    snapshot,
                )
                .map_err(|_| {
                    context_error(
                        "modelCatalogInvalid",
                        "saved model catalog cannot be restored",
                    )
                })?,
            );
        }
        let mut config = self
            .config_manager
            .load_with_overrides(None, overrides.clone())
            .await
            .map_err(|error| config_load_error(&error))?;
        let effective = config.config_layer_stack.effective_config();
        let default_provider = config
            .config_layer_stack
            .required_model_provider()
            .or_else(|| {
                effective
                    .get("model_provider")
                    .and_then(toml::Value::as_str)
            })
            .unwrap_or("openai");
        if params.model.is_none()
            && config.model_provider_id != default_provider
            && (source_provider_id.as_deref() != Some(config.model_provider_id.as_str())
                || loaded_model.is_none())
        {
            // A default model belongs to the provider selected alongside it.
            config.model = None;
        }
        let mut catalog_source = None;
        if let Some((path, catalog)) = requested_catalog {
            config.model_catalog = Some(catalog);
            catalog_source = Some(path.display().to_string());
        } else if !provider_changed && let Some(snapshot) = saved_snapshot {
            let context =
                ResolvedModelContext::from_snapshot(&config, self.auth_manager.clone(), snapshot)
                    .map_err(|_| {
                    context_error(
                        "modelProviderUnavailable",
                        "saved model context cannot be restored",
                    )
                })?;
            config.model_catalog = context.snapshot.catalog.clone();
            config.resolved_model_context = Some(context);
        } else {
            let effective = config.config_layer_stack.effective_config();
            let default_provider = config
                .config_layer_stack
                .required_model_provider()
                .or_else(|| {
                    effective
                        .get("model_provider")
                        .and_then(toml::Value::as_str)
                })
                .unwrap_or("openai");
            if default_provider == config.model_provider_id
                && let Some(path) = effective
                    .get("model_catalog_json")
                    .and_then(toml::Value::as_str)
            {
                let path = AbsolutePathBuf::resolve_path_against_base(
                    path,
                    self.config.codex_home.as_path(),
                );
                config.model_catalog = Some(
                    codex_core::model_context::load_catalog(path.as_path()).map_err(|_| {
                        context_error(
                            "modelCatalogInvalid",
                            "configured model catalog cannot be loaded",
                        )
                    })?,
                );
                catalog_source = Some(path.display().to_string());
            }
        }
        if let (Some(selected), Some(catalog)) = (&config.model, &config.model_catalog)
            && !catalog.models.iter().any(|model| model.slug == *selected)
        {
            return Err(context_error(
                "modelSelectionUnavailable",
                "requested model is absent from the static catalog",
            ));
        }
        if config.resolved_model_context.is_none() || params.model_catalog_json.is_some() {
            config.resolved_model_context = Some(
                ResolvedModelContext::resolve(&config, self.auth_manager.clone(), catalog_source)
                    .map_err(|_| {
                    context_error("modelCatalogInvalid", "cannot resolve model context")
                })?,
            );
        }
        // Reload through the ordinary authority path with the snapshot as trusted host state.
        // Preserve the model selection (including an absent selection) rather than importing
        // the default model associated with a different provider during this reload.
        let selected_model = config.model.clone();
        overrides.model_context = config.resolved_model_context.clone();
        config = self
            .config_manager
            .load_with_overrides(None, overrides)
            .await
            .map_err(|error| config_load_error(&error))?;
        config.model = selected_model;
        let context = config.resolved_model_context.as_ref().ok_or_else(|| {
            context_error(
                "modelContextConflict",
                "resolved model context was not retained",
            )
        })?;
        if !context.matches_config(&config) {
            return Err(context_error(
                "modelContextConflict",
                "effective provider changed during resolution",
            ));
        }
        self.config_manager
            .check_thread_model_provider(&config)
            .await
            .map_err(|error| config_load_error(&error))?;
        let manager = context.models_manager.clone();
        let models = manager
            .list_models(
                RefreshStrategy::OnlineIfUncached,
                config.http_client_factory(),
            )
            .await;
        if models.is_empty() {
            return Err(context_error(
                "modelSelectionUnavailable",
                "the resolved catalog contains no available models",
            ));
        }
        let model = config
            .model
            .clone()
            .or_else(|| {
                models
                    .iter()
                    .find(|model| model.is_default)
                    .or(models.first())
                    .map(|model| model.model.clone())
            })
            .ok_or_else(|| {
                context_error(
                    "modelSelectionUnavailable",
                    "no models are available in this context",
                )
            })?;
        if context
            .snapshot
            .catalog
            .as_ref()
            .is_some_and(|catalog| !catalog.models.iter().any(|entry| entry.slug == model))
        {
            return Err(context_error(
                "modelSelectionUnavailable",
                "requested model is absent from the static catalog",
            ));
        }
        if config.model_provider.model_catalog_url.is_some()
            && !models.iter().any(|entry| entry.model == model)
        {
            return Err(context_error(
                "modelSelectionUnavailable",
                "requested model is absent from the provider's authoritative catalog",
            ));
        }
        config.model = Some(model.clone());
        if revision != self.model_context_owner_generation() {
            return Err(context_error(
                "modelContextExpired",
                "authentication changed during model resolution",
            ));
        }
        if let Some(saved) = saved_config
            && params.intent == ModelContextIntent::Resume
        {
            let route_changed = saved.model_provider_id != config.model_provider_id
                || saved.model_provider != config.model_provider
                || saved.model_catalog != config.model_catalog;
            let snapshot_changed = saved.resolved_model_context.as_ref().is_some_and(|saved| {
                config
                    .resolved_model_context
                    .as_ref()
                    .is_none_or(|resolved| saved != resolved)
            });
            if route_changed || snapshot_changed || loaded_model.as_deref() != Some(model.as_str())
            {
                return Err(context_error(
                    "modelContextConflict",
                    "loaded thread retains its existing model context",
                ));
            }
        }
        let context = config.resolved_model_context.as_ref().ok_or_else(|| {
            context_error(
                "modelContextConflict",
                "resolved model context is unavailable",
            )
        })?;
        let info = ModelContextInfo {
            version: context.snapshot.version,
            model_provider: config.model_provider_id.clone(),
            catalog_digest: context.snapshot.catalog_digest.clone(),
            catalog_source: context.snapshot.catalog_source.clone(),
        };
        let provider = codex_model_provider::create_model_provider(
            config.model_provider.clone(),
            Some(self.auth_manager.clone()),
        );
        let caps = provider.capabilities();
        let capabilities = codex_app_server_protocol::ModelContextCapabilities {
            image_generation: caps.image_generation,
            web_search: caps.web_search,
        };
        let mut warnings = Vec::new();
        if params
            .model_provider
            .as_ref()
            .is_some_and(|requested| requested != &config.model_provider_id)
        {
            warnings.push("Managed configuration selected a different model provider; the effective provider is reported in this response.".to_string());
        }
        let requires_openai_auth = config.model_provider.requires_openai_auth;
        let model_provider = config.model_provider_id.clone();
        let model_context_id = self
            .config_manager
            .model_contexts
            .insert(
                connection_id,
                Arc::new(config),
                params.intent,
                params.thread_id,
                revision,
            )
            .await?;
        Ok(ModelContextResolveResponse {
            requires_openai_auth,
            model_context_id,
            model_provider,
            model,
            context: info,
            capabilities,
            warnings,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "attachment validates the existing lifecycle request fields together"
    )]
    pub(super) async fn pending_model_context(
        &self,
        request: &ConnectionRequestId,
        id: Option<&str>,
        intent: ModelContextIntent,
        thread_id: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
        config: Option<&HashMap<String, serde_json::Value>>,
        cwd: Option<&str>,
    ) -> Result<Option<PendingModelContext>, JSONRPCErrorError> {
        let Some(id) = id else {
            return Ok(None);
        };
        if provider.is_some()
            || config.is_some_and(|config| {
                config.keys().any(|key| {
                    matches!(
                        key.as_str(),
                        "model" | "model_provider" | "model_catalog_json" | "model_providers"
                    ) || key.starts_with("model_providers.")
                })
            })
        {
            return Err(context_error(
                "modelContextConflict",
                "resolve provider/catalog choices first; a model override must use the top-level model field",
            ));
        }
        let pending = self
            .config_manager
            .model_contexts
            .get(
                request.connection_id,
                id,
                self.model_context_owner_generation(),
            )
            .await?;
        if let Some(config) = config {
            let dotted = config.get("features.api_key_model_discovery");
            let nested = config
                .get("features")
                .and_then(|features| features.get("api_key_model_discovery"));
            if config
                .keys()
                .any(|key| key.starts_with("features.api_key_model_discovery."))
            {
                return Err(context_error(
                    "modelContextConflict",
                    "model discovery settings must match the resolved context",
                ));
            }
            for requested in [dotted, nested].into_iter().flatten() {
                let enabled = requested
                    .as_bool()
                    .ok_or_else(|| invalid_params("api_key_model_discovery must be a boolean"))?;
                if !pending
                    .config
                    .resolved_model_context
                    .as_ref()
                    .is_some_and(|context| context.discovery_setting_matches(enabled))
                {
                    return Err(context_error(
                        "modelContextConflict",
                        "model discovery settings changed; resolve a new context",
                    ));
                }
            }
        }
        if pending.intent != intent || pending.thread_id.as_deref() != thread_id {
            return Err(context_error(
                "modelContextConflict",
                "model context intent or target does not match",
            ));
        }
        if let Some(cwd) = cwd {
            let cwd = AbsolutePathBuf::relative_to_current_dir(cwd)
                .map_err(|_| invalid_params("invalid cwd"))?;
            if cwd != pending.config.cwd {
                return Err(context_error(
                    "modelContextConflict",
                    "model context working directory does not match",
                ));
            }
        }
        if intent == ModelContextIntent::Resume
            && let Some(id) = thread_id
        {
            validate_loaded_resume_context(self.thread_manager.as_ref(), id, &pending, model)
                .await?;
        }
        if pending.config.model_provider.env_key.is_some() {
            pending.config.model_provider.api_key().map_err(|_| {
                context_error(
                    "modelProviderUnavailable",
                    "provider credential is unavailable in the app-server environment",
                )
            })?;
        }
        self.config_manager
            .check_thread_model_provider(&pending.config)
            .await
            .map_err(|error| config_load_error(&error))?;
        let selected_model = model.or(pending.config.model.as_deref());
        crate::model_context::validate_model_selection(&pending.config, selected_model, None)
            .await?;
        // Reads used for discovery never consume a handle. Attachment consumes it atomically
        // after validation, so overlapping lifecycle requests cannot both create a thread.
        self.config_manager
            .model_contexts
            .take(
                request.connection_id,
                id,
                self.model_context_owner_generation(),
            )
            .await
            .map(Some)
    }
}

/// Call again under the resume permit: another request can load the target after lookup.
pub(super) async fn validate_loaded_resume_context(
    thread_manager: &ThreadManager,
    thread_id: &str,
    pending: &PendingModelContext,
    model: Option<&str>,
) -> Result<(), JSONRPCErrorError> {
    if let Ok(id) = ThreadId::from_string(thread_id)
        && let Ok(thread) = thread_manager.get_thread(id).await
    {
        let loaded = thread.config().await;
        let changed = loaded.model_provider_id != pending.config.model_provider_id
            || loaded.model_provider != pending.config.model_provider
            || loaded.model_catalog != pending.config.model_catalog
            || loaded
                .resolved_model_context
                .as_ref()
                .is_some_and(|loaded| {
                    pending
                        .config
                        .resolved_model_context
                        .as_ref()
                        .is_none_or(|pending| loaded != pending)
                });
        let selected_model = model.or(pending.config.model.as_deref());
        if changed
            || thread.config_snapshot().await.model.as_str() != selected_model.unwrap_or_default()
        {
            return Err(context_error(
                "modelContextConflict",
                "loaded thread retains its existing model context",
            ));
        }
    }
    Ok(())
}

pub(super) fn apply_pending_model_context(
    overrides: &mut ConfigOverrides,
    context: Option<&PendingModelContext>,
) {
    if let Some(context) = context {
        overrides.model = overrides
            .model
            .clone()
            .or_else(|| context.config.model.clone());
        overrides.model_provider = Some(context.config.model_provider_id.clone());
        overrides.cwd = Some(context.config.cwd.to_path_buf());
        overrides.model_context = context.config.resolved_model_context.clone();
    }
}

/// Recheck the resolved owner after configuration/environment initialization and again before
/// publishing a new runtime. Do not cancel core startup halfway through its store operations.
pub(super) async fn attach_model_context(
    config_manager: &ConfigManager,
    pending: Option<&PendingModelContext>,
    thread_manager: &ThreadManager,
    create: std::pin::Pin<
        Box<dyn std::future::Future<Output = CodexResult<NewThread>> + Send + '_>,
    >,
) -> Result<CodexResult<NewThread>, JSONRPCErrorError> {
    let validate = async || {
        if let Some(pending) = pending {
            config_manager
                .model_contexts
                .validate_attachment(pending, || {
                    thread_manager
                        .auth_manager()
                        .auth_change_state_receiver()
                        .borrow()
                        .owner_generation
                })
                .await?;
        }
        Ok::<_, JSONRPCErrorError>(())
    };
    validate().await?;
    let created = create.await;
    if let Err(error) = validate().await {
        if let Ok(created) = created {
            let _ =
                tokio::time::timeout(Duration::from_secs(10), created.thread.shutdown_and_wait())
                    .await;
            thread_manager
                .remove_thread_if_matches(&created.thread_id, &created.thread)
                .await;
        }
        return Err(error);
    }
    Ok(created)
}

pub(super) fn restore_persisted_model_context(
    overrides: &mut ConfigOverrides,
    history: &[RolloutItem],
    owner: ThreadId,
    codex_home: &Path,
    request: Option<&HashMap<String, serde_json::Value>>,
) -> Result<(), JSONRPCErrorError> {
    if overrides.model_context.is_some() {
        return Ok(());
    }
    let Some(settings) = history.iter().rev().find_map(|item| match item {
        RolloutItem::EventMsg(EventMsg::ThreadSettingsApplied(event))
            if event.thread_id == Some(owner) && event.thread_settings.model_context.is_some() =>
        {
            Some(&event.thread_settings)
        }
        _ => None,
    }) else {
        return Ok(());
    };
    if request.is_some_and(|request| {
        request
            .keys()
            .any(|key| key == "model_providers" || key.starts_with("model_providers."))
    }) {
        return Err(context_error(
            "modelContextConflict",
            "saved model contexts use host-owned provider definitions",
        ));
    }
    let snapshot = settings.model_context.as_ref().ok_or_else(|| {
        context_error(
            "modelContextUnavailable",
            "saved model context is unavailable",
        )
    })?;
    let requested_provider = overrides.model_provider.as_deref().or_else(|| {
        request
            .and_then(|request| request.get("model_provider"))
            .and_then(serde_json::Value::as_str)
    });
    if requested_provider.is_some_and(|provider| provider != snapshot.provider_id) {
        return Err(context_error(
            "modelContextConflict",
            "resolve a model context to replace the saved provider",
        ));
    }
    let catalog = codex_core::model_context::restore_catalog_snapshot(codex_home, snapshot)
        .map_err(|_| {
            context_error(
                "modelCatalogInvalid",
                "saved catalog object is unavailable or corrupt",
            )
        })?;
    let model = overrides
        .model
        .as_deref()
        .or_else(|| {
            request
                .and_then(|request| request.get("model"))
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or(&settings.model)
        .to_string();
    if catalog
        .as_ref()
        .is_some_and(|catalog| !catalog.models.iter().any(|entry| entry.slug == model))
    {
        return Err(context_error(
            "modelSelectionUnavailable",
            "requested model is absent from the saved catalog",
        ));
    }
    overrides.model_provider = Some(snapshot.provider_id.clone());
    overrides.model = Some(model);
    overrides.model_catalog_snapshot = Some(catalog);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cold_resume_context_is_rechecked_when_another_request_loads_the_target()
    -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;
        std::fs::write(
            home.path().join("config.toml"),
            r#"
model_provider = "fixture"
[features]
code_mode = false
code_mode_host = false
[model_providers.fixture]
name = "Fixture"
base_url = "http://127.0.0.1:9/v1"
wire_api = "responses"
"#,
        )?;
        let config_manager =
            ConfigManager::without_managed_config_for_tests(home.path().to_path_buf());
        let mut config = config_manager
            .load_latest_config(Some(home.path().to_path_buf()))
            .await?;
        let threads = codex_core::test_support::thread_manager_with_models_provider_and_home(
            codex_login::CodexAuth::from_api_key("fixture-key"),
            config.model_provider.clone(),
            home.path().to_path_buf(),
            Arc::new(codex_exec_server::EnvironmentManager::default_for_tests()),
        );
        let auth = threads.auth_manager();
        let mut catalog = codex_models_manager::bundled_models_response()?;
        catalog.models.truncate(1);
        catalog.models[0].context_window = Some(32_000);
        config.model = Some(catalog.models[0].slug.clone());
        config.model_catalog = Some(catalog.clone());
        config.resolved_model_context =
            Some(ResolvedModelContext::resolve(&config, auth.clone(), None)?);
        let mut replacement = config.clone();
        catalog.models[0].context_window = Some(96_000);
        replacement.model_catalog = Some(catalog);
        replacement.resolved_model_context = Some(ResolvedModelContext::resolve(
            &replacement,
            auth.clone(),
            None,
        )?);
        let target = threads.reserve_thread_id();
        let connection = ConnectionId(1);
        config_manager.model_contexts.connect(connection).await;
        let generation = auth.auth_change_state_receiver().borrow().owner_generation;
        let mut pending = Vec::new();
        for config in [config.clone(), replacement] {
            let id = config_manager
                .model_contexts
                .insert(
                    connection,
                    Arc::new(config),
                    ModelContextIntent::Resume,
                    Some(target.to_string()),
                    generation,
                )
                .await
                .expect("handle");
            let context = config_manager
                .model_contexts
                .take(connection, &id, generation)
                .await
                .expect("consume while cold");
            validate_loaded_resume_context(&threads, &target.to_string(), &context, None)
                .await
                .expect("cold target");
            pending.push(context);
        }
        let mut options = StartThreadOptions::new(config);
        options.reserved_thread_id = Some(target);
        let created = threads.start_thread(options).await?;
        validate_loaded_resume_context(&threads, &target.to_string(), &pending[0], None)
            .await
            .expect("same retained context");
        let error =
            validate_loaded_resume_context(&threads, &target.to_string(), &pending[1], None)
                .await
                .expect_err("another request loaded a different context");
        assert_eq!(
            error.data.expect("reason")["reason"],
            "modelContextConflict"
        );
        created.thread.shutdown_and_wait().await?;
        Ok(())
    }

    #[tokio::test]
    async fn attachment_rechecks_owner_before_creation_and_removes_a_revoked_runtime()
    -> anyhow::Result<()> {
        for change_before_creation in [true, false] {
            let home = tempfile::tempdir()?;
            std::fs::write(
                home.path().join("config.toml"),
                r#"
model_provider = "fixture"
[features]
code_mode = false
code_mode_host = false
[model_providers.fixture]
name = "Fixture"
base_url = "http://127.0.0.1:9/v1"
wire_api = "responses"
"#,
            )?;
            let config_manager =
                ConfigManager::without_managed_config_for_tests(home.path().to_path_buf());
            let config = config_manager
                .load_latest_config(Some(home.path().to_path_buf()))
                .await?;
            let threads = codex_core::test_support::thread_manager_with_models_provider_and_home(
                codex_login::CodexAuth::from_api_key("fixture-key"),
                config.model_provider.clone(),
                home.path().to_path_buf(),
                Arc::new(codex_exec_server::EnvironmentManager::default_for_tests()),
            );
            let auth = threads.auth_manager();
            let generation = auth.auth_change_state_receiver().borrow().owner_generation;
            let connection = ConnectionId(1);
            config_manager.model_contexts.connect(connection).await;
            let id = config_manager
                .model_contexts
                .insert(
                    connection,
                    Arc::new(config.clone()),
                    ModelContextIntent::Start,
                    None,
                    generation,
                )
                .await
                .expect("handle");
            let pending = config_manager
                .model_contexts
                .take(connection, &id, generation)
                .await
                .expect("consume");
            if change_before_creation {
                auth.logout().await?;
            }
            let mut created_id = None;
            let result = attach_model_context(
                &config_manager,
                Some(&pending),
                &threads,
                Box::pin(async {
                    assert!(
                        !change_before_creation,
                        "revoked attachment must not poll creation"
                    );
                    let created = threads
                        .start_thread(StartThreadOptions::new(config))
                        .await?;
                    created_id = Some(created.thread_id);
                    auth.logout()
                        .await
                        .expect("change authentication owner during creation");
                    Ok(created)
                }),
            )
            .await;
            let error = result.err().expect("owner transition rejects attachment");
            assert_eq!(error.data.expect("reason")["reason"], "modelContextExpired");
            if let Some(id) = created_id {
                assert!(
                    threads.get_thread(id).await.is_err(),
                    "revoked runtime must be removed"
                );
            }
        }
        Ok(())
    }
}

//! Prepares child configuration from captured step settings and requested overrides.
//!
//! Spawn and reload share live runtime policy; role and model precedence, full-history
//! inheritance, and validation messages remain the same for each multi-agent version.

use crate::agent::role::DEFAULT_ROLE_NAME;
use crate::agent::role::apply_role_to_config;
use crate::config::Config;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnContext;
use codex_features::Feature;
use codex_models_manager::manager::RefreshStrategy;
use codex_models_manager::manager::SharedModelsManager;
use codex_protocol::config_types::SERVICE_TIER_DEFAULT_REQUEST_VALUE;
use codex_protocol::models::BaseInstructions;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::protocol::MultiAgentVersion;

pub(crate) const MAX_SPAWN_AGENT_MODEL_OVERRIDES: usize = 5;

pub(crate) fn model_supports_multi_agent_backend(
    model: &ModelPreset,
    multi_agent_version: MultiAgentVersion,
) -> bool {
    multi_agent_version != MultiAgentVersion::V2
        || model.multi_agent_version != Some(MultiAgentVersion::Disabled)
}

/// Selects the existing spawn tool's role-inheritance rules.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpawnConfigVersion {
    V1,
    V2,
}

pub(crate) struct SpawnConfigOptions<'a> {
    pub(crate) version: SpawnConfigVersion,
    pub(crate) full_history_fork: bool,
    pub(crate) role_name: Option<&'a str>,
    pub(crate) model: Option<&'a str>,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
}

pub(crate) struct PreparedSpawnConfig {
    pub(crate) config: Config,
    pub(crate) role_name: Option<String>,
}

/// Resolves child settings before starting the thread, retaining the invoking tool's precedence.
pub(crate) async fn prepare_agent_spawn_config(
    session: &Session,
    step_context: &StepContext,
    options: SpawnConfigOptions<'_>,
) -> Result<PreparedSpawnConfig, String> {
    let turn = step_context.turn.as_ref();
    let mut config =
        build_agent_spawn_config(&session.get_base_instructions().await, step_context)?;
    if options.version == SpawnConfigVersion::V1 && options.full_history_fork {
        reject_full_fork_agent_type_override(options.role_name)?;
    }
    apply_requested_spawn_agent_model_overrides(
        session,
        step_context,
        &mut config,
        options.model,
        options.reasoning_effort,
    )
    .await?;
    if !options.full_history_fork
        || (options.version == SpawnConfigVersion::V2 && options.role_name.is_some())
    {
        apply_spawn_agent_role(session, &mut config, options.role_name).await?;
        if options.version == SpawnConfigVersion::V2
            && options.full_history_fork
            && config.developer_instructions.is_none()
        {
            config
                .developer_instructions
                .clone_from(&turn.developer_instructions);
        }
    }
    apply_spawn_agent_service_tier(session, &mut config).await?;
    apply_spawn_agent_runtime_overrides(&mut config, turn)?;

    // Remember an applied configured default so cold reload reapplies its restrictions.
    let role_name = options
        .role_name
        .or_else(|| {
            (options.version == SpawnConfigVersion::V2
                && !options.full_history_fork
                && config
                    .agent_roles
                    .get(DEFAULT_ROLE_NAME)
                    .is_some_and(|role| role.config_file.is_some()))
            .then_some(DEFAULT_ROLE_NAME)
        })
        .map(str::to_owned);
    Ok(PreparedSpawnConfig { config, role_name })
}

/// Builds the base config snapshot for a newly spawned sub-agent.
///
/// The returned config starts from the parent's effective config and then refreshes the
/// model selection and reasoning settings captured for the invoking step, plus the turn's
/// runtime approval policy, sandbox, and cwd. Role-specific overrides are layered
/// after this step; skipping this helper and cloning stale config state directly can send the child
/// agent out with the wrong provider or runtime policy.
pub(crate) fn build_agent_spawn_config(
    base_instructions: &BaseInstructions,
    step_context: &StepContext,
) -> Result<Config, String> {
    let mut config = build_agent_shared_config(step_context.turn.as_ref())?;
    let settings = &step_context.settings;
    config.model = Some(settings.model_info.slug.clone());
    config.model_reasoning_effort = settings.effective_reasoning_effort();
    config.model_reasoning_summary = Some(settings.reasoning_summary);
    config.base_instructions = Some(base_instructions.text.clone());
    config.base_instructions_provenance = base_instructions.provenance.clone();
    Ok(config)
}

pub(crate) fn build_agent_resume_config(turn: &TurnContext) -> Result<Config, String> {
    let mut config = build_agent_shared_config(turn)?;
    if turn.multi_agent_version == MultiAgentVersion::V2 {
        config.multi_agent_v2.task_payload_locked = true;
    }
    // For resume, keep base instructions sourced from rollout/session metadata.
    config.base_instructions = None;
    config.base_instructions_provenance = None;
    Ok(config)
}

fn build_agent_shared_config(turn: &TurnContext) -> Result<Config, String> {
    let base_config = turn.config.clone();
    let mut config = (*base_config).clone();
    if config
        .features
        .enabled(Feature::SubagentDefaultContextLimits)
    {
        config.model_context_window = None;
        config.model_auto_compact_token_limit = None;
    }
    // Preserve activation for history forks without freezing the parent's model-owned prompts.
    // Fresh child startup restores configured preferences from the retained snapshot.
    config.token_budget = turn.configured_token_budget.clone();
    config.model = Some(turn.model_info().slug.clone());
    config.model_provider = turn.provider.info().clone();
    config.model_reasoning_effort = turn
        .reasoning_effort()
        .or(turn.model_info().default_reasoning_level.as_ref())
        .cloned();
    config.model_reasoning_summary = Some(turn.reasoning_summary());
    config.developer_instructions = turn.developer_instructions.clone();
    if turn.multi_agent_version == MultiAgentVersion::V2
        && let Some(developer_instructions) = turn
            .config
            .multi_agent_v2
            .subagent_developer_instructions
            .clone()
    {
        config.developer_instructions = Some(developer_instructions);
    }
    apply_spawn_agent_runtime_overrides(&mut config, turn)?;

    Ok(config)
}

fn reject_full_fork_agent_type_override(agent_type: Option<&str>) -> Result<(), String> {
    if agent_type.is_some() {
        return Err(
            "Full-history forked agents inherit the parent agent type; omit agent_type, or spawn without a full-history fork.".to_string());
    }
    Ok(())
}

/// Copies runtime-only turn state onto a child config before it is handed to `AgentControl`.
///
/// These values are chosen by the live turn rather than persisted config, so leaving them stale can
/// make a child agent disagree with its parent about approval policy, cwd, or sandboxing.
fn apply_spawn_agent_runtime_overrides(
    config: &mut Config,
    turn: &TurnContext,
) -> Result<(), String> {
    config
        .permissions
        .approval_policy
        .set(turn.approval_policy())
        .map_err(|err| format!("approval_policy is invalid: {err}"))?;
    config.approvals_reviewer = turn.config.approvals_reviewer;
    #[allow(deprecated)]
    let turn_cwd = turn.cwd.clone();
    config.cwd = turn_cwd;
    config
        .permissions
        .set_permission_profile_from_session_snapshot(
            turn.config
                .permissions
                .permission_profile_state()
                .snapshot(),
        )
        .map_err(|err| format!("permission_profile is invalid: {err}"))?;
    Ok(())
}

async fn apply_requested_spawn_agent_model_overrides(
    session: &Session,
    step_context: &StepContext,
    config: &mut Config,
    requested_model: Option<&str>,
    requested_reasoning_effort: Option<ReasoningEffort>,
) -> Result<(), String> {
    let turn = step_context.turn.as_ref();
    let requested_model = requested_model.or(turn.config.agent_default_subagent_model.as_deref());
    let requested_reasoning_effort = requested_reasoning_effort
        .or_else(|| turn.config.agent_default_subagent_reasoning_effort.clone());
    if requested_model.is_none() && requested_reasoning_effort.is_none() {
        return Ok(());
    }

    if let Some(requested_model) = requested_model {
        let available_models = session
            .services
            .models_manager
            .list_models(RefreshStrategy::Offline, config.http_client_factory())
            .await;
        let selected_model_name = find_spawn_agent_model_name(
            &available_models,
            requested_model,
            turn.multi_agent_version,
        )?;
        let selected_model_info = session
            .services
            .models_manager
            .get_model_info(&selected_model_name, &config.to_models_manager_config())
            .await;

        config.model = Some(selected_model_name.clone());
        if let Some(reasoning_effort) = requested_reasoning_effort {
            validate_spawn_agent_reasoning_effort(
                &selected_model_name,
                &selected_model_info.supported_reasoning_levels,
                &reasoning_effort,
            )?;
            config.model_reasoning_effort = Some(reasoning_effort);
        } else {
            config.model_reasoning_effort = selected_model_info.default_reasoning_level;
        }

        return Ok(());
    }

    if let Some(reasoning_effort) = requested_reasoning_effort {
        validate_spawn_agent_reasoning_effort(
            &step_context.settings.model_info.slug,
            &step_context.settings.model_info.supported_reasoning_levels,
            &reasoning_effort,
        )?;
        config.model_reasoning_effort = Some(reasoning_effort);
    }

    Ok(())
}

pub(crate) async fn apply_spawn_agent_service_tier(
    session: &Session,
    config: &mut Config,
) -> Result<(), String> {
    let Some(service_tier) = session.services.agent_control.service_tier() else {
        config.service_tier = None;
        return Ok(());
    };
    if service_tier == SERVICE_TIER_DEFAULT_REQUEST_VALUE {
        config.service_tier = Some(service_tier);
        return Ok(());
    }

    let models_manager = resolve_child_models_manager(session, config).await?;
    let model = config.model.clone().ok_or_else(|| {
        "spawn_agent could not resolve the child model for service tier validation".to_string()
    })?;
    let model_info = models_manager
        .get_model_info(model.as_str(), &config.to_models_manager_config())
        .await;

    config.service_tier = model_info
        .supports_service_tier(service_tier.as_str())
        .then_some(service_tier);
    Ok(())
}

async fn apply_spawn_agent_role(
    session: &Session,
    config: &mut Config,
    role_name: Option<&str>,
) -> Result<(), String> {
    let previous_model = config.model.clone();
    let previous_reasoning_effort = config.model_reasoning_effort.clone();
    apply_role_to_config(config, role_name).await?;
    let context_changed = config
        .resolved_model_context
        .as_ref()
        .is_some_and(|context| !context.matches_config(config));
    let models_manager = resolve_child_models_manager(session, config).await?;
    if !context_changed
        && config.model == previous_model
        && config.model_reasoning_effort == previous_reasoning_effort
    {
        return Ok(());
    }

    let Some(reasoning_effort) = config.model_reasoning_effort.clone() else {
        return Ok(());
    };
    let model = config.model.clone().ok_or_else(|| {
        "spawn_agent could not resolve the child model for reasoning effort validation".to_string()
    })?;
    let model_info = models_manager
        .get_model_info(&model, &config.to_models_manager_config())
        .await;
    if model_info.used_fallback_model_metadata {
        return Ok(());
    }

    validate_spawn_agent_reasoning_effort(
        &model,
        &model_info.supported_reasoning_levels,
        &reasoning_effort,
    )
}

/// Select a role's default and metadata from its own provider before validating child settings.
async fn resolve_child_models_manager(
    session: &Session,
    config: &mut Config,
) -> Result<SharedModelsManager, String> {
    let Some(context) = config.resolved_model_context.clone() else {
        return Ok(session.services.models_manager.clone());
    };
    crate::model_context::validate_managed_catalog(config, config.model_catalog.as_ref())
        .map_err(|error| error.to_string())?;
    let context = if context.matches_config(config) {
        context
    } else {
        crate::model_context::ResolvedModelContext::resolve(
            config,
            session.services.auth_manager.clone(),
            None,
        )
        .map_err(|error| error.to_string())?
    };
    context
        .validate_static_model_selection(config.model.as_deref())
        .map_err(|error| error.to_string())?;
    let manager = context.models_manager.clone();
    if config.model_catalog.is_none() && config.model_provider.model_catalog_url.is_some() {
        let models = manager
            .list_models(
                RefreshStrategy::OnlineIfUncached,
                config.http_client_factory(),
            )
            .await;
        if models.is_empty()
            || config
                .model
                .as_ref()
                .is_some_and(|selected| !models.iter().any(|model| model.model == *selected))
        {
            return Err("selected child model is unavailable in the provider catalog".to_string());
        }
    }
    config.model = Some(
        manager
            .get_default_model(
                &config.model,
                false,
                RefreshStrategy::Offline,
                config.http_client_factory(),
            )
            .await,
    );
    config.resolved_model_context = Some(context);
    Ok(manager)
}

fn find_spawn_agent_model_name(
    available_models: &[ModelPreset],
    requested_model: &str,
    multi_agent_version: MultiAgentVersion,
) -> Result<String, String> {
    available_models
        .iter()
        .find(|model| {
            model.model == requested_model
                && model_supports_multi_agent_backend(model, multi_agent_version)
        })
        .map(|model| model.model.clone())
        .ok_or_else(|| {
            let available = available_models
                .iter()
                .filter(|model| model.show_in_picker)
                .filter(|model| model_supports_multi_agent_backend(model, multi_agent_version))
                .take(MAX_SPAWN_AGENT_MODEL_OVERRIDES)
                .map(|model| model.model.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "Unknown model `{requested_model}` for spawn_agent. Available models: {available}"
            )
        })
}

fn validate_spawn_agent_reasoning_effort(
    model: &str,
    supported_reasoning_levels: &[ReasoningEffortPreset],
    requested_reasoning_effort: &ReasoningEffort,
) -> Result<(), String> {
    if supported_reasoning_levels
        .iter()
        .any(|preset| &preset.effort == requested_reasoning_effort)
    {
        return Ok(());
    }

    let supported = supported_reasoning_levels
        .iter()
        .map(|preset| preset.effort.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "Reasoning effort `{requested_reasoning_effort}` is not supported for model `{model}`. Supported reasoning efforts: {supported}"
    ))
}

#[cfg(test)]
mod mixed_context_tests {
    use super::*;
    use crate::config::AgentRoleConfig;
    use codex_protocol::openai_models::ModelVisibility;
    use codex_protocol::openai_models::ModelsResponse;

    #[tokio::test]
    async fn same_provider_model_only_role_cannot_escape_static_catalog() {
        let (session, turn) = crate::session::tests::make_session_and_context().await;
        let home = tempfile::tempdir().expect("home");
        let mut config = (*turn.config).clone();
        let model = codex_models_manager::bundled_models_response()
            .expect("catalog")
            .models
            .into_iter()
            .find(|model| model.slug == "gpt-5.5")
            .expect("model");
        config.model = Some(model.slug.clone());
        config.model_catalog = Some(ModelsResponse {
            models: vec![model],
        });
        config.resolved_model_context = Some(
            crate::model_context::ResolvedModelContext::resolve(
                &config,
                session.services.auth_manager.clone(),
                None,
            )
            .expect("parent context"),
        );
        let path = home.path().join("model-only.toml");
        std::fs::write(&path, "model = 'not-in-catalog'\n").expect("role");
        config.agent_roles.insert(
            "model-only".into(),
            AgentRoleConfig {
                config_file: Some(path.clone()),
                ..Default::default()
            },
        );
        let mut invalid = config.clone();
        let error = apply_spawn_agent_role(&session, &mut invalid, Some("model-only"))
            .await
            .expect_err("reusing a context must still validate model membership");
        assert!(
            error.contains("absent from the static model catalog"),
            "{error}"
        );
        std::fs::write(path, "model = 'gpt-5.5'\n").expect("supported role");
        let original = config.resolved_model_context.clone().expect("context");
        apply_spawn_agent_role(&session, &mut config, Some("model-only"))
            .await
            .expect("supported same-provider model");
        assert!(std::sync::Arc::ptr_eq(
            &original,
            config
                .resolved_model_context
                .as_ref()
                .expect("context retained")
        ));
    }

    #[test_case::test_case(false; "native_default")]
    #[test_case::test_case(true; "static_default_and_child_metadata")]
    #[tokio::test]
    async fn provider_only_role_resolves_child_default_before_settings_validation(
        static_catalog: bool,
    ) {
        let (mut session, turn) = crate::session::tests::make_session_and_context().await;
        let home = tempfile::tempdir().expect("temporary home");
        let mut config = (*turn.config).clone();
        let mut model = codex_models_manager::bundled_models_response()
            .expect("bundled catalog")
            .models
            .into_iter()
            .find(|model| model.slug == "gpt-5.5")
            .expect("parent model");
        model.visibility = ModelVisibility::List;
        model.supported_reasoning_levels = vec![ReasoningEffortPreset {
            effort: ReasoningEffort::Low,
            description: "parent low".into(),
        }];
        config.model = Some(model.slug.clone());
        config.model_reasoning_effort = Some(ReasoningEffort::Low);
        config.model_catalog = Some(ModelsResponse {
            models: vec![model.clone()],
        });
        let parent = crate::model_context::ResolvedModelContext::resolve(
            &config,
            session.services.auth_manager.clone(),
            None,
        )
        .expect("parent context");
        session.services.models_manager = parent.models_manager.clone();
        config.resolved_model_context = Some(parent);
        let role_path = home.path().join("child.toml");
        let catalog_path = home.path().join("child-models.json");
        let catalog_setting = if static_catalog {
            model.context_window = Some(96_000);
            model.service_tiers.clear();
            model.additional_speed_tiers.clear();
            model.default_service_tier = None;
            model.supported_reasoning_levels = vec![ReasoningEffortPreset {
                effort: ReasoningEffort::High,
                description: "child high".into(),
            }];
            std::fs::write(
                &catalog_path,
                serde_json::to_vec(&ModelsResponse {
                    models: vec![model.clone()],
                })
                .expect("catalog JSON"),
            )
            .expect("child catalog");
            format!(
                "model_catalog_json = {:?}\n",
                catalog_path.display().to_string()
            )
        } else {
            String::new()
        };
        std::fs::write(
            &role_path,
            format!(
                "model_provider = 'ollama'\nmodel_reasoning_effort = 'high'\n{catalog_setting}"
            ),
        )
        .expect("role");
        config.agent_roles.insert(
            "provider-only".into(),
            AgentRoleConfig {
                config_file: Some(role_path),
                ..Default::default()
            },
        );
        let parent_config = config.clone();
        apply_spawn_agent_role(&session, &mut config, Some("provider-only"))
            .await
            .expect("choose target provider's default before validating inherited effort");
        assert_eq!(config.model_provider_id, "ollama");
        assert!(config.model.is_some());
        assert_eq!(config.model_reasoning_effort, Some(ReasoningEffort::High));
        let context = config
            .resolved_model_context
            .as_ref()
            .expect("child context");
        assert!(context.matches_config(&config));
        if static_catalog {
            assert_eq!(config.model.as_deref(), Some("gpt-5.5"));
            assert_eq!(
                context
                    .snapshot
                    .catalog
                    .as_ref()
                    .expect("static snapshot")
                    .models[0]
                    .context_window,
                Some(96_000)
            );
            session.services.agent_control.propagate_config_update(
                crate::agent::api::AgentConfigUpdate::ServiceTier(Some("fast".into())),
            );
            config.model = None;
            apply_spawn_agent_service_tier(&session, &mut config)
                .await
                .expect("default precedes tier validation");
            assert_eq!(config.model.as_deref(), Some("gpt-5.5"));
            assert_eq!(
                config.service_tier, None,
                "child metadata does not support the parent's fast tier"
            );
            model.supported_reasoning_levels[0].effort = ReasoningEffort::Low;
            std::fs::write(
                catalog_path,
                serde_json::to_vec(&ModelsResponse {
                    models: vec![model],
                })
                .expect("catalog JSON"),
            )
            .expect("restrict child effort");
            let mut unsupported = parent_config;
            let error = apply_spawn_agent_role(&session, &mut unsupported, Some("provider-only"))
                .await
                .expect_err("validate against the new child's effort levels");
            assert!(error.contains("not supported"), "{error}");
        }
    }
}

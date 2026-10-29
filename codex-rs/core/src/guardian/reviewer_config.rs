//! Resolves reviewer models and adds policy context and live network state to reviewer configuration.
//! Both prewarming and reviews finish this setup before context preparation and reuse checks.

use std::sync::Arc;

use codex_features::Feature;
use codex_guardian_reviewer::guardian_output_contract_prompt;
use codex_model_provider::approval_review_uses_parent_model;
use codex_prompts::GuardianPolicyInstructions;
use codex_prompts::ResolvedModelMessages;
use codex_protocol::models::BaseInstructionsProvenance;

use crate::config::Config;
use crate::config::NetworkProxySpec;
use crate::context::ContextualUserFragment;
use crate::context::GuardianConversationHistory;

/// Adds the captured model, policy prompt and live network rules before reuse selection.
pub fn build_guardian_review_session_config(
    mut guardian_config: Config,
    live_network_config: Option<codex_network_proxy::NetworkProxyConfig>,
    active_model: &str,
    reasoning_effort: Option<codex_protocol::openai_models::ReasoningEffort>,
    reasoning_summary: codex_protocol::config_types::ReasoningSummary,
    personality: Option<codex_protocol::config_types::Personality>,
    model_messages: ResolvedModelMessages<'_>,
) -> anyhow::Result<Config> {
    guardian_config.model = Some(active_model.to_owned());
    guardian_config.model_reasoning_effort = reasoning_effort;
    guardian_config.model_reasoning_summary = Some(reasoning_summary);
    guardian_config.personality = personality;
    let auto_review = model_messages.auto_review();
    let tenant_policy_config = guardian_config.resolve_guardian_policy(model_messages);
    let extra_policy = guardian_config
        .guardian_extra_policy
        .as_deref()
        .unwrap_or_default();
    let policy_template = guardian_config
        .guardian_policy_template
        .as_deref()
        .unwrap_or(auto_review.policy_template);
    let mut instructions = GuardianPolicyInstructions::new(
        guardian_config.guardian_transcript_mode,
        tenant_policy_config,
        extra_policy,
        policy_template,
        guardian_output_contract_prompt(),
    )
    .render()
    .into_text()
    .unwrap_or_default();
    if guardian_config
        .features
        .enabled(Feature::GuardianConversationHistoryTools)
        && guardian_config.features.enabled(Feature::Apps)
    {
        instructions.push('\n');
        instructions.push_str(
            &GuardianConversationHistory {
                prompt: guardian_config
                    .guardian_conversation_history_prompt
                    .as_deref(),
            }
            .render()
            .into_text()
            .unwrap_or_default(),
        );
    }
    guardian_config.base_instructions = Some(instructions);
    guardian_config.base_instructions_provenance = Some(BaseInstructionsProvenance::Custom);
    if let Some(live_network_config) = live_network_config
        && guardian_config.permissions.network.is_some()
    {
        let network_constraints = guardian_config
            .config_layer_stack
            .requirements()
            .network
            .as_ref()
            .map(|network| network.value.clone());
        guardian_config.permissions.network = Some(NetworkProxySpec::from_config_and_constraints(
            live_network_config,
            network_constraints,
            guardian_config.permissions.permission_profile(),
        )?);
    }
    Ok(guardian_config)
}

/// Resolves the same catalog-backed reviewer for approval and checkpoint migration.
pub(crate) async fn resolve_review_model(
    session: &crate::session::session::Session,
    context: &super::GuardianReviewContext,
) -> (
    codex_guardian_reviewer::ReviewModel,
    Arc<codex_protocol::openai_models::ModelInfo>,
) {
    let turn = context.turn();
    if approval_review_uses_parent_model(&turn.config.model_provider_id) {
        return (
            codex_guardian_reviewer::select_parent_review_model(
                &context.model_info,
                context.reasoning_effort.as_ref(),
            ),
            Arc::clone(&context.model_info),
        );
    }
    let available_models = session
        .services
        .models_manager
        .list_models(
            codex_models_manager::manager::RefreshStrategy::Offline,
            turn.config.http_client_factory(),
        )
        .await;
    let default_review_model_id = turn.provider.approval_review_preferred_model();
    let review_model = codex_guardian_reviewer::select_review_model(
        &context.model_info,
        context.reasoning_effort.as_ref(),
        default_review_model_id,
        &available_models,
    );
    // Resolve a separate reviewer against the current catalog on every attempt.
    // Parent fallback must retain the action's metadata even after a catalog refresh.
    let guardian_model_info =
        if !review_model.catalog_contains_auto_review && !review_model.model_overridden {
            Arc::clone(&context.model_info)
        } else {
            Arc::new(
                session
                    .services
                    .models_manager
                    .get_model_info(
                        review_model.model.as_str(),
                        &turn.config.to_models_manager_config(),
                    )
                    .await,
            )
        };
    (review_model, guardian_model_info)
}

/// Legacy sessions may retain their provider and model without retaining their
/// catalog. Do not construct a custom-provider reviewer from OpenAI fallback data.
pub(super) fn ensure_provider_review_catalog(
    config: &Config,
    parent_model: &codex_protocol::openai_models::ModelInfo,
    models_manager: &codex_models_manager::manager::SharedModelsManager,
) -> anyhow::Result<()> {
    if !approval_review_uses_parent_model(&config.model_provider_id) {
        return Ok(());
    }
    let has_model = config.model_catalog.as_ref().map_or_else(
        || config.model_provider.model_catalog_url.is_some(),
        |catalog| {
            catalog
                .models
                .iter()
                .any(|model| model.slug == parent_model.slug)
        },
    );
    let has_runtime_binding = config
        .resolved_model_context
        .as_ref()
        .is_some_and(|context| {
            context.matches_config(config) && Arc::ptr_eq(&context.models_manager, models_manager)
        });
    anyhow::ensure!(
        has_runtime_binding && has_model && !parent_model.used_fallback_model_metadata,
        "Automatic approval review for provider `{}` requires its own model catalog containing `{}`, bound to the runtime model manager. \
         Use `modelContext/resolve` and attach its `modelContextId`. With the current CLI, unload \
         the thread and resume with an explicit `model_catalog_json`, or configure the provider's \
         `model_catalog_url` before resolving. The bundled OpenAI catalog cannot be used for this reviewer.",
        config.model_provider_id,
        parent_model.slug,
    );
    Ok(())
}

//! First-thread model, catalog, and approval choices come from invocation inputs.
//! Profile defaults are resolved by the embedded server; explicit invocation choices still win.

use crate::legacy_core::config::ConfigOverrides;
use codex_app_server_protocol::ApprovalsReviewer;
use codex_app_server_protocol::ThreadStartParams;
use codex_config::LoaderOverrides;

#[derive(Clone, Default)]
pub(crate) struct StartupLaunchChoices {
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<serde_json::Value>,
    pub(crate) provider: Option<String>,
    pub(crate) catalog_json: Option<String>,
    pub(crate) model_context_id: Option<String>,
    selected_profile: bool,
    pub(super) approvals_reviewer: StartupApprovalsReviewer,
}

#[derive(Clone, Copy, Default)]
pub(super) enum StartupApprovalsReviewer {
    #[default]
    Config,
    Invocation(Option<ApprovalsReviewer>),
}

impl StartupLaunchChoices {
    pub(crate) fn from_launch(
        cli_overrides: &[(String, toml::Value)],
        harness_overrides: &ConfigOverrides,
        loader_overrides: &LoaderOverrides,
    ) -> Self {
        let value = |key| {
            cli_overrides
                .iter()
                .rev()
                .find(|(path, _)| path == key)
                .map(|(_, value)| value)
        };
        let selected_profile = loader_overrides.user_config_profile.is_some();
        let reviewer = harness_overrides
            .approvals_reviewer
            .or_else(|| value("approvals_reviewer")?.clone().try_into().ok())
            .map(Into::into);
        Self {
            model: harness_overrides.model.clone().or_else(|| {
                value("model")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
            }),
            effort: value("model_reasoning_effort")
                .and_then(|value| serde_json::to_value(value).ok()),
            provider: harness_overrides.model_provider.clone().or_else(|| {
                value("model_provider")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
            }),
            catalog_json: value("model_catalog_json")
                .and_then(toml::Value::as_str)
                .map(str::to_owned),
            model_context_id: None,
            selected_profile,
            approvals_reviewer: match (selected_profile, reviewer) {
                (true, None) => StartupApprovalsReviewer::Config,
                (_, reviewer) => StartupApprovalsReviewer::Invocation(reviewer),
            },
        }
    }

    pub(crate) fn requests_model_context(&self) -> bool {
        self.provider.is_some() || self.catalog_json.is_some()
    }

    pub(super) fn configure(self, params: &mut ThreadStartParams) {
        if let Some(id) = self.model_context_id {
            params.model_context_id = Some(id);
            params.model = self.model;
            params.model_provider = None;
            if let Some(config) = params.config.as_mut() {
                config.remove("model");
                config.remove("model_provider");
                config.remove("model_catalog_json");
            }
        } else if self.selected_profile {
            params.model_provider = self.provider.or(params.model_provider.take());
            return;
        } else {
            params.model = self.model;
            params.model_provider = self.provider;
        }
        if let Some(overrides) = params.config.as_mut() {
            overrides.remove("model_reasoning_effort");
        }
        if let Some(effort) = self.effort {
            params
                .config
                .get_or_insert_default()
                .insert("model_reasoning_effort".into(), effort);
        }
    }
}

/// One classifier drives daemon eligibility and typed launch-choice extraction.
pub(crate) fn is_model_selection_override(key: &str, value: &toml::Value) -> bool {
    matches!(key, "model" | "model_provider" | "model_catalog_json") && value.is_str()
}

#[cfg(test)]
mod mixed_provider_tests {
    use super::*;

    #[test]
    fn selected_profile_keeps_its_reviewer_without_an_invocation_override() {
        let choices = StartupLaunchChoices::from_launch(
            &[],
            &ConfigOverrides::default(),
            &LoaderOverrides {
                user_config_profile: Some("work".parse().expect("profile")),
                ..Default::default()
            },
        );
        assert!(matches!(
            choices.approvals_reviewer,
            StartupApprovalsReviewer::Config
        ));
    }

    #[test]
    fn profile_and_mixed_provider_choices_keep_explicit_approval_review() {
        let choices = StartupLaunchChoices::from_launch(
            &[
                ("model_provider".into(), toml::Value::String("kimi".into())),
                (
                    "model_catalog_json".into(),
                    toml::Value::String("models.json".into()),
                ),
                (
                    "approvals_reviewer".into(),
                    toml::Value::String("auto_review".into()),
                ),
            ],
            &ConfigOverrides::default(),
            &LoaderOverrides {
                user_config_profile: Some("work".parse().expect("profile")),
                ..Default::default()
            },
        );
        assert!(choices.requests_model_context());
        assert!(choices.selected_profile);
        assert!(matches!(
            choices.approvals_reviewer,
            StartupApprovalsReviewer::Invocation(Some(ApprovalsReviewer::AutoReview))
        ));
    }

    #[test]
    fn selected_profile_keeps_explicit_model_for_resolution_and_attachment() {
        let mut choices = StartupLaunchChoices::from_launch(
            &[("model".into(), toml::Value::String("kv-model".into()))],
            &ConfigOverrides {
                model: Some("flag-model".into()),
                ..Default::default()
            },
            &LoaderOverrides {
                user_config_profile: Some("work".parse().expect("profile")),
                ..Default::default()
            },
        );
        assert_eq!(choices.model.as_deref(), Some("flag-model"));
        choices.model_context_id = Some("profile-context".into());
        let mut params = ThreadStartParams {
            model: Some("profile-default".into()),
            ..Default::default()
        };
        choices.configure(&mut params);
        assert_eq!(params.model.as_deref(), Some("flag-model"));
        assert_eq!(params.model_context_id.as_deref(), Some("profile-context"));
    }

    #[test]
    fn exact_kimi_launch_preserves_typed_choices_and_model_flag_precedence() {
        let overrides = vec![
            ("model_provider".into(), toml::Value::String("kimi".into())),
            (
                "model_catalog_json".into(),
                toml::Value::String("~/.codex/models_kimi.json".into()),
            ),
            (
                "model".into(),
                toml::Value::String("overridden-model".into()),
            ),
        ];
        let choices = StartupLaunchChoices::from_launch(
            &overrides,
            &ConfigOverrides {
                model: Some("k3.1-pro".into()),
                ..Default::default()
            },
            &LoaderOverrides::default(),
        );
        assert_eq!(choices.provider.as_deref(), Some("kimi"));
        assert_eq!(
            choices.catalog_json.as_deref(),
            Some("~/.codex/models_kimi.json")
        );
        assert_eq!(choices.model.as_deref(), Some("k3.1-pro"));
        assert!(choices.requests_model_context());
        assert!(
            overrides
                .iter()
                .all(|(key, value)| is_model_selection_override(key, value))
        );
    }

    #[test]
    fn attaching_a_resolved_context_keeps_explicit_model_and_effort_without_duplicate_route_fields()
    {
        let choices = StartupLaunchChoices {
            model: Some("selected-model".into()),
            effort: Some(serde_json::json!("high")),
            model_context_id: Some("resolved-handle".into()),
            ..Default::default()
        };
        let mut params = ThreadStartParams {
            model: Some("local-model".into()),
            model_provider: Some("local-provider".into()),
            config: Some(std::collections::HashMap::from([
                ("model_catalog_json".into(), serde_json::json!("old.json")),
                ("model_reasoning_effort".into(), serde_json::json!("high")),
            ])),
            ..Default::default()
        };
        choices.configure(&mut params);
        assert_eq!(params.model_context_id.as_deref(), Some("resolved-handle"));
        assert_eq!(params.model.as_deref(), Some("selected-model"));
        assert!(params.model_provider.is_none());
        let config = params.config.expect("settings");
        assert!(!config.contains_key("model_catalog_json"));
        assert_eq!(config["model_reasoning_effort"], "high");
    }
}

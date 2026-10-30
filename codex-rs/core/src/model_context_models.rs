//! Custom provider catalogs do not inherit the default OpenAI account's picker filter.

use codex_http_client::HttpClientFactory;
use codex_login::AuthManager;
use codex_models_manager::CatalogModel;
use codex_models_manager::ModelsManagerConfig;
use codex_models_manager::manager::ModelsManager;
use codex_models_manager::manager::ModelsManagerFuture;
use codex_models_manager::manager::RefreshStrategy;
use codex_models_manager::manager::SharedModelsManager;
use codex_protocol::config_types::CollaborationModeMask;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ModelsResponse;
use std::sync::Arc;
use tokio::sync::TryLockError;

#[derive(Debug)]
pub(super) struct ProviderCatalogModelsManager(pub(super) SharedModelsManager);

impl ModelsManager for ProviderCatalogModelsManager {
    fn for_originator(&self, originator: &str) -> Option<SharedModelsManager> {
        self.0
            .for_originator(originator)
            .map(|manager| Arc::new(Self(manager)) as SharedModelsManager)
    }

    fn set_api_key_model_discovery_enabled(&self, enabled: bool) {
        self.0.set_api_key_model_discovery_enabled(enabled);
    }

    fn raw_model_catalog(
        &self,
        strategy: RefreshStrategy,
        factory: HttpClientFactory,
    ) -> ModelsManagerFuture<'_, ModelsResponse> {
        self.0.raw_model_catalog(strategy, factory)
    }

    fn model_catalog_with_origins(
        &self,
        strategy: RefreshStrategy,
        factory: HttpClientFactory,
    ) -> ModelsManagerFuture<'_, Vec<CatalogModel>> {
        self.0.model_catalog_with_origins(strategy, factory)
    }

    fn refresh_after_auth_change(&self, factory: HttpClientFactory) -> ModelsManagerFuture<'_, ()> {
        self.0.refresh_after_auth_change(factory)
    }

    fn get_remote_models(&self) -> ModelsManagerFuture<'_, Vec<ModelInfo>> {
        self.0.get_remote_models()
    }

    fn try_get_remote_models(&self) -> Result<Vec<ModelInfo>, TryLockError> {
        self.0.try_get_remote_models()
    }

    fn auth_manager(&self) -> Option<&AuthManager> {
        self.0.auth_manager()
    }

    fn build_available_models(&self, mut models: Vec<ModelInfo>) -> Vec<ModelPreset> {
        models.sort_by_key(|model| model.priority);
        let mut presets: Vec<_> = models.into_iter().map(ModelPreset::from).collect();
        ModelPreset::mark_default_by_picker_visibility(&mut presets);
        presets
    }

    fn list_collaboration_modes(&self) -> Vec<CollaborationModeMask> {
        self.0.list_collaboration_modes()
    }

    fn get_default_model<'a>(
        &'a self,
        requested: &'a Option<String>,
        fallback: bool,
        strategy: RefreshStrategy,
        factory: HttpClientFactory,
    ) -> ModelsManagerFuture<'a, String> {
        Box::pin(async move {
            if !fallback && let Some(model) = requested {
                return model.clone();
            }
            let models = self.list_models(strategy, factory).await;
            if let Some(model) = requested
                && models.iter().any(|entry| entry.model == *model)
            {
                return model.clone();
            }
            models
                .iter()
                .find(|model| model.is_default)
                .or(models.first())
                .map(|model| model.model.clone())
                .unwrap_or_default()
        })
    }

    fn get_model_info<'a>(
        &'a self,
        model: &'a str,
        config: &'a ModelsManagerConfig,
    ) -> ModelsManagerFuture<'a, ModelInfo> {
        self.0.get_model_info(model, config)
    }

    fn refresh_if_new_etag(
        &self,
        etag: String,
        factory: HttpClientFactory,
    ) -> ModelsManagerFuture<'_, ()> {
        self.0.refresh_if_new_etag(etag, factory)
    }
}

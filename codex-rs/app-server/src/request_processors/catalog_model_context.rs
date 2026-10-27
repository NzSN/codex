//! Catalog lookup against an explicit pending context or retained thread route.

use super::*;
use crate::model_context::context_error;
use codex_app_server_protocol::ModelProviderCapabilitiesReadParams;
use codex_app_server_protocol::ModelProviderCapabilitiesReadResponse;

impl CatalogRequestProcessor {
    pub(super) async fn scoped_model_config(
        &self,
        connection_id: ConnectionId,
        context_id: Option<&str>,
        thread_id: Option<&str>,
    ) -> Result<Option<Arc<Config>>, JSONRPCErrorError> {
        match (context_id, thread_id) {
            (Some(_), Some(_)) => Err(invalid_params(
                "supply modelContextId or threadId, not both",
            )),
            (None, None) => Ok(None),
            (Some(id), None) => {
                let auth = self.thread_manager.auth_manager();
                let revision = auth.auth_change_state_receiver().borrow().owner_generation;
                let pending = self
                    .config_manager
                    .model_contexts
                    .get(connection_id, id, revision)
                    .await?;
                self.config_manager
                    .check_thread_model_provider(&pending.config)
                    .await
                    .map_err(|error| config_load_error(&error))?;
                Ok(Some(pending.config))
            }
            (None, Some(id)) => {
                let id =
                    ThreadId::from_string(id).map_err(|_| invalid_params("invalid threadId"))?;
                let thread = self.thread_manager.get_thread(id).await.map_err(|_| {
                    context_error(
                        "modelContextUnavailable",
                        "thread must be loaded to read its model context",
                    )
                })?;
                let config = thread.config().await;
                self.config_manager
                    .check_thread_model_provider(&config)
                    .await
                    .map_err(|error| config_load_error(&error))?;
                Ok(Some(config))
            }
        }
    }

    pub(crate) async fn model_provider_capabilities_read_scoped(
        &self,
        connection_id: ConnectionId,
        params: ModelProviderCapabilitiesReadParams,
    ) -> Result<ModelProviderCapabilitiesReadResponse, JSONRPCErrorError> {
        let config = match self
            .scoped_model_config(
                connection_id,
                params.model_context_id.as_deref(),
                params.thread_id.as_deref(),
            )
            .await?
        {
            Some(config) => config,
            None => Arc::new(
                self.config_manager
                    .load_latest_config(None)
                    .await
                    .map_err(|error| config_load_error(&error))?,
            ),
        };
        let provider = codex_model_provider::create_model_provider(
            config.model_provider.clone(),
            Some(self.thread_manager.auth_manager()),
        );
        let caps = provider.capabilities();
        Ok(ModelProviderCapabilitiesReadResponse {
            image_generation: caps.image_generation,
            web_search: caps.web_search,
        })
    }
}

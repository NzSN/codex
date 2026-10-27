//! Fetch picker models without blocking the event loop and keep new-thread defaults in sync.

use super::AppServerSession;
use super::model_preset_from_api_model;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::RequestId;
use codex_protocol::openai_models::ModelPreset;
use uuid::Uuid;

impl AppServerSession {
    pub(crate) fn set_available_models(&mut self, models: Vec<ModelPreset>) {
        if let Some(default) = models
            .iter()
            .find(|model| model.is_default)
            .or(models.first())
        {
            self.default_model = Some(default.model.clone());
        }
        self.available_models = models;
    }

    pub(crate) fn fetch_models(
        &self,
        request_id: Uuid,
        thread_id: Option<codex_protocol::ThreadId>,
        app_event_tx: AppEventSender,
    ) {
        let request_handle = self.request_handle();
        let thread_id = if self.client.mixed_provider_app_server_version() == Some(1) {
            thread_id.map(|id| id.to_string())
        } else {
            None
        };
        let model_context_id = if thread_id.is_none() {
            self.pending_model_context
                .as_ref()
                .map(|context| context.model_context_id.clone())
        } else {
            None
        };
        tokio::spawn(async move {
            let result = request_handle
                .request_typed::<ModelListResponse>(ClientRequest::ModelList {
                    request_id: RequestId::String(format!("model-list-{request_id}")),
                    params: ModelListParams {
                        model_context_id,
                        thread_id,
                        cursor: None,
                        limit: None,
                        include_hidden: Some(true),
                    },
                })
                .await
                .map(|response| {
                    response
                        .data
                        .into_iter()
                        .map(model_preset_from_api_model)
                        .collect()
                })
                .map_err(|err| err.to_string());
            app_event_tx.send(AppEvent::ModelsLoaded { request_id, result });
        });
    }
}

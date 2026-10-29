//! Resolve model choices before bootstrap and retain the server's effective selection.

use super::*;
use codex_app_server_protocol::ModelContextIntent;
use codex_app_server_protocol::ModelContextResolveParams;
use codex_app_server_protocol::ModelContextResolveResponse;

impl AppServerSession {
    pub(crate) fn supports_mixed_model_contexts(&self) -> bool {
        self.client.mixed_provider_app_server_version() == Some(1)
    }

    pub(crate) fn clear_pending_model_context(&mut self) {
        self.pending_model_context = None;
        self.pending_model_target = None;
    }

    pub(crate) async fn ensure_model_context(
        &mut self,
        config: &Config,
        intent: ModelContextIntent,
        thread_id: Option<ThreadId>,
    ) -> Result<()> {
        self.ensure_model_context_choices(config, intent, thread_id, true)
            .await
    }

    pub(crate) async fn ensure_inherited_model_context(
        &mut self,
        config: &Config,
        intent: ModelContextIntent,
        thread_id: Option<ThreadId>,
    ) -> Result<()> {
        self.ensure_model_context_choices(config, intent, thread_id, false)
            .await
    }

    async fn ensure_model_context_choices(
        &mut self,
        config: &Config,
        intent: ModelContextIntent,
        thread_id: Option<ThreadId>,
        apply_launch_choices: bool,
    ) -> Result<()> {
        if self.client.mixed_provider_app_server_version() != Some(1) {
            if apply_launch_choices
                && self.launch_model_choices.requests_model_context()
                && !self.uses_embedded_app_server()
            {
                color_eyre::eyre::bail!(
                    "The shared app-server does not support mixed provider model contexts. Upgrade the daemon or rerun with --no-daemon."
                );
            }
            return Ok(());
        }
        let request_id = self.next_request_id();
        let choices = if apply_launch_choices {
            self.launch_model_choices.clone()
        } else {
            StartupLaunchChoices::default()
        };
        let response: ModelContextResolveResponse = self
            .client
            .request_typed(ClientRequest::ModelContextResolve {
                request_id,
                params: ModelContextResolveParams {
                    intent,
                    thread_id: thread_id.map(|id| id.to_string()),
                    service_name: None,
                    cwd: if intent != ModelContextIntent::Resume {
                        thread_cwd_from_config(
                            config,
                            self.thread_params_mode(),
                            self.remote_cwd_override(),
                        )
                    } else {
                        None
                    },
                    model_provider: choices.provider.clone().or_else(|| {
                        apply_launch_choices
                            .then(|| self.model_provider_override.clone())
                            .flatten()
                    }),
                    model_catalog_json: choices.catalog_json.clone(),
                    model: choices.model.clone(),
                },
            })
            .await
            .map_err(|error| bootstrap_request_error("modelContext/resolve failed", error))?;
        self.pending_model_context = Some(response);
        self.pending_model_target = Some((intent, thread_id));
        Ok(())
    }
}

pub(super) fn configure_start_context(
    params: &mut ThreadStartParams,
    context: &ModelContextResolveResponse,
) {
    params.model_context_id = Some(context.model_context_id.clone());
    params.model = None;
    params.model_provider = None;
    remove_model_choices(&mut params.config);
}

pub(super) fn remove_model_choices(config: &mut Option<HashMap<String, serde_json::Value>>) {
    if let Some(config) = config {
        for key in ["model", "model_provider", "model_catalog_json"] {
            config.remove(key);
        }
    }
}

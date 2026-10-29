//! Bounded, connection-owned handles for resolved model contexts.

use crate::outgoing_message::ConnectionId;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::ModelContextIntent;
use codex_core::config::Config;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::Mutex;

const TTL: Duration = Duration::from_secs(600);
const MAX_PER_CONNECTION: usize = 16;
const MAX_TOTAL: usize = 64;
const MAX_BYTES: usize = 128 * 1024 * 1024;

pub(crate) fn context_error(reason: &str, message: &str) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32602,
        message: message.to_owned(),
        data: Some(serde_json::json!({"reason": reason})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn config() -> Arc<Config> {
        let home = tempfile::tempdir().expect("temporary home");
        Arc::new(
            codex_core::config::ConfigBuilder::default()
                .codex_home(home.path().to_path_buf())
                .build()
                .await
                .expect("config"),
        )
    }

    #[tokio::test]
    async fn handles_bind_to_connection_and_owner_generation_and_close_reclaims_them() {
        let registry = ModelContextRegistry::default();
        let owner = ConnectionId(1);
        registry.connect(owner).await;
        let id = registry
            .insert(
                owner,
                config().await,
                ModelContextIntent::Start,
                None,
                "codex_cli_rs".into(),
                5,
            )
            .await
            .expect("handle");
        assert!(registry.get(owner, &id, 5).await.is_ok());
        assert!(registry.get(ConnectionId(2), &id, 5).await.is_err());
        assert!(registry.get(owner, &id, 6).await.is_err());
        registry.disconnect(owner).await;
        assert!(registry.get(owner, &id, 5).await.is_err());
        assert!(
            registry
                .insert(
                    owner,
                    config().await,
                    ModelContextIntent::Start,
                    None,
                    "codex_cli_rs".into(),
                    5
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn expiry_and_connection_limit_are_enforced() {
        let registry = ModelContextRegistry::default();
        let owner = ConnectionId(1);
        registry.connect(owner).await;
        let config = config().await;
        let mut ids = Vec::new();
        for _ in 0..MAX_PER_CONNECTION {
            ids.push(
                registry
                    .insert(
                        owner,
                        config.clone(),
                        ModelContextIntent::Start,
                        None,
                        "codex_cli_rs".into(),
                        1,
                    )
                    .await
                    .expect("handle"),
            );
        }
        let error = registry
            .insert(
                owner,
                config.clone(),
                ModelContextIntent::Start,
                None,
                "codex_cli_rs".into(),
                1,
            )
            .await
            .expect_err("limit");
        assert_eq!(
            error.data.expect("reason")["reason"],
            "modelContextLimitExceeded"
        );
        registry
            .state
            .lock()
            .await
            .entries
            .get_mut(&ids[0])
            .expect("entry")
            .touched = Instant::now() - TTL;
        let error = registry
            .get(owner, &ids[0], 1)
            .await
            .err()
            .expect("expired");
        assert_eq!(error.data.expect("reason")["reason"], "modelContextExpired");
        assert!(
            registry
                .insert(
                    owner,
                    config,
                    ModelContextIntent::Start,
                    None,
                    "codex_cli_rs".into(),
                    1
                )
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn concurrent_attachment_consumes_exactly_once_and_checks_owner_after_lookup() {
        let registry = ModelContextRegistry::default();
        let owner = ConnectionId(1);
        registry.connect(owner).await;
        let id = registry
            .insert(
                owner,
                config().await,
                ModelContextIntent::Start,
                None,
                "codex_cli_rs".into(),
                5,
            )
            .await
            .expect("handle");
        let (first, second) =
            tokio::join!(registry.take(owner, &id, 5), registry.take(owner, &id, 5));
        assert_ne!(
            first.is_ok(),
            second.is_ok(),
            "only one attachment may consume a handle"
        );
        let context = first.or(second).expect("one attachment");
        assert!(registry.get(owner, &id, 5).await.is_err());
        assert!(registry.validate_attachment(&context, || 5).await.is_ok());
        let changed = registry
            .validate_attachment(&context, || 6)
            .await
            .expect_err("owner changed after lookup");
        assert_eq!(
            changed.data.expect("reason")["reason"],
            "modelContextExpired"
        );
        registry.disconnect(owner).await;
        assert!(registry.validate_attachment(&context, || 5).await.is_err());
    }
}

#[derive(Clone)]
pub(crate) struct PendingModelContext {
    pub(crate) config: Arc<Config>,
    pub(crate) intent: ModelContextIntent,
    pub(crate) thread_id: Option<String>,
    pub(crate) originator: String,
    pub(crate) owner_generation: u64,
    owner: ConnectionId,
    touched: Instant,
    bytes: usize,
}

#[derive(Default)]
struct RegistryState {
    entries: HashMap<String, PendingModelContext>,
    connections: HashSet<ConnectionId>,
}

#[derive(Default)]
pub(crate) struct ModelContextRegistry {
    state: Mutex<RegistryState>,
}

impl ModelContextRegistry {
    pub(crate) async fn insert(
        &self,
        owner: ConnectionId,
        config: Arc<Config>,
        intent: ModelContextIntent,
        thread_id: Option<String>,
        originator: String,
        owner_generation: u64,
    ) -> Result<String, JSONRPCErrorError> {
        let bytes = config
            .resolved_model_context
            .as_ref()
            .and_then(|context| context.snapshot.catalog.as_ref())
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| context_error("modelCatalogInvalid", "cannot encode model catalog"))?
            .map_or(0, |bytes| bytes.len());
        let mut state = self.state.lock().await;
        if !state.connections.contains(&owner) {
            return Err(context_error(
                "modelContextUnavailable",
                "connection closed during model resolution",
            ));
        }
        let entries = &mut state.entries;
        entries.retain(|_, context| context.touched.elapsed() < TTL);
        if entries.len() >= MAX_TOTAL
            || entries
                .values()
                .filter(|context| context.owner == owner)
                .count()
                >= MAX_PER_CONNECTION
            || entries.values().map(|context| context.bytes).sum::<usize>() + bytes > MAX_BYTES
        {
            return Err(context_error(
                "modelContextLimitExceeded",
                "too many pending model contexts",
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        entries.insert(
            id.clone(),
            PendingModelContext {
                config,
                intent,
                thread_id,
                originator,
                owner_generation,
                owner,
                touched: Instant::now(),
                bytes,
            },
        );
        Ok(id)
    }

    pub(crate) async fn get(
        &self,
        owner: ConnectionId,
        id: &str,
        owner_generation: u64,
    ) -> Result<PendingModelContext, JSONRPCErrorError> {
        let mut state = self.state.lock().await;
        let context = state
            .entries
            .get_mut(id)
            .filter(|entry| entry.owner == owner)
            .ok_or_else(|| {
                context_error(
                    "modelContextUnavailable",
                    "model context is unavailable on this connection",
                )
            })?;
        if context.touched.elapsed() >= TTL {
            return Err(context_error(
                "modelContextExpired",
                "model context expired; resolve it again",
            ));
        }
        if context.owner_generation != owner_generation {
            return Err(context_error(
                "modelContextExpired",
                "authentication changed; resolve the model context again",
            ));
        }
        context.touched = Instant::now();
        Ok(context.clone())
    }

    pub(crate) async fn connect(&self, owner: ConnectionId) {
        self.state.lock().await.connections.insert(owner);
    }

    /// Consume on the first attachment attempt. Failed or cancelled attempts must resolve again.
    pub(crate) async fn take(
        &self,
        owner: ConnectionId,
        id: &str,
        owner_generation: u64,
    ) -> Result<PendingModelContext, JSONRPCErrorError> {
        let mut state = self.state.lock().await;
        let context = state
            .entries
            .get(id)
            .filter(|entry| entry.owner == owner)
            .ok_or_else(|| {
                context_error(
                    "modelContextUnavailable",
                    "model context is unavailable on this connection",
                )
            })?;
        if context.owner_generation != owner_generation || context.touched.elapsed() >= TTL {
            return Err(context_error(
                "modelContextExpired",
                "model context expired; resolve it again",
            ));
        }
        state.entries.remove(id).ok_or_else(|| {
            context_error(
                "modelContextUnavailable",
                "model context is unavailable on this connection",
            )
        })
    }

    pub(crate) async fn validate_attachment(
        &self,
        context: &PendingModelContext,
        owner_generation: impl FnOnce() -> u64,
    ) -> Result<(), JSONRPCErrorError> {
        let state = self.state.lock().await;
        if context.owner_generation != owner_generation() {
            return Err(context_error(
                "modelContextExpired",
                "authentication changed during model context attachment",
            ));
        }
        if !state.connections.contains(&context.owner) {
            return Err(context_error(
                "modelContextUnavailable",
                "connection closed during model context attachment",
            ));
        }
        Ok(())
    }

    pub(crate) async fn disconnect(&self, owner: ConnectionId) {
        let mut state = self.state.lock().await;
        state.connections.remove(&owner);
        state.entries.retain(|_, context| context.owner != owner);
    }
}

pub(crate) async fn validate_model_selection(
    config: &Config,
    model: Option<&str>,
    collaboration_model: Option<&str>,
) -> Result<(), JSONRPCErrorError> {
    let selections: Vec<_> = [model, collaboration_model].into_iter().flatten().collect();
    if selections.is_empty() {
        return Ok(());
    }
    if let Some(context) = &config.resolved_model_context {
        let available = if let Some(catalog) = &context.snapshot.catalog {
            catalog
                .models
                .iter()
                .map(|model| model.slug.clone())
                .collect::<Vec<_>>()
        } else if config.model_provider.model_catalog_url.is_some() {
            context
                .models_manager
                .list_models(
                    codex_models_manager::manager::RefreshStrategy::OnlineIfUncached,
                    config.http_client_factory(),
                )
                .await
                .into_iter()
                .map(|model| model.model)
                .collect()
        } else {
            return Ok(());
        };
        for selected in selections {
            if !available.iter().any(|model| model == selected) {
                return Err(context_error(
                    "modelSelectionUnavailable",
                    "selected model is absent from this thread's authoritative catalog",
                ));
            }
        }
    }
    Ok(())
}

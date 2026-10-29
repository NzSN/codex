//! Context-local managers share catalogs without writing the default disk cache.

use std::sync::Mutex;

use chrono::Utc;
use codex_models_manager::cache::ModelsCache;
use codex_models_manager::cache::ModelsCacheEntry;
use codex_models_manager::cache::ModelsCacheError;
use codex_models_manager::cache::ModelsCacheFuture;

const CACHE_TTL: chrono::Duration = chrono::Duration::seconds(300);

#[derive(Debug)]
pub(crate) struct ContextModelsCache {
    identity: String,
    entry: Mutex<Option<ModelsCacheEntry>>,
}

impl ContextModelsCache {
    pub(crate) fn new(identity: String) -> Self {
        Self {
            identity,
            entry: Mutex::new(None),
        }
    }
}

impl ModelsCache for ContextModelsCache {
    fn load<'a>(
        &'a self,
        client_version: &'a str,
    ) -> ModelsCacheFuture<'a, Result<Option<ModelsCacheEntry>, ModelsCacheError>> {
        Box::pin(async move {
            let entry = self
                .entry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Ok(entry
                .as_ref()
                .filter(|entry| {
                    entry.identity.as_deref() == Some(self.identity.as_str())
                        && entry.client_version.as_deref() == Some(client_version)
                        && Utc::now().signed_duration_since(entry.fetched_at) <= CACHE_TTL
                })
                .cloned())
        })
    }

    fn store<'a>(
        &'a self,
        entry: &'a ModelsCacheEntry,
    ) -> ModelsCacheFuture<'a, Result<(), ModelsCacheError>> {
        Box::pin(async move {
            // Auth can change while an existing manager is alive. Its new identity must not
            // publish into the cache retained by contexts for the previous account.
            if entry.identity.as_deref() == Some(self.identity.as_str()) {
                *self
                    .entry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(entry.clone());
            }
            Ok(())
        })
    }

    fn refresh_ttl<'a>(
        &'a self,
        client_version: &'a str,
        identity: &'a str,
        etag: &'a str,
    ) -> ModelsCacheFuture<'a, Result<(), ModelsCacheError>> {
        Box::pin(async move {
            let mut entry = self
                .entry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = entry.as_mut()
                && identity == self.identity
                && entry.identity.as_deref() == Some(identity)
                && entry.client_version.as_deref() == Some(client_version)
                && entry.etag.as_deref() == Some(etag)
                && entry.fetched_at != chrono::DateTime::<Utc>::MIN_UTC
            {
                entry.fetched_at = Utc::now();
            }
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "models_cache_tests.rs"]
mod tests;

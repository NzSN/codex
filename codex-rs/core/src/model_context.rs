//! Provider routing and model metadata resolved as one session-owned unit.

#[path = "model_context_models.rs"]
mod model_context_models;

use crate::config::Config;
use codex_features::Feature;
use codex_login::AuthManager;
use codex_model_provider::create_model_provider;
use codex_model_provider_info::ModelProviderInfo;
use codex_models_manager::ModelCatalogOriginator;
use codex_models_manager::manager::SharedModelsManager;
use codex_protocol::model_context::ModelContextSnapshot;
use codex_protocol::openai_models::ModelsResponse;
use sha2::Digest;
use sha2::Sha256;
use std::collections::HashSet;
use std::fmt;
use std::fs::File;
use std::io;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

pub const MAX_CATALOG_BYTES: usize = 8 * 1024 * 1024;
const MAX_MODELS: usize = 2048;
const MAX_INSTRUCTION_BYTES: usize = 64 * 1024;

pub struct ResolvedModelContext {
    pub snapshot: ModelContextSnapshot,
    pub models_manager: SharedModelsManager,
    unscoped_models_manager: SharedModelsManager,
    catalog_originator: Option<ModelCatalogOriginator>,
    provider: ModelProviderInfo,
    api_key_model_discovery: bool,
}

impl PartialEq for ResolvedModelContext {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot == other.snapshot
            && self.provider == other.provider
            && self.api_key_model_discovery == other.api_key_model_discovery
            && self.catalog_originator == other.catalog_originator
    }
}

impl fmt::Debug for ResolvedModelContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.snapshot.fmt(f)
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub fn load_catalog(path: &Path) -> io::Result<ModelsResponse> {
    let mut bytes = Vec::new();
    if !std::fs::metadata(path)?.is_file() {
        return Err(invalid("model catalog must be a regular file"));
    }
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("model catalog must be a regular file"));
    }
    file.take((MAX_CATALOG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err(invalid("model catalog exceeds 8 MiB"));
    }
    let catalog: ModelsResponse = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("model catalog is not a valid ModelsResponse"))?;
    validate_catalog(&catalog)?;
    Ok(catalog)
}

pub fn validate_catalog(catalog: &ModelsResponse) -> io::Result<()> {
    if catalog.models.is_empty() || catalog.models.len() > MAX_MODELS {
        return Err(invalid("model catalog must contain 1..=2048 models"));
    }
    let mut slugs = HashSet::new();
    for model in &catalog.models {
        if model.slug.trim().is_empty() || !slugs.insert(&model.slug) {
            return Err(invalid(
                "model catalog has an empty or duplicate model slug",
            ));
        }
        if model.context_window.is_some_and(|n| n <= 0)
            || model.max_context_window.is_some_and(|n| n <= 0)
            || model.auto_compact_token_limit.is_some_and(|n| n <= 0)
            || !(1..=100).contains(&model.effective_context_window_percent)
        {
            return Err(invalid("model catalog contains invalid context limits"));
        }
        if let Some(messages) = &model.model_messages {
            for instructions in [
                messages.instructions_template.as_deref(),
                messages.persistent_instructions.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                if instructions.len() > MAX_INSTRUCTION_BYTES {
                    return Err(invalid("model instructions exceed 64 KiB"));
                }
            }
        }
    }
    if serde_json::to_vec(catalog)?.len() > MAX_CATALOG_BYTES {
        return Err(invalid("canonical model catalog exceeds 8 MiB"));
    }
    Ok(())
}

pub fn catalog_digest(catalog: &ModelsResponse) -> io::Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(1, catalog))?)
    ))
}

/// Compare with authority-owned content, never a caller's mutable effective catalog.
pub(crate) fn validate_managed_catalog(
    config: &Config,
    catalog: Option<&ModelsResponse>,
) -> io::Result<()> {
    if let Some(required) = &config.config_layer_stack.requirements().model_catalog_json {
        let required = load_catalog(required.value.as_path())?;
        if catalog != Some(&required) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "model catalog violates managed requirements",
            ));
        }
    }
    Ok(())
}

/// Restore a thread-owned snapshot against current host provider definitions and restrictions.
pub(crate) fn restore_context_snapshot(
    config: &mut Config,
    auth: Arc<AuthManager>,
    snapshot: ModelContextSnapshot,
) -> io::Result<()> {
    let provider = config
        .model_providers
        .get(&snapshot.provider_id)
        .cloned()
        .ok_or_else(|| invalid("saved model provider is unavailable"))?;
    config.model_provider_id = snapshot.provider_id.clone();
    config.model_provider = provider;
    let context = ResolvedModelContext::from_snapshot(config, auth, snapshot)?;
    config.model_catalog = context.snapshot.catalog.clone();
    config.resolved_model_context = Some(context);
    Ok(())
}

/// Fingerprint an explicit public route descriptor, never serialized provider secrets.
pub fn route_fingerprint(provider_id: &str, provider: &ModelProviderInfo) -> io::Result<String> {
    for endpoint in [
        provider.base_url.as_deref(),
        provider.model_catalog_url.as_ref().map(|url| url.as_str()),
    ]
    .into_iter()
    .flatten()
    {
        let endpoint =
            url::Url::parse(endpoint).map_err(|_| invalid("invalid model provider endpoint"))?;
        if !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
        {
            return Err(invalid(
                "model context endpoints must use separate host-owned credential and query references",
            ));
        }
    }
    let mut headers = std::collections::BTreeMap::new();
    let mut credential_headers = Vec::new();
    for (name, value) in provider.http_headers.iter().flatten() {
        let name = name.to_ascii_lowercase();
        match name.as_str() {
            // Transport identification is not a durable route or credential identity.
            "version" | "x-amzn-mantle-client-agent" => {}
            "authorization" | "proxy-authorization" | "api-key" | "x-api-key" => {
                credential_headers.push(name)
            }
            "openai-organization"
            | "openai-project"
            | "chatgpt-account-id"
            | "x-tenant-id"
            | "host"
            | "originator"
            | "openai-beta"
            | "api-version" => {
                headers.insert(name, value.as_str());
            }
            _ => {
                return Err(invalid(
                    "model context static header needs an explicit public routing definition or env_http_headers reference",
                ));
            }
        }
    }
    credential_headers.sort();
    let mut queries = std::collections::BTreeMap::new();
    let mut credential_queries = Vec::new();
    for (name, value) in provider.query_params.iter().flatten() {
        match name.as_str() {
            "api_key" | "key" | "token" | "access_token" => credential_queries.push(name.as_str()),
            "api-version" | "region" | "tenant" | "project" => {
                queries.insert(name, value.as_str());
            }
            _ => {
                return Err(invalid(
                    "model context query parameter needs an explicit public routing definition",
                ));
            }
        }
    }
    credential_queries.sort();
    let environment_headers: std::collections::BTreeMap<_, _> =
        provider.env_http_headers.iter().flatten().collect();
    let descriptor = serde_json::json!({
        "version": 1,
        "provider": provider_id,
        "base_url": provider.base_url,
        "wire_api": provider.wire_api,
        "env_key": provider.env_key,
        "static_bearer_reference": provider.experimental_bearer_token.is_some(),
        "requires_openai_auth": provider.requires_openai_auth,
        "capabilities": provider.capabilities,
        "gateway_oauth": provider.gateway_oauth,
        "aws": provider.aws.as_ref().map(|aws| (&aws.region, &aws.profile, aws.credential_export.is_some(), aws.auth_refresh.is_some())),
        "command_auth": provider.auth.is_some(),
        "headers": headers,
        "credential_headers": credential_headers,
        "queries": queries,
        "credential_queries": credential_queries,
        "environment_headers": environment_headers,
        "catalog_endpoint": provider.model_catalog_url.as_ref().map(|url| url.as_str()),
    });
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&descriptor)?)
    ))
}

impl ResolvedModelContext {
    pub fn resolve(
        config: &Config,
        auth: Arc<AuthManager>,
        catalog_source: Option<String>,
    ) -> io::Result<Arc<Self>> {
        if let Some(catalog) = &config.model_catalog {
            validate_catalog(catalog)?;
            if config
                .model
                .as_ref()
                .is_some_and(|selected| !catalog.models.iter().any(|model| model.slug == *selected))
            {
                return Err(invalid(
                    "selected model is absent from the static model catalog",
                ));
            }
        }
        let snapshot = ModelContextSnapshot {
            version: 1,
            provider_id: config.model_provider_id.clone(),
            route_fingerprint: route_fingerprint(
                &config.model_provider_id,
                &config.model_provider,
            )?,
            catalog_source,
            catalog_digest: config
                .model_catalog
                .as_ref()
                .map(catalog_digest)
                .transpose()?,
            catalog: config.model_catalog.clone(),
        };
        Self::from_snapshot(config, auth, snapshot)
    }

    pub fn from_snapshot(
        config: &Config,
        auth: Arc<AuthManager>,
        mut snapshot: ModelContextSnapshot,
    ) -> io::Result<Arc<Self>> {
        if snapshot.catalog.is_none()
            && let Some(digest) = &snapshot.catalog_digest
        {
            let path = catalog_object_path(config, digest)?;
            snapshot.catalog = Some(load_catalog(&path)?);
        }
        if config
            .config_layer_stack
            .required_model_provider()
            .is_some_and(|required| required != snapshot.provider_id)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "model context violates managed provider selection",
            ));
        }
        validate_managed_catalog(config, snapshot.catalog.as_ref())?;
        if snapshot.version != 1 {
            return Err(invalid("unsupported required model-context version"));
        }
        if snapshot.provider_id != config.model_provider_id
            || snapshot.route_fingerprint
                != route_fingerprint(&config.model_provider_id, &config.model_provider)?
        {
            return Err(invalid(
                "saved model provider route changed; resolve an explicit replacement context",
            ));
        }
        let digest = snapshot
            .catalog
            .as_ref()
            .map(|catalog| {
                validate_catalog(catalog)?;
                catalog_digest(catalog)
            })
            .transpose()?;
        if digest != snapshot.catalog_digest {
            return Err(invalid(
                "saved model catalog digest does not match its snapshot",
            ));
        }
        let provider = create_model_provider(config.model_provider.clone(), Some(auth));
        // Pending/independent contexts must not overwrite the default disk cache.
        let manager = provider.models_manager_for_context(snapshot.catalog.clone());
        let manager: SharedModelsManager = if config.model_provider.requires_openai_auth {
            manager
        } else {
            Arc::new(model_context_models::ProviderCatalogModelsManager(manager))
        };
        let api_key_model_discovery = config.features.enabled(Feature::ApiKeyModelDiscovery);
        manager.set_api_key_model_discovery_enabled(api_key_model_discovery);
        Ok(Arc::new(Self {
            snapshot,
            unscoped_models_manager: manager.clone(),
            models_manager: manager,
            catalog_originator: None,
            provider: config.model_provider.clone(),
            api_key_model_discovery,
        }))
    }

    /// Keep the thread's authoritative binding and runtime manager aligned while
    /// selecting an originator catalog. Unknown originators restore the default scope.
    pub fn for_originator(self: &Arc<Self>, originator: &str) -> Arc<Self> {
        let requested = ModelCatalogOriginator::from_originator(originator);
        if requested == self.catalog_originator {
            return self.clone();
        }
        let scoped =
            requested.and_then(|scope| self.unscoped_models_manager.for_originator(scope.as_str()));
        let (models_manager, catalog_originator) = match scoped {
            Some(manager) => (manager, requested),
            None => (self.unscoped_models_manager.clone(), None),
        };
        if Arc::ptr_eq(&models_manager, &self.models_manager) {
            return self.clone();
        }
        Arc::new(Self {
            snapshot: self.snapshot.clone(),
            models_manager,
            unscoped_models_manager: self.unscoped_models_manager.clone(),
            catalog_originator,
            provider: self.provider.clone(),
            api_key_model_discovery: self.api_key_model_discovery,
        })
    }

    pub fn matches_config(&self, config: &Config) -> bool {
        self.snapshot.provider_id == config.model_provider_id
            && self.provider == config.model_provider
            && self.snapshot.catalog == config.model_catalog
            && self
                .discovery_setting_matches(config.features.enabled(Feature::ApiKeyModelDiscovery))
    }

    pub fn discovery_setting_matches(&self, api_key_model_discovery: bool) -> bool {
        self.api_key_model_discovery == api_key_model_discovery
    }

    pub fn validate_static_model_selection(&self, model: Option<&str>) -> io::Result<()> {
        if let (Some(catalog), Some(model)) = (&self.snapshot.catalog, model)
            && !catalog.models.iter().any(|entry| entry.slug == model)
        {
            return Err(invalid(
                "selected model is absent from the static model catalog",
            ));
        }
        Ok(())
    }

    pub(crate) fn persistence_snapshot(&self, config: &Config) -> ModelContextSnapshot {
        if !config.ephemeral
            && matches!(
                config.experimental_thread_store,
                crate::config::ThreadStoreConfig::Local
            )
        {
            // The object is committed before Session::spawn can publish its reference.
            ModelContextSnapshot {
                version: self.snapshot.version,
                provider_id: self.snapshot.provider_id.clone(),
                route_fingerprint: self.snapshot.route_fingerprint.clone(),
                catalog_source: self.snapshot.catalog_source.clone(),
                catalog_digest: self.snapshot.catalog_digest.clone(),
                catalog: None,
            }
        } else {
            self.snapshot.clone()
        }
    }
}

#[cfg(test)]
#[path = "model_context_tests.rs"]
mod tests;

/// Persist the minimum reader contract before creating a durable mixed-context session.
pub(crate) fn require_store_version(config: &Config) -> io::Result<()> {
    use std::io::Write;
    if let Some(context) = &config.resolved_model_context
        && let (Some(digest), Some(catalog)) =
            (&context.snapshot.catalog_digest, &context.snapshot.catalog)
    {
        let path = catalog_object_path(config, digest)?;
        let directory = path
            .parent()
            .ok_or_else(|| invalid("catalog object has no directory"))?;
        std::fs::create_dir_all(directory)?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(&serde_json::to_vec(catalog)?)?;
        temporary.as_file().sync_all()?;
        match temporary.persist_noclobber(&path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                if catalog_digest(&load_catalog(&path)?)? != *digest {
                    return Err(invalid("existing catalog object failed digest validation"));
                }
            }
            Err(error) => return Err(error.error),
        }
        #[cfg(unix)]
        File::open(directory)?.sync_all()?;
    }
    let path = config
        .codex_home
        .join(codex_protocol::model_context::STORE_REQUIREMENT_FILE);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(b"1\n")?;
            file.sync_all()
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let value = std::fs::read_to_string(path)?;
            if value.trim() != "1" {
                return Err(invalid("unsupported required model-context store version"));
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn catalog_object_path(config: &Config, digest: &str) -> io::Result<std::path::PathBuf> {
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("invalid catalog object digest"));
    }
    Ok(config
        .codex_home
        .join("model-contexts")
        .join(format!("{digest}.json"))
        .to_path_buf())
}

/// Restore immutable catalog content without loading user configuration or credentials.
pub fn restore_catalog_snapshot(
    codex_home: &Path,
    snapshot: &ModelContextSnapshot,
) -> io::Result<Option<ModelsResponse>> {
    if snapshot.version != 1 {
        return Err(invalid("unsupported required model-context version"));
    }
    let catalog = match (&snapshot.catalog, &snapshot.catalog_digest) {
        (Some(catalog), _) => Some(catalog.clone()),
        (None, Some(digest)) => {
            if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(invalid("invalid catalog object digest"));
            }
            Some(load_catalog(
                &codex_home
                    .join("model-contexts")
                    .join(format!("{digest}.json")),
            )?)
        }
        (None, None) => None,
    };
    let digest = catalog
        .as_ref()
        .map(|catalog| {
            validate_catalog(catalog)?;
            catalog_digest(catalog)
        })
        .transpose()?;
    if digest != snapshot.catalog_digest {
        return Err(invalid("saved catalog digest mismatch"));
    }
    Ok(catalog)
}

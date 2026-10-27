//! Secret-free, versioned model-context state retained with a conversation.

use crate::openai_models::ModelsResponse;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use ts_rs::TS;

/// Managed launchers pass this requirement to prevent selecting an unaware older server.
pub const STORE_REQUIREMENT_FILE: &str = "required-model-context-version";

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct ModelContextSnapshot {
    pub version: u32,
    pub provider_id: String,
    pub route_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<ModelsResponse>,
}

// Catalogs contain instructions. Diagnostics must never print their contents.
impl fmt::Debug for ModelContextSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelContextSnapshot")
            .field("version", &self.version)
            .field("provider_id", &self.provider_id)
            .field("catalog_digest", &self.catalog_digest)
            .finish_non_exhaustive()
    }
}

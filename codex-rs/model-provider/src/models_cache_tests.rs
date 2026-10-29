use super::*;
use crate::create_model_provider;
use crate::shared_state::ModelProviderSharedState;
use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use codex_model_provider_info::ModelProviderInfo;
use codex_models_manager::manager::RefreshStrategy;
use codex_protocol::openai_models::ModelsResponse;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

fn entry(identity: &str) -> ModelsCacheEntry {
    ModelsCacheEntry {
        fetched_at: Utc::now(),
        etag: Some("etag".into()),
        client_version: Some("1.0.0".into()),
        identity: Some(identity.into()),
        models: vec![],
    }
}

#[tokio::test]
async fn context_cache_rejects_other_identities_versions_and_expired_entries() {
    let cache = ContextModelsCache::new("first".into());
    let fresh = entry("first");
    cache.store(&fresh).await.unwrap();
    assert_eq!(cache.load("1.0.0").await.unwrap(), Some(fresh.clone()));
    assert_eq!(cache.load("other-version").await.unwrap(), None);
    cache.store(&entry("other-account")).await.unwrap();
    assert_eq!(cache.load("1.0.0").await.unwrap(), Some(fresh.clone()));

    for fetched_at in [
        Utc::now() - CACHE_TTL - chrono::Duration::seconds(1),
        chrono::DateTime::<Utc>::MIN_UTC,
    ] {
        cache
            .store(&ModelsCacheEntry {
                fetched_at,
                ..fresh.clone()
            })
            .await
            .unwrap();
        assert_eq!(cache.load("1.0.0").await.unwrap(), None);
    }
}

#[tokio::test]
async fn context_cache_revalidation_preserves_identity_and_access_rejections() {
    let cache = ContextModelsCache::new("first".into());
    let expired = ModelsCacheEntry {
        fetched_at: Utc::now() - CACHE_TTL - chrono::Duration::seconds(1),
        ..entry("first")
    };
    for (version, identity, etag) in [
        ("other-version", "first", "etag"),
        ("1.0.0", "other-account", "etag"),
        ("1.0.0", "first", "other-etag"),
    ] {
        cache.store(&expired).await.unwrap();
        cache.refresh_ttl(version, identity, etag).await.unwrap();
        assert_eq!(cache.load("1.0.0").await.unwrap(), None);
    }
    cache.refresh_ttl("1.0.0", "first", "etag").await.unwrap();
    let refreshed = cache.load("1.0.0").await.unwrap().unwrap();
    assert!(refreshed.fetched_at > expired.fetched_at);
    assert_eq!(refreshed.identity, expired.identity);
    assert_eq!(refreshed.models, expired.models);
    cache
        .store(&ModelsCacheEntry {
            fetched_at: chrono::DateTime::<Utc>::MIN_UTC,
            ..expired
        })
        .await
        .unwrap();
    cache.refresh_ttl("1.0.0", "first", "etag").await.unwrap();
    assert_eq!(cache.load("1.0.0").await.unwrap(), None);
}

#[test]
fn context_cache_registry_is_bounded_and_keeps_recent_scopes() {
    let state = ModelProviderSharedState::default();
    let oldest = state.context_models_cache("oldest".into());
    let recent = state.context_models_cache("recent".into());
    for index in 0..31 {
        state.context_models_cache(format!("scope-{index}"));
        assert!(Arc::ptr_eq(
            &recent,
            &state.context_models_cache("recent".into())
        ));
    }
    assert!(!Arc::ptr_eq(
        &oldest,
        &state.context_models_cache("oldest".into())
    ));
}

#[tokio::test]
async fn restored_managers_reuse_only_the_same_provider_and_credentials() {
    let server = MockServer::start().await;
    let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault);
    for (catalog_path, token, marker) in [
        ("/first/catalog", "first-key", "first catalog"),
        ("/first/catalog", "other-key", "other account"),
        ("/second/catalog", "first-key", "other provider"),
    ] {
        let mut catalog = codex_models_manager::bundled_models_response().unwrap();
        catalog.models.truncate(1);
        catalog.models[0].slug = "shared-model".into();
        catalog.models[0].display_name = marker.into();
        catalog.models[0].model_messages = Some(codex_protocol::openai_models::ModelMessages {
            instructions_template: Some(marker.into()),
            ..Default::default()
        });
        let mock = Mock::given(method("GET"))
            .and(path(catalog_path))
            .and(header("authorization", format!("Bearer {token}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(&catalog))
            .expect(1)
            .mount_as_scoped(&server)
            .await;
        let info = ModelProviderInfo {
            model_catalog_url: Some(format!("{}{catalog_path}", server.uri()).into()),
            experimental_bearer_token: Some(token.into()),
            requires_openai_auth: false,
            // Another provider test toggles the process-wide residency requirement.
            // Pin its effective value so that test cannot change this cache's identity.
            http_headers: Some(std::collections::HashMap::from([(
                codex_login::default_client::RESIDENCY_HEADER_NAME.into(),
                "us".into(),
            )])),
            ..ModelProviderInfo::create_openai_provider(Some(server.uri()))
        };
        for _ in 0..2 {
            let manager = create_model_provider(info.clone(), None)
                .models_manager_for_context(/*config_model_catalog*/ None);
            manager.set_api_key_model_discovery_enabled(true);
            let actual = manager
                .raw_model_catalog(RefreshStrategy::OnlineIfUncached, factory.clone())
                .await;
            assert_eq!(actual.models.len(), 1, "{marker}: catalog was not loaded");
            assert_eq!(actual, catalog);
        }
        // A cached remote catalog cannot bypass this context's discovery opt-out.
        let disabled = create_model_provider(info, None)
            .models_manager_for_context(/*config_model_catalog*/ None);
        assert_eq!(
            disabled
                .raw_model_catalog(RefreshStrategy::Offline, factory.clone())
                .await,
            ModelsResponse { models: vec![] }
        );
        drop(mock);
    }
}

use super::*;
use crate::config::ConfigBuilder;

fn catalog() -> ModelsResponse {
    let mut response = codex_models_manager::bundled_models_response().expect("bundled catalog");
    response.models.truncate(1);
    response.models[0].slug = "shared-model-name".to_string();
    response.models[0].context_window = Some(32_000);
    response.models[0].max_context_window = Some(128_000);
    response
}

#[tokio::test]
async fn originator_context_scoping_is_stable_and_can_restore_the_default() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    let config = ConfigBuilder::without_managed_config_for_tests()
        .codex_home(home.path().to_path_buf())
        .build()
        .await?;
    let auth = AuthManager::from_auth_for_testing(
        codex_login::CodexAuth::create_dummy_chatgpt_auth_for_testing(),
    );
    let default = ResolvedModelContext::resolve(&config, auth, None)?;
    let work = default.for_originator("codex_work_cca");
    let chat = work.for_originator("chatgpt_cca");
    assert!(!Arc::ptr_eq(&default.models_manager, &work.models_manager));
    assert!(!Arc::ptr_eq(&work.models_manager, &chat.models_manager));
    assert!(Arc::ptr_eq(&work, &work.for_originator("codex_work_cca")));
    assert!(Arc::ptr_eq(
        &default.models_manager,
        &chat.for_originator("codex_cli_rs").models_manager
    ));
    assert!(work.matches_config(&config));
    assert_eq!(default.snapshot, work.snapshot);
    assert_ne!(default, work);
    assert_ne!(work, chat);
    assert_eq!(default, chat.for_originator("codex_cli_rs"));
    Ok(())
}

#[tokio::test]
async fn scoped_native_catalog_retains_the_runtime_binding_for_custom_review() -> anyhow::Result<()>
{
    use codex_http_client::HttpClientFactory;
    use codex_http_client::OutboundProxyPolicy;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    let server = MockServer::start().await;
    let home = tempfile::tempdir()?;
    let mut config = ConfigBuilder::without_managed_config_for_tests()
        .codex_home(home.path().to_path_buf())
        .build()
        .await?;
    config.features.disable(Feature::CodeMode)?;
    config.features.disable(Feature::CodeModeHost)?;
    config.features.enable(Feature::ApiKeyModelDiscovery)?;
    config.features.disable(Feature::ShellSnapshot)?;
    config.model_provider_id = "originator-catalog".into();
    config.model_provider = ModelProviderInfo {
        name: "Originator catalog".into(),
        base_url: Some(server.uri()),
        model_catalog_url: Some(format!("{}/catalog", server.uri()).into()),
        experimental_bearer_token: Some("originator-test-key".into()),
        requires_openai_auth: false,
        ..ModelProviderInfo::create_openai_provider(None)
    };
    config.model_providers.insert(
        config.model_provider_id.clone(),
        config.model_provider.clone(),
    );
    config.model = Some("shared-model-name".into());
    let threads = crate::test_support::thread_manager_with_models_provider_and_home(
        codex_login::CodexAuth::from_api_key("originator-test-key"),
        config.model_provider.clone(),
        home.path().to_path_buf(),
        Arc::new(codex_exec_server::EnvironmentManager::default_for_tests()),
    );
    config.resolved_model_context = Some(ResolvedModelContext::resolve(
        &config,
        threads.auth_manager(),
        None,
    )?);
    for scope in ["codex_work_cca", "chatgpt_cca"] {
        let mut response = catalog();
        response.models[0].description = Some(scope.into());
        Mock::given(method("GET"))
            .and(path("/catalog"))
            .and(header("originator", scope))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
        let mut options = crate::StartThreadOptions::new(config.clone());
        options.metrics_service_name = Some(scope.into());
        let started = threads.start_thread(options).await?;
        let bound = started.thread.config().await;
        let context = bound
            .resolved_model_context
            .as_ref()
            .expect("runtime context");
        let manager = &started.thread.session.services.models_manager;
        assert!(Arc::ptr_eq(&context.models_manager, manager));
        let info = manager
            .get_model_info("shared-model-name", &bound.to_models_manager_config())
            .await;
        assert_eq!(info.description.as_deref(), Some(scope));
        assert!(!info.used_fallback_model_metadata);
        let catalog = manager
            .model_catalog_with_origins(
                codex_models_manager::manager::RefreshStrategy::InMemory,
                HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
            )
            .await;
        let selected = catalog
            .iter()
            .find(|entry| entry.model.slug == "shared-model-name")
            .expect("scoped provider model");
        assert_eq!(selected.origin, codex_models_manager::ModelOrigin::Provider);
        assert_eq!(selected.model.description.as_deref(), Some(scope));
        assert!(
            !manager
                .list_models(
                    codex_models_manager::manager::RefreshStrategy::Offline,
                    HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault)
                )
                .await
                .is_empty()
        );
        started.thread.shutdown_and_wait().await?;
    }
    server.verify().await;
    Ok(())
}

#[tokio::test]
async fn discovery_settings_require_an_isolated_manager_and_compatible_binding()
-> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    let mut config = ConfigBuilder::without_managed_config_for_tests()
        .codex_home(home.path().to_path_buf())
        .build()
        .await?;
    config.features.enable(Feature::ApiKeyModelDiscovery)?;
    let auth =
        AuthManager::from_auth_for_testing(codex_login::CodexAuth::from_api_key("fixture-key"));
    let enabled = ResolvedModelContext::resolve(&config, auth.clone(), None)?;
    assert!(enabled.matches_config(&config));
    config.features.disable(Feature::ApiKeyModelDiscovery)?;
    assert!(!enabled.matches_config(&config));
    let disabled = ResolvedModelContext::from_snapshot(&config, auth, enabled.snapshot.clone())?;
    assert!(disabled.matches_config(&config));
    assert!(disabled.discovery_setting_matches(false));
    assert!(enabled.discovery_setting_matches(true));
    assert!(!Arc::ptr_eq(
        &enabled.models_manager,
        &disabled.models_manager
    ));
    assert_ne!(
        enabled, disabled,
        "discovery strategy participates in context compatibility"
    );
    let opted_out = ConfigBuilder::without_managed_config_for_tests()
        .codex_home(home.path().to_path_buf())
        .cli_overrides(vec![(
            "features.api_key_model_discovery".into(),
            toml::Value::Boolean(false),
        )])
        .harness_overrides(crate::config::ConfigOverrides {
            model_context: Some(enabled.clone()),
            ..Default::default()
        })
        .build()
        .await?;
    assert!(
        !enabled.matches_config(&opted_out),
        "the core factory must isolate an inherited manager when its setting differs"
    );
    let compatible = ConfigBuilder::without_managed_config_for_tests()
        .codex_home(home.path().to_path_buf())
        .cli_overrides(vec![(
            "features.api_key_model_discovery".into(),
            toml::Value::Boolean(true),
        )])
        .harness_overrides(crate::config::ConfigOverrides {
            model_context: Some(enabled.clone()),
            ..Default::default()
        })
        .build()
        .await?;
    assert!(enabled.matches_config(&compatible));
    Ok(())
}

#[tokio::test]
async fn reused_static_context_rejects_unknown_model_at_thread_start() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    let mut config = ConfigBuilder::without_managed_config_for_tests()
        .codex_home(home.path().to_path_buf())
        .build()
        .await?;
    config.features.disable(Feature::CodeMode)?;
    config.features.disable(Feature::CodeModeHost)?;
    config.features.disable(Feature::ApiKeyModelDiscovery)?;
    let threads = crate::test_support::thread_manager_with_models_provider_and_home(
        codex_login::CodexAuth::from_api_key("fixture-key"),
        config.model_provider.clone(),
        home.path().to_path_buf(),
        Arc::new(codex_exec_server::EnvironmentManager::default_for_tests()),
    );
    config.model_catalog = Some(catalog());
    config.model = Some("shared-model-name".into());
    config.resolved_model_context = Some(ResolvedModelContext::resolve(
        &config,
        threads.auth_manager(),
        None,
    )?);
    config.model = Some("not-in-catalog".into());
    assert!(
        config
            .resolved_model_context
            .as_ref()
            .expect("context")
            .matches_config(&config)
    );
    let error = threads
        .start_thread(crate::StartThreadOptions::new(config.clone()))
        .await
        .err()
        .expect("unknown model cannot reach fallback metadata");
    assert!(
        error
            .to_string()
            .contains("absent from the static model catalog"),
        "{error}"
    );
    config.model = Some("shared-model-name".into());
    let valid = threads
        .start_thread(crate::StartThreadOptions::new(config.clone()))
        .await?;
    assert_eq!(valid.session_configured.model, "shared-model-name");
    valid.thread.shutdown_and_wait().await?;
    let original = config
        .resolved_model_context
        .clone()
        .expect("original context");
    config.features.enable(Feature::ApiKeyModelDiscovery)?;
    let isolated = threads
        .start_thread(crate::StartThreadOptions::new(config))
        .await?;
    let isolated_config = isolated.thread.config().await;
    let context = isolated_config
        .resolved_model_context
        .as_ref()
        .expect("isolated context");
    assert!(context.discovery_setting_matches(true));
    assert!(original.discovery_setting_matches(false));
    assert!(
        !Arc::ptr_eq(&original.models_manager, &context.models_manager),
        "changing inherited settings must not mutate the original manager"
    );
    isolated.thread.shutdown_and_wait().await?;
    Ok(())
}

#[tokio::test]
async fn managed_catalog_checks_authoritative_content_after_config_mutation() -> anyhow::Result<()>
{
    let home = tempfile::tempdir()?;
    let required_path = home.path().join("required.json");
    std::fs::write(&required_path, serde_json::to_vec(&catalog())?)?;
    std::fs::write(
        home.path().join("requirements.toml"),
        format!(
            "model_catalog_json = {:?}\n",
            required_path.display().to_string()
        ),
    )?;
    let mut config = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .loader_overrides(
            codex_config::LoaderOverrides::with_managed_config_path_for_tests(
                home.path().join("managed_config.toml"),
            ),
        )
        .build()
        .await?;
    let auth =
        AuthManager::from_auth_for_testing(codex_login::CodexAuth::from_api_key("fixture-key"));
    let saved = ResolvedModelContext::resolve(&config, auth.clone(), None)?;
    config.model_provider_id = "ollama".to_string();
    config.model_provider = config.model_providers["ollama"].clone();
    config.model_catalog = None;
    let error = ResolvedModelContext::resolve(&config, auth.clone(), None)
        .expect_err("two absent catalogs cannot satisfy a mandatory file");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    // Equality with a mutated effective catalog also cannot replace authority-owned content.
    let mut other = catalog();
    other.models[0].context_window = Some(96_000);
    config.model_catalog = Some(other.clone());
    assert!(ResolvedModelContext::resolve(&config, auth.clone(), None).is_err());
    config.model_provider_id = saved.snapshot.provider_id.clone();
    config.model_provider = config.model_providers[&config.model_provider_id].clone();
    config.model_catalog = saved.snapshot.catalog.clone();
    std::fs::write(&required_path, serde_json::to_vec(&other)?)?;
    assert!(
        ResolvedModelContext::from_snapshot(&config, auth, saved.snapshot.clone()).is_err(),
        "changed required content must be revalidated"
    );
    Ok(())
}

#[test]
fn catalog_validation_rejects_duplicate_models_and_invalid_limits() {
    let mut response = catalog();
    response.models.push(response.models[0].clone());
    assert!(validate_catalog(&response).is_err());
    response.models.pop();
    response.models[0].context_window = Some(0);
    assert!(validate_catalog(&response).is_err());
}

#[test]
fn catalog_loading_is_bounded_and_requires_valid_metadata() {
    let directory = tempfile::tempdir().expect("temp directory");
    let file = directory.path().join("models.json");
    std::fs::write(&file, vec![b' '; MAX_CATALOG_BYTES + 1]).expect("oversized file");
    assert!(load_catalog(&file).is_err());
    std::fs::write(&file, br#"{"models":[]}"#).expect("empty catalog");
    assert!(load_catalog(&file).is_err());
    let response = catalog();
    std::fs::write(&file, serde_json::to_vec(&response).expect("encode")).expect("catalog file");
    assert_eq!(load_catalog(&file).expect("valid catalog"), response);
}

#[tokio::test]
async fn same_model_slug_uses_independent_context_metadata_and_restores_without_file() {
    let home = tempfile::tempdir().expect("temp directory");
    let mut config = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .build()
        .await
        .expect("config");
    let auth = AuthManager::shared_from_config(&config, false)
        .await
        .expect("auth manager");
    config.model_catalog = Some(catalog());
    let first =
        ResolvedModelContext::resolve(&config, auth.clone(), Some("deleted-models.json".into()))
            .expect("first context");
    config.model_catalog.as_mut().expect("catalog").models[0].context_window = Some(96_000);
    let second =
        ResolvedModelContext::resolve(&config, auth.clone(), None).expect("second context");
    let model = "shared-model-name";
    let first_info = first
        .models_manager
        .get_model_info(model, &config.to_models_manager_config())
        .await;
    let second_info = second
        .models_manager
        .get_model_info(model, &config.to_models_manager_config())
        .await;
    assert_eq!(first_info.context_window, Some(32_000));
    assert_eq!(second_info.context_window, Some(96_000));
    assert_ne!(
        first.snapshot.catalog_digest,
        second.snapshot.catalog_digest
    );
    let persisted = serde_json::to_vec(&first.snapshot).expect("encode durable state");
    let snapshot = serde_json::from_slice(&persisted).expect("restore durable state");
    let restored = ResolvedModelContext::from_snapshot(&config, auth.clone(), snapshot)
        .expect("restore without source file");
    assert_eq!(
        restored
            .models_manager
            .get_model_info(model, &config.to_models_manager_config())
            .await
            .context_window,
        Some(32_000)
    );
    let mut corrupt = first.snapshot.clone();
    corrupt.catalog_digest = Some("invalid".into());
    assert!(ResolvedModelContext::from_snapshot(&config, auth.clone(), corrupt).is_err());
    config.model_provider.base_url = Some("http://changed-provider.invalid/v1".into());
    assert!(ResolvedModelContext::from_snapshot(&config, auth, first.snapshot.clone()).is_err());
}

#[test]
fn route_fingerprint_does_not_serialize_credentials_and_detects_endpoint_changes() {
    let mut provider = ModelProviderInfo {
        base_url: Some("https://provider.invalid/v1".into()),
        experimental_bearer_token: Some("first-secret".into()),
        ..Default::default()
    };
    let first = route_fingerprint("kimi", &provider).expect("fingerprint");
    provider.experimental_bearer_token = Some("rotated-secret".into());
    assert_eq!(
        route_fingerprint("kimi", &provider).expect("fingerprint"),
        first
    );
    provider.base_url = Some("https://another-provider.invalid/v1".into());
    assert_ne!(
        route_fingerprint("kimi", &provider).expect("fingerprint"),
        first
    );
}

#[tokio::test]
async fn durable_catalog_objects_are_verified_and_missing_objects_do_not_fall_back() {
    let home = tempfile::tempdir().expect("temporary home");
    let mut config = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .build()
        .await
        .expect("config");
    let auth = AuthManager::shared_from_config(&config, false)
        .await
        .expect("auth manager");
    config.model_catalog = Some(catalog());
    let context = ResolvedModelContext::resolve(&config, auth.clone(), Some("removed.json".into()))
        .expect("context");
    config.resolved_model_context = Some(context.clone());
    require_store_version(&config).expect("commit object and reader version");
    let saved = context.persistence_snapshot(&config);
    assert!(saved.catalog.is_none());
    let restored = ResolvedModelContext::from_snapshot(&config, auth.clone(), saved.clone())
        .expect("load immutable object");
    assert_eq!(restored.snapshot.catalog, context.snapshot.catalog);
    let path = catalog_object_path(&config, saved.catalog_digest.as_deref().expect("digest"))
        .expect("object path");
    std::fs::write(&path, b"corrupt").expect("corrupt object");
    assert!(ResolvedModelContext::from_snapshot(&config, auth.clone(), saved.clone()).is_err());
    std::fs::remove_file(path).expect("remove object");
    assert!(ResolvedModelContext::from_snapshot(&config, auth, saved).is_err());
    std::fs::write(
        config
            .codex_home
            .join(codex_protocol::model_context::STORE_REQUIREMENT_FILE),
        b"2\n",
    )
    .expect("future reader requirement");
    assert!(require_store_version(&config).is_err());
}

#[tokio::test]
async fn custom_catalog_picker_does_not_inherit_openai_account_filtering() {
    let home = tempfile::tempdir().expect("temporary home");
    let mut config = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .build()
        .await
        .expect("config");
    let auth = AuthManager::shared_from_config(&config, false)
        .await
        .expect("auth manager");
    config.model_provider_id = "custom".into();
    config.model_provider.name = "Custom".into();
    config.model_provider.requires_openai_auth = false;
    let mut catalog = catalog();
    catalog.models[0].supported_in_api = false;
    config.model_catalog = Some(catalog);
    let context = ResolvedModelContext::resolve(&config, auth, None).expect("custom context");
    let models = context
        .models_manager
        .list_models(
            codex_models_manager::manager::RefreshStrategy::Offline,
            config.http_client_factory(),
        )
        .await;
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].model, "shared-model-name");
}

#[test]
fn public_tenant_routing_changes_identity_while_host_credential_rotation_does_not() {
    let mut provider = ModelProviderInfo {
        http_headers: Some(std::collections::HashMap::from([
            ("Authorization".into(), "first-secret".into()),
            ("OpenAI-Project".into(), "project-a".into()),
            ("version".into(), "0.164.0".into()),
        ])),
        ..Default::default()
    };
    let first = route_fingerprint("custom", &provider).expect("route");
    provider
        .http_headers
        .as_mut()
        .expect("headers")
        .insert("version".into(), "0.165.0".into());
    assert_eq!(
        route_fingerprint("custom", &provider).expect("client upgrade"),
        first
    );
    provider
        .http_headers
        .as_mut()
        .expect("headers")
        .insert("Authorization".into(), "second-secret".into());
    assert_eq!(
        route_fingerprint("custom", &provider).expect("rotated credential"),
        first
    );
    provider
        .http_headers
        .as_mut()
        .expect("headers")
        .insert("OpenAI-Project".into(), "project-b".into());
    assert_ne!(
        route_fingerprint("custom", &provider).expect("new tenant"),
        first
    );
}

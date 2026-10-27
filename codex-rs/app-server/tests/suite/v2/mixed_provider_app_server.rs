//! Deterministic mixed-provider integration; this does not qualify a live Kimi service.

use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_repeating_assistant;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ModelContextResolveParams;
use codex_app_server_protocol::ModelContextResolveResponse;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::UserInput;
use codex_protocol::openai_models::ModelMessages;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use tempfile::TempDir;

fn write_catalog(path: &std::path::Path, context_window: i64, marker: &str) -> Result<()> {
    let mut model = codex_models_manager::bundled_models_response()?
        .models
        .remove(0);
    model.slug = "same-model".into();
    model.visibility = ModelVisibility::List;
    model.context_window = Some(context_window);
    model.max_context_window = Some(256_000);
    model.model_messages = Some(ModelMessages {
        instructions_template: Some(marker.into()),
        ..Default::default()
    });
    std::fs::write(
        path,
        serde_json::to_vec(&ModelsResponse {
            models: vec![model],
        })?,
    )?;
    Ok(())
}

async fn resolve(
    server: &mut TestAppServer,
    provider: &str,
    catalog: &std::path::Path,
) -> Result<ModelContextResolveResponse> {
    server
        .request(|request_id| ClientRequest::ModelContextResolve {
            request_id,
            params: ModelContextResolveParams {
                model_provider: Some(provider.into()),
                model: Some("same-model".into()),
                model_catalog_json: Some(catalog.display().to_string()),
                ..Default::default()
            },
        })
        .await
}

#[tokio::test]
async fn contexts_keep_routing_and_instructions_independent_after_catalog_deletion() -> Result<()> {
    let first = create_mock_responses_server_repeating_assistant("first provider").await;
    let second = create_mock_responses_server_repeating_assistant("second provider").await;
    let home = TempDir::new()?;
    let first_catalog = home.path().join("first.json");
    let second_catalog = home.path().join("second.json");
    write_catalog(&first_catalog, 32_000, "FIRST_CONTEXT_INSTRUCTIONS")?;
    write_catalog(&second_catalog, 96_000, "SECOND_CONTEXT_INSTRUCTIONS")?;
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            r#"
model_provider = "first"
model = "same-model"
model_catalog_json = {first_catalog:?}
[features]
code_mode = false
code_mode_host = false
enable_request_compression = false
[model_providers.first]
name = "First"
base_url = "{first_url}/v1"
wire_api = "responses"
requires_openai_auth = false
[model_providers.second]
name = "Second"
base_url = "{second_url}/v1"
wire_api = "responses"
requires_openai_auth = false
"#,
            first_catalog = first_catalog.display().to_string(),
            first_url = first.uri(),
            second_url = second.uri()
        ),
    )?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let context_a = resolve(&mut server, "first", &first_catalog).await?;
    let context_b = resolve(&mut server, "second", &second_catalog).await?;
    assert_ne!(
        context_a.context.catalog_digest,
        context_b.context.catalog_digest
    );
    std::fs::remove_file(&second_catalog)?;
    let list: ModelListResponse = server
        .request(|request_id| ClientRequest::ModelList {
            request_id,
            params: ModelListParams {
                model_context_id: Some(context_b.model_context_id.clone()),
                ..Default::default()
            },
        })
        .await?;
    assert_eq!(list.data.len(), 1);
    assert_eq!(list.data[0].model, "same-model");
    let thread_a = server
        .start_thread(ThreadStartParams {
            model_context_id: Some(context_a.model_context_id),
            ..Default::default()
        })
        .await?;
    let thread_b = server
        .start_thread(ThreadStartParams {
            model_context_id: Some(context_b.model_context_id),
            ..Default::default()
        })
        .await?;
    assert_eq!(thread_a.model_provider, "first");
    assert_eq!(thread_b.model_provider, "second");
    for thread in [&thread_a, &thread_b] {
        server
            .start_turn_and_wait_for_completion(TurnStartParams {
                thread_id: thread.thread.id.clone(),
                input: vec![UserInput::Text {
                    text: "reply".into(),
                    text_elements: Vec::new(),
                }],
                ..Default::default()
            })
            .await?;
    }
    for (endpoint, marker, other) in [
        (
            &first,
            "FIRST_CONTEXT_INSTRUCTIONS",
            "SECOND_CONTEXT_INSTRUCTIONS",
        ),
        (
            &second,
            "SECOND_CONTEXT_INSTRUCTIONS",
            "FIRST_CONTEXT_INSTRUCTIONS",
        ),
    ] {
        let requests = endpoint
            .received_requests()
            .await
            .expect("recorded requests");
        let requests: Vec<_> = requests
            .iter()
            .filter(|request| request.url.path().ends_with("/responses"))
            .collect();
        assert!(!requests.is_empty());
        for request in requests {
            let body: serde_json::Value = serde_json::from_slice(&request.body)?;
            assert_eq!(body["model"], "same-model");
            let serialized = body.to_string();
            assert!(
                serialized.contains(marker),
                "missing selected catalog instructions"
            );
            assert!(
                !serialized.contains(other),
                "another catalog leaked into the request"
            );
        }
    }
    // A scoped request must not change the legacy default catalog.
    let default_list: ModelListResponse = server
        .request(|request_id| ClientRequest::ModelList {
            request_id,
            params: ModelListParams::default(),
        })
        .await?;
    assert_eq!(default_list.data[0].model, "same-model");
    assert!(server.shutdown_gracefully().await?.success());
    std::fs::remove_file(&first_catalog)?;
    let mut restarted = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let resumed: codex_app_server_protocol::ThreadResumeResponse = restarted
        .request(|request_id| ClientRequest::ThreadResume {
            request_id,
            params: ThreadResumeParams {
                thread_id: thread_b.thread.id.clone(),
                ..Default::default()
            },
        })
        .await?;
    assert_eq!(resumed.model_provider, "second");
    assert_eq!(
        resumed
            .model_context
            .as_ref()
            .and_then(|context| context.catalog_digest.as_ref()),
        thread_b
            .model_context
            .as_ref()
            .and_then(|context| context.catalog_digest.as_ref())
    );
    restarted
        .start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: resumed.thread.id,
            input: vec![UserInput::Text {
                text: "reply after restart".into(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn caller_cannot_combine_a_handle_with_unresolved_model_choices() -> Result<()> {
    let home = TempDir::new()?;
    let catalog = home.path().join("models.json");
    write_catalog(&catalog, 32_000, "CONTEXT_INSTRUCTIONS")?;
    std::fs::write(
        home.path().join("config.toml"),
        "model_provider = 'ollama'\n",
    )?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let context = resolve(&mut server, "ollama", &catalog).await?;
    let request = server
        .send_request(
            "thread/start",
            Some(serde_json::to_value(ThreadStartParams {
                model_context_id: Some(context.model_context_id),
                model_provider: Some("openai".into()),
                ..Default::default()
            })?),
        )
        .await?;
    let error = server
        .read_stream_until_error_message(codex_app_server_protocol::RequestId::Integer(request))
        .await?;
    assert_eq!(
        error.error.data.expect("reason")["reason"],
        "modelContextConflict"
    );
    Ok(())
}

#[tokio::test]
async fn a_new_provider_uses_its_visible_default_instead_of_another_providers_model() -> Result<()>
{
    let home = TempDir::new()?;
    let catalog_path = home.path().join("target.json");
    write_catalog(&catalog_path, 32_000, "TARGET_CONTEXT")?;
    let mut catalog: ModelsResponse = serde_json::from_slice(&std::fs::read(&catalog_path)?)?;
    catalog.models[0].slug = "visible-target".into();
    let mut hidden = catalog.models[0].clone();
    hidden.slug = "hidden-target".into();
    hidden.visibility = ModelVisibility::Hide;
    hidden.priority = -100;
    catalog.models.insert(0, hidden);
    std::fs::write(&catalog_path, serde_json::to_vec(&catalog)?)?;
    std::fs::write(
        home.path().join("config.toml"),
        r#"
model_provider = "ollama"
model = "another-providers-model"
[model_providers.custom]
name = "Custom"
base_url = "http://127.0.0.1:1/v1"
wire_api = "responses"
"#,
    )?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let context: ModelContextResolveResponse = server
        .request(|request_id| ClientRequest::ModelContextResolve {
            request_id,
            params: ModelContextResolveParams {
                model_provider: Some("custom".into()),
                model_catalog_json: Some(catalog_path.display().to_string()),
                ..Default::default()
            },
        })
        .await?;
    assert_eq!(context.model, "visible-target");
    Ok(())
}

#[tokio::test]
async fn native_provider_catalogs_are_scoped_and_reject_unknown_models() -> Result<()> {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let endpoint = MockServer::start().await;
    let home = TempDir::new()?;
    let file = home.path().join("fixture.json");
    write_catalog(&file, 48_000, "REMOTE_CATALOG_MODEL")?;
    let catalog: ModelsResponse = serde_json::from_slice(&std::fs::read(file)?)?;
    Mock::given(method("GET"))
        .and(path("/catalog"))
        .and(header("Authorization", "Bearer catalog-test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(catalog))
        .mount(&endpoint)
        .await;
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            r#"
model_provider = "catalog"
[features]
api_key_model_discovery = true
[model_providers.catalog]
name = "Catalog fixture"
base_url = "{uri}/v1"
model_catalog_url = "{uri}/catalog"
env_key = "MIXED_PROVIDER_TEST_KEY"
wire_api = "responses"
"#,
            uri = endpoint.uri()
        ),
    )?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .with_env_overrides(&[("MIXED_PROVIDER_TEST_KEY", Some("catalog-test-key"))])
        .build_initialized()
        .await?;
    let context: ModelContextResolveResponse = server
        .request(|request_id| ClientRequest::ModelContextResolve {
            request_id,
            params: ModelContextResolveParams {
                model: Some("same-model".into()),
                ..Default::default()
            },
        })
        .await?;
    assert_eq!(context.model_provider, "catalog");
    assert_eq!(context.model, "same-model");
    assert!(context.context.catalog_digest.is_none());
    let request = server
        .send_request(
            "modelContext/resolve",
            Some(serde_json::json!({"model":"missing-model"})),
        )
        .await?;
    let error = server
        .read_stream_until_error_message(codex_app_server_protocol::RequestId::Integer(request))
        .await?;
    assert_eq!(
        error.error.data.expect("reason")["reason"],
        "modelSelectionUnavailable"
    );
    // Attaching and subsequent model-change APIs must enforce the same authoritative list.
    let request = server
        .send_request(
            "thread/start",
            Some(serde_json::json!({
                "modelContextId": context.model_context_id, "model": "missing-model"
            })),
        )
        .await?;
    let error = server
        .read_stream_until_error_message(codex_app_server_protocol::RequestId::Integer(request))
        .await?;
    assert_eq!(
        error.error.data.expect("reason")["reason"],
        "modelSelectionUnavailable"
    );
    let thread = server
        .start_thread(ThreadStartParams {
            model_context_id: Some(context.model_context_id),
            ..Default::default()
        })
        .await?;
    for (method, params) in [
        (
            "turn/start",
            serde_json::json!({"threadId": thread.thread.id, "model":"missing-model", "input":[]}),
        ),
        (
            "thread/settings/update",
            serde_json::json!({"threadId": thread.thread.id, "model":"missing-model"}),
        ),
        (
            "turn/settings/update",
            serde_json::json!({"threadId": thread.thread.id, "turnId":"unused", "model":"missing-model"}),
        ),
    ] {
        let request = server.send_request(method, Some(params)).await?;
        let error = server
            .read_stream_until_error_message(codex_app_server_protocol::RequestId::Integer(request))
            .await?;
        assert_eq!(
            error.error.data.expect("reason")["reason"],
            "modelSelectionUnavailable",
            "{method}"
        );
    }
    let discovery_context: ModelContextResolveResponse = server
        .request(|request_id| ClientRequest::ModelContextResolve {
            request_id,
            params: ModelContextResolveParams {
                model: Some("same-model".into()),
                ..Default::default()
            },
        })
        .await?;
    let requests_before = endpoint
        .received_requests()
        .await
        .expect("native requests")
        .len();
    for config in [
        serde_json::json!({"features.api_key_model_discovery": false}),
        serde_json::json!({"features": {"api_key_model_discovery": false}}),
    ] {
        let request = server
            .send_request(
                "thread/start",
                Some(serde_json::json!({
                    "modelContextId": discovery_context.model_context_id, "config": config
                })),
            )
            .await?;
        let error = server
            .read_stream_until_error_message(codex_app_server_protocol::RequestId::Integer(request))
            .await?;
        assert_eq!(
            error.error.data.expect("reason")["reason"],
            "modelContextConflict"
        );
        assert_eq!(
            endpoint
                .received_requests()
                .await
                .expect("native requests")
                .len(),
            requests_before,
            "a conflicting opt-out must be rejected before discovery"
        );
    }
    let listed: ModelListResponse = server
        .request(|request_id| ClientRequest::ModelList {
            request_id,
            params: ModelListParams {
                model_context_id: Some(discovery_context.model_context_id.clone()),
                ..Default::default()
            },
        })
        .await?;
    assert!(
        listed.data.iter().any(|model| model.model == "same-model"),
        "rejected overrides must not mutate the resolved manager"
    );
    let compatible = server
        .start_thread(ThreadStartParams {
            model_context_id: Some(discovery_context.model_context_id),
            config: Some(std::collections::HashMap::from([(
                "features.api_key_model_discovery".into(),
                serde_json::json!(true),
            )])),
            ..Default::default()
        })
        .await?;
    assert_eq!(compatible.model, "same-model");
    // Resolve a still-pending context before revoking this provider, then prove that
    // rejected attachment/model-change requests perform no native discovery.
    let pending: ModelContextResolveResponse = server
        .request(|request_id| ClientRequest::ModelContextResolve {
            request_id,
            params: ModelContextResolveParams {
                model: Some("same-model".into()),
                ..Default::default()
            },
        })
        .await?;
    let requests_before = endpoint
        .received_requests()
        .await
        .expect("recorded native discovery")
        .len();
    std::fs::write(
        home.path().join("requirements.toml"),
        "model_provider = 'openai'\n",
    )?;
    for (method, params) in [
        (
            "thread/start",
            serde_json::json!({"modelContextId":pending.model_context_id}),
        ),
        (
            "turn/start",
            serde_json::json!({"threadId":thread.thread.id, "model":"same-model", "input":[]}),
        ),
        (
            "thread/settings/update",
            serde_json::json!({"threadId":thread.thread.id, "model":"same-model"}),
        ),
        (
            "turn/settings/update",
            serde_json::json!({"threadId":thread.thread.id, "turnId":"unused", "model":"same-model"}),
        ),
    ] {
        let request = server.send_request(method, Some(params)).await?;
        let error = server
            .read_stream_until_error_message(codex_app_server_protocol::RequestId::Integer(request))
            .await?;
        assert!(
            error
                .error
                .message
                .contains("required model provider settings changed"),
            "{method}: {error:?}"
        );
        assert_eq!(
            endpoint
                .received_requests()
                .await
                .expect("recorded native discovery")
                .len(),
            requests_before,
            "{method} must check current policy before provider I/O"
        );
    }
    Ok(())
}

#[tokio::test]
async fn cold_resume_replaces_an_obsolete_or_missing_catalog_under_current_requirements()
-> Result<()> {
    use codex_app_server_protocol::ModelContextIntent;
    let endpoint = create_mock_responses_server_repeating_assistant("persist this thread").await;
    let home = TempDir::new()?;
    let original = home.path().join("original.json");
    let replacement = home.path().join("replacement.json");
    write_catalog(&original, 32_000, "OLD_CATALOG")?;
    write_catalog(&replacement, 96_000, "REPLACEMENT_CATALOG")?;
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            r#"
model_provider = "fixture"
model = "same-model"
model_catalog_json = {catalog:?}
[features]
code_mode = false
code_mode_host = false
[model_providers.fixture]
name = "Fixture"
base_url = "{base_url}/v1"
wire_api = "responses"
"#,
            catalog = original.display().to_string(),
            base_url = endpoint.uri()
        ),
    )?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let initial = resolve(&mut server, "fixture", &original).await?;
    let digest = initial
        .context
        .catalog_digest
        .expect("saved catalog digest");
    let thread = server
        .start_thread(ThreadStartParams {
            model_context_id: Some(initial.model_context_id),
            ..Default::default()
        })
        .await?;
    server
        .start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread.thread.id.clone(),
            input: vec![UserInput::Text {
                text: "persist this thread".into(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    assert!(server.shutdown_gracefully().await?.success());
    // Both an obsolete policy-bound object and a missing object must allow an explicit
    // compliant replacement. Resolution alone does not load/modify the source thread.
    for remove_saved_object in [false, true] {
        if remove_saved_object {
            std::fs::remove_file(
                home.path()
                    .join("model-contexts")
                    .join(format!("{digest}.json")),
            )?;
        }
        std::fs::write(
            home.path().join("requirements.toml"),
            format!(
                "model_catalog_json = {:?}\n",
                replacement.display().to_string()
            ),
        )?;
        let mut restarted = TestAppServer::builder()
            .with_codex_home(home.path())
            .build_initialized()
            .await?;
        let request = restarted
            .send_request(
                "modelContext/resolve",
                Some(serde_json::json!({
                    "intent":"resume", "threadId":thread.thread.id, "modelCatalogJson":original
                })),
            )
            .await?;
        let denied = restarted
            .read_stream_until_error_message(codex_app_server_protocol::RequestId::Integer(request))
            .await?;
        assert!(
            denied.error.message.contains("managed requirements"),
            "replacement must still satisfy current policy: {denied:?}"
        );
        let context: ModelContextResolveResponse = restarted
            .request(|request_id| ClientRequest::ModelContextResolve {
                request_id,
                params: ModelContextResolveParams {
                    intent: ModelContextIntent::Resume,
                    thread_id: Some(thread.thread.id.clone()),
                    model_catalog_json: Some(replacement.display().to_string()),
                    ..Default::default()
                },
            })
            .await?;
        assert_ne!(
            context.context.catalog_digest.as_deref(),
            Some(digest.as_str())
        );
        if remove_saved_object {
            let resumed: codex_app_server_protocol::ThreadResumeResponse = restarted
                .request(|request_id| ClientRequest::ThreadResume {
                    request_id,
                    params: ThreadResumeParams {
                        thread_id: thread.thread.id.clone(),
                        model_context_id: Some(context.model_context_id),
                        ..Default::default()
                    },
                })
                .await?;
            assert_eq!(resumed.model, "same-model");
            assert_eq!(
                resumed
                    .model_context
                    .expect("restored context")
                    .catalog_digest,
                context.context.catalog_digest
            );
        }
        assert!(restarted.shutdown_gracefully().await?.success());
    }
    Ok(())
}

#[cfg(unix)]
mod local_clients {
    use super::*;
    use futures::SinkExt;
    use futures::StreamExt;
    use serde_json::Value;
    use serde_json::json;
    use tokio::net::UnixStream;
    use tokio::time::Duration;
    use tokio::time::timeout;
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::Message;

    type Client = WebSocketStream<UnixStream>;

    async fn call(client: &mut Client, id: i64, method: &str, params: Value) -> Result<Value> {
        client
            .send(Message::Text(
                json!({"id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .await?;
        timeout(Duration::from_secs(15), async {
            while let Some(message) = client.next().await {
                if let Message::Text(text) = message? {
                    let response: Value = serde_json::from_str(&text)?;
                    if response.get("id") == Some(&json!(id)) {
                        return Ok(response);
                    }
                }
            }
            anyhow::bail!("connection closed before response")
        })
        .await?
    }

    async fn connect(path: &std::path::Path) -> Result<Client> {
        let stream = UnixStream::connect(path).await?;
        let (mut client, _) = tokio_tungstenite::client_async("ws://localhost/", stream).await?;
        let initialized = call(
            &mut client,
            1,
            "initialize",
            json!({
                "clientInfo":{"name":"mixed-provider-test","version":"1"},
                "capabilities":{"experimentalApi":true}
            }),
        )
        .await?;
        assert_eq!(
            initialized["result"]["serverCapabilities"]["mixedProviderAppServer"],
            1
        );
        client
            .send(Message::Text(
                json!({"method":"initialized"}).to_string().into(),
            ))
            .await?;
        Ok(client)
    }

    async fn wait_completed(client: &mut Client, thread_id: &str) -> Result<()> {
        timeout(Duration::from_secs(20), async {
            while let Some(message) = client.next().await {
                if let Message::Text(text) = message? {
                    let event: Value = serde_json::from_str(&text)?;
                    if event["method"] == "turn/completed"
                        && event["params"]["threadId"] == thread_id
                    {
                        assert_eq!(event["params"]["turn"]["status"], "completed");
                        return Ok(());
                    }
                }
            }
            anyhow::bail!("connection closed before turn completion")
        })
        .await?
    }

    #[tokio::test]
    async fn two_local_connections_keep_context_ownership_and_concurrent_provider_routes()
    -> Result<()> {
        let first = create_mock_responses_server_repeating_assistant("first").await;
        let second = create_mock_responses_server_repeating_assistant("second").await;
        let home = TempDir::new()?;
        let a = home.path().join("a.json");
        let b = home.path().join("b.json");
        write_catalog(&a, 32_000, "LOCAL_FIRST_CONTEXT")?;
        write_catalog(&b, 96_000, "LOCAL_SECOND_CONTEXT")?;
        std::fs::write(
            home.path().join("config.toml"),
            format!(
                r#"
model_provider = "first"
model_catalog_json = {catalog:?}
[features]
code_mode = false
code_mode_host = false
enable_request_compression = false
[model_providers.first]
name = "First"
base_url = "{first}/v1"
wire_api = "responses"
[model_providers.kimi]
name = "Kimi fixture"
base_url = "{second}/v1"
wire_api = "responses"
"#,
                catalog = a.display().to_string(),
                first = first.uri(),
                second = second.uri()
            ),
        )?;
        let socket = home.path().join("server.sock");
        let listen = format!("unix://{}", socket.display());
        let mut process = TestAppServer::builder()
            .with_codex_home(home.path())
            .without_managed_config()
            .with_args(&["--listen", &listen])
            .build()
            .await?;
        timeout(Duration::from_secs(15), async {
            while !socket.exists() {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await?;
        let mut client_a = connect(&socket).await?;
        let mut client_b = connect(&socket).await?;
        let (resolved_a, resolved_b) = tokio::try_join!(
            call(
                &mut client_a,
                2,
                "modelContext/resolve",
                json!({"modelProvider":"first","modelCatalogJson":a,"model":"same-model"})
            ),
            call(
                &mut client_b,
                2,
                "modelContext/resolve",
                json!({"modelProvider":"kimi","modelCatalogJson":b,"model":"same-model"})
            )
        )?;
        assert!(resolved_a.get("error").is_none(), "{resolved_a}");
        assert!(resolved_b.get("error").is_none(), "{resolved_b}");
        let id_a = resolved_a["result"]["modelContextId"]
            .as_str()
            .expect("context A");
        let id_b = resolved_b["result"]["modelContextId"]
            .as_str()
            .expect("context B");
        let stolen = call(
            &mut client_b,
            3,
            "model/list",
            json!({"modelContextId":id_a}),
        )
        .await?;
        assert_eq!(stolen["error"]["data"]["reason"], "modelContextUnavailable");
        let (thread_a, thread_b) =
            tokio::try_join!(
                async {
                    // Queue both requests before reading either response. Startup runs in
                    // background tasks, so a lookup-only implementation attaches twice.
                    for id in [4, 40] {
                        client_a.send(Message::Text(json!({
                        "id": id, "method":"thread/start", "params":{"modelContextId":id_a}
                    }).to_string().into())).await?;
                    }
                    timeout(Duration::from_secs(20), async {
                        let mut responses = Vec::new();
                        while let Some(message) = client_a.next().await {
                            if let Message::Text(text) = message? {
                                let response: Value = serde_json::from_str(&text)?;
                                if [json!(4), json!(40)].contains(&response["id"]) {
                                    responses.push(response);
                                    if responses.len() == 2 {
                                        assert_eq!(
                                            responses
                                                .iter()
                                                .filter(|response| response.get("result").is_some())
                                                .count(),
                                            1,
                                            "{responses:?}"
                                        );
                                        let error = responses
                                            .iter()
                                            .find(|response| response.get("error").is_some())
                                            .expect("duplicate rejected");
                                        assert_eq!(
                                            error["error"]["data"]["reason"],
                                            "modelContextUnavailable"
                                        );
                                        return Ok::<_, anyhow::Error>(
                                            responses
                                                .into_iter()
                                                .find(|response| response.get("result").is_some())
                                                .expect("one successful start"),
                                        );
                                    }
                                }
                            }
                        }
                        anyhow::bail!("connection closed before both starts completed")
                    })
                    .await?
                },
                call(
                    &mut client_b,
                    4,
                    "thread/start",
                    json!({"modelContextId":id_b})
                )
            )?;
        assert_eq!(thread_a["result"]["modelProvider"], "first");
        assert_eq!(thread_b["result"]["modelProvider"], "kimi");
        let a_id = thread_a["result"]["thread"]["id"]
            .as_str()
            .expect("thread A");
        let b_id = thread_b["result"]["thread"]["id"]
            .as_str()
            .expect("thread B");
        let input = json!([{"type":"text","text":"reply","text_elements":[]}]);
        let (turn_a, turn_b) = tokio::try_join!(
            call(
                &mut client_a,
                5,
                "turn/start",
                json!({"threadId":a_id,"input":input})
            ),
            call(
                &mut client_b,
                5,
                "turn/start",
                json!({"threadId":b_id,"input":input})
            )
        )?;
        assert!(turn_a.get("error").is_none(), "{turn_a}");
        assert!(turn_b.get("error").is_none(), "{turn_b}");
        tokio::try_join!(
            wait_completed(&mut client_a, a_id),
            wait_completed(&mut client_b, b_id)
        )?;
        for (endpoint, own, other) in [
            (&first, "LOCAL_FIRST_CONTEXT", "LOCAL_SECOND_CONTEXT"),
            (&second, "LOCAL_SECOND_CONTEXT", "LOCAL_FIRST_CONTEXT"),
        ] {
            let requests = endpoint
                .received_requests()
                .await
                .expect("recorded requests");
            let request = requests
                .iter()
                .find(|request| request.url.path().ends_with("/responses"))
                .expect("provider request");
            let body = std::str::from_utf8(&request.body)?;
            assert!(body.contains(own));
            assert!(!body.contains(other));
        }
        client_a.close(None).await?;
        client_b.close(None).await?;
        process.send_sigterm()?;
        assert!(process.wait_for_exit().await?.success());
        Ok(())
    }
}

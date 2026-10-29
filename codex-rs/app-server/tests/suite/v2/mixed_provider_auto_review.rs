//! Approval execution across mixed-provider resume, using only local mock endpoints.

use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_repeating_assistant;
use codex_app_server_protocol::ApprovalsReviewer;
use codex_app_server_protocol::AskForApproval;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ModelContextIntent;
use codex_app_server_protocol::ModelContextResolveParams;
use codex_app_server_protocol::ModelContextResolveResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput;
use codex_features::Feature;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::ToolMode;
use core_test_support::responses;
use serde_json::json;
use std::path::Path;
use tempfile::TempDir;
use test_case::test_case;

const MODEL: &str = "k3.1-pro";

// Declarations can be top-level, incremental input items, or namespace children.
fn tool_declared(value: &serde_json::Value, kind: &str, name: &str) -> bool {
    match value {
        serde_json::Value::Object(fields) => {
            (value["type"] == kind && value["name"] == name)
                || fields
                    .values()
                    .any(|child| tool_declared(child, kind, name))
        }
        serde_json::Value::Array(items) => items.iter().any(|item| tool_declared(item, kind, name)),
        _ => false,
    }
}

fn write_catalog(path: &Path) -> Result<()> {
    let mut model = codex_models_manager::bundled_models_response()?
        .models
        .remove(0);
    model.slug = MODEL.to_owned();
    model.visibility = ModelVisibility::List;
    model.tool_mode = Some(ToolMode::Direct);
    model.model_messages = None;
    // A copied or stale override must not switch a custom provider's reviewer.
    model.auto_review_model_override = Some("codex-auto-review".to_owned());
    std::fs::write(
        path,
        serde_json::to_vec(&ModelsResponse {
            models: vec![model],
        })?,
    )?;
    Ok(())
}

fn write_config(home: &Path, default_url: &str, kimi_url: &str) -> Result<()> {
    MockResponsesConfig::new(default_url)
        .with_builtin_openai_provider()
        .with_model("gpt-5.4")
        .with_approval_policy("on-request")
        .enable_feature(Feature::GuardianApproval)
        .enable_feature(Feature::GuardianV2)
        .enable_feature(Feature::GuardianV2DecisionsComparison)
        .enable_feature(Feature::UnifiedExec)
        .disable_feature(Feature::ShellSnapshot)
        .disable_feature(Feature::CodeMode)
        .disable_feature(Feature::CodeModeOnly)
        .disable_feature(Feature::CodeModeHost)
        .disable_feature(Feature::EnableRequestCompression)
        .with_extra_config(&format!(
            r#"
[model_providers.kimi]
name = "Kimi fixture"
base_url = "{kimi_url}/v1"
wire_api = "responses"
requires_openai_auth = false
request_max_retries = 0
stream_max_retries = 0
"#
        ))
        .write(home)?;
    Ok(())
}

async fn start_server(home: &Path) -> Result<TestAppServer> {
    TestAppServer::builder()
        .with_codex_home(home)
        .without_managed_config()
        .with_env_overrides(&[
            ("OPENAI_API_KEY", Some("mixed-provider-test-key")),
            ("CODEX_API_KEY", None),
            ("CODEX_ACCESS_TOKEN", None),
        ])
        .build_initialized()
        .await
}

async fn resolve(
    server: &mut TestAppServer,
    thread: Option<&str>,
    catalog: Option<&Path>,
) -> Result<ModelContextResolveResponse> {
    server
        .request(|request_id| ClientRequest::ModelContextResolve {
            request_id,
            params: ModelContextResolveParams {
                intent: if thread.is_some() {
                    ModelContextIntent::Resume
                } else {
                    ModelContextIntent::Start
                },
                thread_id: thread.map(str::to_owned),
                model_provider: thread.is_none().then(|| "kimi".to_owned()),
                model: thread.is_none().then(|| MODEL.to_owned()),
                model_catalog_json: catalog.map(|path| path.display().to_string()),
                ..Default::default()
            },
        })
        .await
}

async fn resume(
    server: &mut TestAppServer,
    thread: &str,
    context: ModelContextResolveResponse,
) -> Result<ThreadResumeResponse> {
    let resumed: ThreadResumeResponse = server
        .request(|request_id| ClientRequest::ThreadResume {
            request_id,
            params: ThreadResumeParams {
                thread_id: thread.to_owned(),
                model_context_id: Some(context.model_context_id),
                ..Default::default()
            },
        })
        .await?;
    assert_eq!(resumed.model_provider, "kimi");
    assert_eq!(resumed.model, MODEL);
    assert_eq!(resumed.approvals_reviewer, ApprovalsReviewer::AutoReview);
    Ok(resumed)
}

async fn turn(server: &mut TestAppServer, thread: &str) -> Result<()> {
    let environments = Some(vec![server.auto_env_params()?]);
    let completed = server
        .start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread.to_owned(),
            environments,
            input: vec![UserInput::Text {
                text: "Create the requested test marker after reviewing the operation.".to_owned(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    assert_eq!(
        completed.turn.status,
        TurnStatus::Completed,
        "{completed:?}"
    );
    Ok(())
}

async fn review_turn(
    server: &mut TestAppServer,
    endpoint: &wiremock::MockServer,
    thread: &str,
    marker: &Path,
    decision: Option<bool>,
) -> Result<()> {
    endpoint.reset().await;
    let mut sequence = vec![responses::sse(vec![
        responses::ev_response_created("action"),
        responses::ev_exec_command_call_with_args(
            "reviewed-command",
            &json!({
                "cmd": format!("printf reviewed > '{}'", marker.display()),
                "sandbox_permissions": "require_escalated",
                "justification": "Create the explicitly requested test marker.",
                "login": false,
            }),
        ),
        responses::ev_completed("action"),
    ])];
    if let Some(allow) = decision {
        sequence.push(responses::sse(vec![
            responses::ev_response_created("review"),
            responses::ev_assistant_message(
                "assessment",
                &json!({"outcome": if allow { "allow" } else { "deny" }, "rationale":"fixture review"}).to_string(),
            ),
            responses::ev_completed("review"),
        ]));
    }
    sequence.push(responses::sse(vec![
        responses::ev_assistant_message("finished", "Done"),
        responses::ev_completed("done"),
    ]));
    let requests = responses::mount_sse_sequence(endpoint, sequence).await;
    turn(server, thread).await?;
    let recorded = requests.requests();
    assert_eq!(recorded.len(), if decision.is_some() { 3 } else { 2 });
    for request in &recorded {
        assert_eq!(request.body_json()["model"], MODEL);
    }
    if decision.is_some() {
        let reviewer = recorded[1].body_json();
        assert!(
            reviewer
                .pointer("/text/format/schema/properties/outcome")
                .is_some()
        );
        assert!(tool_declared(&reviewer, "function", "exec_command"));
        assert!(!tool_declared(&reviewer, "custom", "exec"));
    } else {
        let output = requests
            .function_call_output_text("reviewed-command")
            .expect("review error");
        assert!(output.contains("model_catalog_json"), "{output}");
        assert!(
            output.contains("requires its own model catalog"),
            "{output}"
        );
    }
    assert_eq!(marker.exists(), decision == Some(true));
    if marker.exists() {
        assert_eq!(std::fs::read_to_string(marker)?, "reviewed");
        std::fs::remove_file(marker)?;
    }
    Ok(())
}

#[tokio::test]
async fn kimi_reviews_with_its_own_model_before_and_after_cold_resume() -> Result<()> {
    let default_endpoint =
        create_mock_responses_server_repeating_assistant("Default provider").await;
    let kimi = responses::start_mock_server().await;
    let home = TempDir::new()?;
    let catalog = home.path().join("kimi.json");
    let marker = home.path().join("reviewed-marker");
    write_catalog(&catalog)?;
    write_config(home.path(), &default_endpoint.uri(), &kimi.uri())?;
    let mut server = start_server(home.path()).await?;
    let default_thread = server.start_thread(ThreadStartParams::default()).await?;
    turn(&mut server, &default_thread.thread.id).await?;
    let default_requests = default_endpoint.received_requests().await.unwrap().len();
    let context = resolve(&mut server, None, Some(&catalog)).await?;
    let digest = context
        .context
        .catalog_digest
        .clone()
        .expect("static catalog");
    let started = server
        .start_thread(ThreadStartParams {
            model_context_id: Some(context.model_context_id),
            approval_policy: Some(AskForApproval::OnRequest),
            approvals_reviewer: Some(ApprovalsReviewer::AutoReview),
            ..Default::default()
        })
        .await?;
    let id = started.thread.id;
    review_turn(&mut server, &kimi, &id, &marker, Some(true)).await?;
    assert!(server.shutdown_gracefully().await?.success());
    std::fs::remove_file(&catalog)?;
    let mut restarted = start_server(home.path()).await?;
    let context = resolve(&mut restarted, Some(&id), None).await?;
    assert_eq!(
        context.context.catalog_digest.as_deref(),
        Some(digest.as_str())
    );
    resume(&mut restarted, &id, context).await?;
    review_turn(&mut restarted, &kimi, &id, &marker, Some(false)).await?;
    review_turn(&mut restarted, &kimi, &id, &marker, Some(true)).await?;
    assert_eq!(
        default_endpoint.received_requests().await.unwrap().len(),
        default_requests
    );
    assert!(restarted.shutdown_gracefully().await?.success());
    Ok(())
}

#[tokio::test]
async fn legacy_kimi_review_requires_catalog_and_recovers_after_explicit_rebind() -> Result<()> {
    let default_endpoint =
        create_mock_responses_server_repeating_assistant("Default provider").await;
    let kimi = responses::start_mock_server().await;
    let home = TempDir::new()?;
    let catalog = home.path().join("kimi.json");
    let marker = home.path().join("legacy-marker");
    write_catalog(&catalog)?;
    write_config(home.path(), &default_endpoint.uri(), &kimi.uri())?;
    let mut server = start_server(home.path()).await?;
    // The old API records provider/model/settings without a bound catalog.
    let started = server
        .start_thread(ThreadStartParams {
            model_provider: Some("kimi".to_owned()),
            model: Some(MODEL.to_owned()),
            approvals_reviewer: Some(ApprovalsReviewer::AutoReview),
            ..Default::default()
        })
        .await?;
    let id = started.thread.id;
    responses::mount_sse_once(
        &kimi,
        responses::sse(vec![
            responses::ev_assistant_message("legacy", "Persist legacy thread"),
            responses::ev_completed("legacy"),
        ]),
    )
    .await;
    turn(&mut server, &id).await?;
    assert!(server.shutdown_gracefully().await?.success());
    let mut restarted = start_server(home.path()).await?;
    let context = resolve(&mut restarted, Some(&id), None).await?;
    assert_eq!(context.context.catalog_digest, None);
    resume(&mut restarted, &id, context).await?;
    review_turn(&mut restarted, &kimi, &id, &marker, None).await?;
    assert!(restarted.shutdown_gracefully().await?.success());
    let mut repaired = start_server(home.path()).await?;
    let context = resolve(&mut repaired, Some(&id), Some(&catalog)).await?;
    assert!(context.context.catalog_digest.is_some());
    resume(&mut repaired, &id, context).await?;
    review_turn(&mut repaired, &kimi, &id, &marker, Some(true)).await?;
    assert!(repaired.shutdown_gracefully().await?.success());
    Ok(())
}

fn write_review_catalog(path: &Path, policy: &str) -> Result<()> {
    write_catalog(path)?;
    let mut data: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    data["models"][0]["model_messages"] = json!({"auto_review":{"policy_template":policy}});
    std::fs::write(path, serde_json::to_vec(&data)?)?;
    Ok(())
}

fn set_startup_catalog(home: &Path, catalog: &Path, provider: Option<&str>) -> Result<()> {
    let config_path = home.join("config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
    let values = config.as_table_mut().expect("config table");
    values.insert(
        "model_catalog_json".to_owned(),
        catalog.display().to_string().into(),
    );
    if let Some(provider) = provider {
        values.insert("model_provider".to_owned(), provider.into());
        values.insert("model".to_owned(), MODEL.into());
    }
    std::fs::write(config_path, toml::to_string(&config)?)?;
    Ok(())
}

async fn assert_kimi_review_policy(endpoint: &wiremock::MockServer) -> Result<()> {
    let bodies = endpoint
        .received_requests()
        .await
        .expect("recorded provider requests")
        .into_iter()
        .map(|request| request.body_json::<serde_json::Value>())
        .collect::<Result<Vec<_>, _>>()?;
    let review = bodies
        .iter()
        .find(|body| {
            body.pointer("/text/format/schema/properties/outcome")
                .is_some()
        })
        .expect("review request")
        .to_string();
    assert!(review.contains("KIMI_PROVIDER_REVIEW_POLICY"));
    assert!(!review.contains("DEFAULT_PROVIDER_REVIEW_POLICY"));
    Ok(())
}

#[test_case(false; "inherited_default_catalog")]
#[test_case(true; "unbound_thread_catalog_override")]
#[tokio::test]
async fn legacy_matching_model_name_requires_runtime_catalog_binding(
    catalog_override: bool,
) -> Result<()> {
    let default_endpoint = create_mock_responses_server_repeating_assistant("default").await;
    let kimi = responses::start_mock_server().await;
    let home = TempDir::new()?;
    let default_catalog = home.path().join("default.json");
    let kimi_catalog = home.path().join("kimi.json");
    write_review_catalog(&default_catalog, "DEFAULT_PROVIDER_REVIEW_POLICY")?;
    write_review_catalog(&kimi_catalog, "KIMI_PROVIDER_REVIEW_POLICY")?;
    write_config(home.path(), &default_endpoint.uri(), &kimi.uri())?;
    set_startup_catalog(home.path(), &default_catalog, None)?;
    let mut server = start_server(home.path()).await?;
    let thread = server
        .start_thread(ThreadStartParams {
            model: Some(MODEL.to_owned()),
            model_provider: Some("kimi".to_owned()),
            approvals_reviewer: Some(ApprovalsReviewer::AutoReview),
            config: catalog_override.then(|| {
                std::collections::HashMap::from([(
                    "model_catalog_json".to_owned(),
                    json!(kimi_catalog),
                )])
            }),
            ..Default::default()
        })
        .await?;
    // Both catalogs advertise the same slug. Neither config presence nor a
    // successful lookup in the default manager proves that Kimi owns the metadata.
    let marker = home.path().join("marker");
    review_turn(&mut server, &kimi, &thread.thread.id, &marker, None).await?;
    assert!(server.shutdown_gracefully().await?.success());

    let mut repaired = start_server(home.path()).await?;
    let context = resolve(&mut repaired, Some(&thread.thread.id), Some(&kimi_catalog)).await?;
    resume(&mut repaired, &thread.thread.id, context).await?;
    review_turn(&mut repaired, &kimi, &thread.thread.id, &marker, Some(true)).await?;
    assert_kimi_review_policy(&kimi).await?;
    assert!(repaired.shutdown_gracefully().await?.success());
    Ok(())
}

#[tokio::test]
async fn legacy_startup_kimi_catalog_establishes_runtime_binding() -> Result<()> {
    let default_endpoint = create_mock_responses_server_repeating_assistant("default").await;
    let kimi = responses::start_mock_server().await;
    let home = TempDir::new()?;
    let catalog = home.path().join("kimi.json");
    write_review_catalog(&catalog, "KIMI_PROVIDER_REVIEW_POLICY")?;
    write_config(home.path(), &default_endpoint.uri(), &kimi.uri())?;
    set_startup_catalog(home.path(), &catalog, Some("kimi"))?;
    let mut server = start_server(home.path()).await?;
    let thread = server
        .start_thread(ThreadStartParams {
            approvals_reviewer: Some(ApprovalsReviewer::AutoReview),
            ..Default::default()
        })
        .await?;
    assert_eq!(thread.model_provider, "kimi");
    assert!(
        thread
            .model_context
            .as_ref()
            .and_then(|context| context.catalog_digest.as_ref())
            .is_some()
    );
    review_turn(
        &mut server,
        &kimi,
        &thread.thread.id,
        &home.path().join("marker"),
        Some(true),
    )
    .await?;
    assert_kimi_review_policy(&kimi).await?;
    assert!(server.shutdown_gracefully().await?.success());
    Ok(())
}

#[tokio::test]
async fn valid_thread_model_overrides_stale_daemon_default_without_losing_review() -> Result<()> {
    let default_endpoint = create_mock_responses_server_repeating_assistant("default").await;
    let kimi = responses::start_mock_server().await;
    let home = TempDir::new()?;
    let catalog = home.path().join("kimi.json");
    write_review_catalog(&catalog, "KIMI_PROVIDER_REVIEW_POLICY")?;
    write_config(home.path(), &default_endpoint.uri(), &kimi.uri())?;
    set_startup_catalog(home.path(), &catalog, Some("kimi"))?;
    let config_path = home.path().join("config.toml");
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
    config["model"] = "retired-kimi-model".into();
    std::fs::write(config_path, toml::to_string(&config)?)?;
    let mut server = start_server(home.path()).await?;

    // Building the provider binding must not make either the stale default or
    // another invalid explicit selection acceptable for execution.
    for model in [None, Some("another-unknown-model".to_owned())] {
        let request_id = server
            .send_thread_start_request_with_auto_env(ThreadStartParams {
                model,
                approvals_reviewer: Some(ApprovalsReviewer::AutoReview),
                ..Default::default()
            })
            .await?;
        let error = server
            .read_stream_until_error_message(codex_app_server_protocol::RequestId::Integer(
                request_id,
            ))
            .await?;
        assert!(
            error
                .error
                .message
                .contains("absent from the static model catalog"),
            "{error:?}"
        );
    }
    let thread = server
        .start_thread(ThreadStartParams {
            model: Some(MODEL.to_owned()),
            approvals_reviewer: Some(ApprovalsReviewer::AutoReview),
            ..Default::default()
        })
        .await?;
    assert_eq!(thread.model_provider, "kimi");
    assert_eq!(thread.model, MODEL);
    assert!(
        thread
            .model_context
            .as_ref()
            .and_then(|context| context.catalog_digest.as_ref())
            .is_some()
    );
    review_turn(
        &mut server,
        &kimi,
        &thread.thread.id,
        &home.path().join("valid-model-marker"),
        Some(true),
    )
    .await?;
    assert_kimi_review_policy(&kimi).await?;
    assert!(server.shutdown_gracefully().await?.success());
    Ok(())
}

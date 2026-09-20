//! Regression contract for recipient-based task delivery, without live provider calls.
//!
//! These handler tests inspect submitted communication, not the final Responses wire payload.
//! A non-OpenAI recipient must receive readable task content while queue-only messages must
//! retain their non-waking semantics. A passing test here does not prove provider compatibility.
//! Direct calls intentionally omit encrypted-argument metadata, which plaintext mode accepts under
//! the advertised schema contract.

use super::*;
use pretty_assertions::assert_eq;

#[derive(Clone, Copy, Debug)]
enum Delivery {
    Spawn,
    SendMessage,
    FollowupTask,
}

#[derive(Debug, PartialEq, Eq)]
struct TaskDelivery {
    plaintext_contains_task: bool,
    encrypted_content: Option<String>,
    trigger_turn: bool,
}

#[test_case::test_case("openai", "openai", Delivery::Spawn; "openai_to_openai_spawn")]
#[test_case::test_case("openai", "openai", Delivery::SendMessage; "openai_to_openai_sendmessage")]
#[test_case::test_case("openai", "openai", Delivery::FollowupTask; "openai_to_openai_followuptask")]
#[test_case::test_case("openai", "deepseek", Delivery::Spawn; "openai_to_deepseek_spawn")]
#[test_case::test_case("openai", "deepseek", Delivery::SendMessage; "openai_to_deepseek_sendmessage")]
#[test_case::test_case("openai", "deepseek", Delivery::FollowupTask; "openai_to_deepseek_followuptask")]
#[test_case::test_case("deepseek", "openai", Delivery::Spawn; "deepseek_to_openai_spawn")]
#[test_case::test_case("deepseek", "openai", Delivery::SendMessage; "deepseek_to_openai_sendmessage")]
#[test_case::test_case("deepseek", "openai", Delivery::FollowupTask; "deepseek_to_openai_followuptask")]
#[test_case::test_case("deepseek", "deepseek", Delivery::Spawn; "deepseek_to_deepseek_spawn")]
#[test_case::test_case("deepseek", "deepseek", Delivery::SendMessage; "deepseek_to_deepseek_sendmessage")]
#[test_case::test_case("deepseek", "deepseek", Delivery::FollowupTask; "deepseek_to_deepseek_followuptask")]
#[tokio::test]
async fn provider_delivery_matrix(parent: &str, child: &str, delivery: Delivery) {
    const TASK: &str = "Reply with exactly MATRIX_TASK.";
    let (mut session, mut turn) = make_session_and_context().await;
    let mut config = (*turn.config).clone();
    let mut deepseek = built_in_model_providers(/*openai_base_url*/ None)["ollama"].clone();
    deepseek.name = "DeepSeek".to_string();
    // The test manager captures submissions; this fixture never needs a real endpoint or key.
    deepseek.base_url = Some("http://127.0.0.1:1/v1".to_string());
    config
        .model_providers
        .insert("deepseek".to_string(), deepseek);
    config
        .model_providers
        .get_mut("openai")
        .expect("built-in OpenAI provider")
        .base_url = Some("http://127.0.0.1:1/v1".to_string());
    config.model_provider_id = parent.to_string();
    config.model_provider = config.model_providers[parent].clone();
    assert_eq!(config.model_provider.is_openai(), parent == "openai");
    config
        .features
        .enable(Feature::MultiAgentV2)
        .expect("enable MultiAgentV2");
    config.multi_agent_v2.task_payload = codex_protocol::protocol::MultiAgentTaskPayload::Plaintext;

    tokio::fs::create_dir_all(&config.codex_home)
        .await
        .expect("create role directory");
    let role_path = config.codex_home.as_path().join("matrix-recipient.toml");
    tokio::fs::write(&role_path, format!("model_provider = \"{child}\"\n"))
        .await
        .expect("write recipient provider override");
    config.agent_roles.insert(
        "matrix-recipient".to_string(),
        AgentRoleConfig {
            description: Some("Provider delivery matrix recipient".to_string()),
            config_file: Some(role_path),
            nickname_candidates: None,
        },
    );
    turn.provider = create_model_provider(config.model_provider.clone(), turn.auth_manager.clone());
    set_turn_config(&mut turn, config);
    let manager = thread_manager();
    let root = manager
        .start_thread(StartThreadOptions::new((*turn.config).clone()))
        .await
        .expect("start parent");
    session.services.agent_control = manager.agent_control();
    session.thread_id = root.thread_id;
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let before_spawn = manager.captured_ops().len();
    SpawnAgentHandlerV2::default()
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "spawn_agent",
            function_payload(json!({
                "message": TASK,
                "task_name": "matrix_worker",
                "agent_type": "matrix-recipient",
                "fork_turns": "none"
            })),
        ))
        .await
        .expect("spawn matrix recipient");
    let recipient = session
        .services
        .agent_control
        .resolve_agent_reference(session.thread_id, &turn.session_source, "matrix_worker")
        .await
        .expect("resolve recipient");
    let snapshot = manager
        .get_thread(recipient)
        .await
        .expect("recipient exists")
        .config_snapshot()
        .await;
    assert_eq!(snapshot.model_provider_id, child);

    // Messaging cases do not assert spawn delivery, so that assertion cannot mask their result.
    let first_op = match delivery {
        Delivery::Spawn => before_spawn,
        Delivery::SendMessage | Delivery::FollowupTask => manager.captured_ops().len(),
    };
    match delivery {
        Delivery::Spawn => {}
        Delivery::SendMessage => {
            SendMessageHandlerV2
                .handle(invocation(
                    session.clone(),
                    turn.clone(),
                    "send_message",
                    function_payload(json!({"target": "matrix_worker", "message": TASK})),
                ))
                .await
                .expect("send matrix message");
        }
        Delivery::FollowupTask => {
            FollowupTaskHandlerV2
                .handle(invocation(
                    session.clone(),
                    turn.clone(),
                    "followup_task",
                    function_payload(json!({"target": "matrix_worker", "message": TASK})),
                ))
                .await
                .expect("send matrix followup");
        }
    }

    let observed: Vec<TaskDelivery> = manager
        .captured_ops()
        .into_iter()
        .skip(first_op)
        .filter_map(|(id, op)| match op {
            Op::InterAgentCommunication { communication, .. } if id == recipient => {
                Some(TaskDelivery {
                    plaintext_contains_task: communication.content.contains(TASK),
                    encrypted_content: communication.encrypted_content,
                    trigger_turn: communication.trigger_turn,
                })
            }
            _ => None,
        })
        .collect();
    let expected = vec![TaskDelivery {
        plaintext_contains_task: true,
        encrypted_content: None,
        trigger_turn: matches!(delivery, Delivery::Spawn | Delivery::FollowupTask),
    }];
    assert_eq!(observed, expected, "{parent} -> {child}: {delivery:?}");
}

#[tokio::test]
async fn oversized_plaintext_spawn_is_rejected_before_allocating_child() {
    let (mut session, mut turn) = make_session_and_context().await;
    let mut config = (*turn.config).clone();
    config
        .features
        .enable(Feature::MultiAgentV2)
        .expect("enable MultiAgentV2");
    config.multi_agent_v2.task_payload = codex_protocol::protocol::MultiAgentTaskPayload::Plaintext;
    set_turn_config(&mut turn, config);

    let manager = thread_manager();
    let root = manager
        .start_thread(StartThreadOptions::new((*turn.config).clone()))
        .await
        .expect("start parent");
    session.services.agent_control = manager.agent_control();
    session.thread_id = root.thread_id;
    let session = Arc::new(session);
    let turn = Arc::new(turn);
    let before = manager.list_thread_ids().await;

    let error = SpawnAgentHandlerV2::default()
        .handle(invocation(
            session,
            turn,
            "spawn_agent",
            function_payload(json!({
                "message": "x".repeat(4097),
                "task_name": "oversized_worker",
                "fork_turns": "none",
            })),
        ))
        .await
        .err()
        .expect("oversized plaintext task should fail");

    assert_eq!(
        error.to_string(),
        "plaintext collaboration message exceeds the 4096-byte rendered limit"
    );
    assert_eq!(manager.list_thread_ids().await, before);
}

#[tokio::test]
async fn encrypted_mode_compatibility_parent_reserves_native_child_completion_envelope() {
    let (mut session, mut turn) = make_session_and_context().await;
    let mut config = (*turn.config).clone();
    let mut deepseek = built_in_model_providers(/*openai_base_url*/ None)["ollama"].clone();
    deepseek.name = "DeepSeek".to_string();
    deepseek.base_url = Some("http://127.0.0.1:1/v1".to_string());
    config
        .model_providers
        .insert("deepseek".to_string(), deepseek.clone());
    config.model_provider_id = "deepseek".to_string();
    config.model_provider = deepseek;
    config
        .features
        .enable(Feature::MultiAgentV2)
        .expect("enable MultiAgentV2");

    tokio::fs::create_dir_all(&config.codex_home)
        .await
        .expect("create role directory");
    let role_path = config.codex_home.as_path().join("native-recipient.toml");
    tokio::fs::write(&role_path, "model_provider = \"openai\"\n")
        .await
        .expect("write native recipient provider override");
    config.agent_roles.insert(
        "native-recipient".to_string(),
        AgentRoleConfig {
            description: Some("Native recipient".to_string()),
            config_file: Some(role_path),
            nickname_candidates: None,
        },
    );
    turn.provider = create_model_provider(config.model_provider.clone(), turn.auth_manager.clone());
    set_turn_config(&mut turn, config);

    let manager = thread_manager();
    let root = manager
        .start_thread(StartThreadOptions::new((*turn.config).clone()))
        .await
        .expect("start parent");
    session.services.agent_control = manager.agent_control();
    session.thread_id = root.thread_id;
    let before = manager.list_thread_ids().await;

    let error = SpawnAgentHandlerV2::default()
        .handle(invocation(
            Arc::new(session),
            Arc::new(turn),
            "spawn_agent",
            function_payload(json!({
                "message": "legacy native task",
                "task_name": "a".repeat(4096),
                "agent_type": "native-recipient",
                "fork_turns": "none",
            })),
        ))
        .await
        .err()
        .expect("oversized compatibility completion envelope should fail");

    assert_eq!(
        error.to_string(),
        "plaintext collaboration message exceeds the 4096-byte rendered limit"
    );
    assert_eq!(manager.list_thread_ids().await, before);
}

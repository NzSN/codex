use super::plaintext_collaboration::decoded_request_body;
use super::plaintext_collaboration::request_contains;
use super::plaintext_collaboration::request_is_child;
use anyhow::Result;
use codex_features::Feature;
use codex_protocol::ThreadId;
use codex_protocol::protocol::MultiAgentTaskPayload;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call_with_namespace;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_response_once_match;
use core_test_support::responses::mount_sse_once_match;
use core_test_support::responses::sse;
use core_test_support::responses::sse_response;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::time::Duration;
use tokio::time::Instant;
use tokio::time::sleep;

const NAMESPACE: &str = "plaintext_team";
const SPAWN_PROMPT: &str = "spawn the scheduling worker";
const SPAWN_TASK: &str = "perform the initial scheduling task";
const SPAWN_CALL_ID: &str = "spawn-scheduling-worker";
const QUEUE_PROMPT: &str = "queue context without waking the worker";
const QUEUED_MESSAGE: &str = "remember this queued context";
const QUEUE_CALL_ID: &str = "queue-scheduling-worker";
const WAIT_CALL_ID: &str = "wait-for-scheduling-worker";
const FOLLOWUP_PROMPT: &str = "wake the scheduling worker";
const FOLLOWUP_TASK: &str = "continue with the follow-up task";
const FOLLOWUP_CALL_ID: &str = "followup-scheduling-worker";
const CHILD_RESULT: &str = "initial child complete";

#[derive(Clone, Copy, Debug)]
enum CapturedRequestOrigin {
    Root,
    Child,
}

fn is_child_request(request: &wiremock::Request, root_thread_id: ThreadId) -> bool {
    decoded_request_body(request)
        .and_then(|body| {
            body["client_metadata"]["x-codex-parent-thread-id"]
                .as_str()
                .map(str::to_string)
        })
        .is_some_and(|parent| parent == root_thread_id.to_string())
}

fn collaboration_user_texts(request: &ResponsesRequest) -> Vec<String> {
    request
        .message_input_texts("user")
        .into_iter()
        .filter(|text| text.starts_with("Message Type: "))
        .collect()
}

async fn wait_for_matching_request(
    response: &core_test_support::responses::ResponseMock,
    expected: &str,
    origin: CapturedRequestOrigin,
) -> Result<ResponsesRequest> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(request) = response.requests().into_iter().find(|request| {
            let has_parent =
                request.body_json()["client_metadata"]["x-codex-parent-thread-id"].is_string();
            let origin_matches = match origin {
                CapturedRequestOrigin::Root => !has_parent,
                CapturedRequestOrigin::Child => has_parent,
            };
            origin_matches && request.body_contains_text(expected)
        }) {
            return Ok(request);
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for request containing {expected:?}");
        }
        sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_only_stays_idle_followup_wakes_and_completion_reaches_deepseek_parent() -> Result<()>
{
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut spawn_event = ev_function_call_with_namespace(
        SPAWN_CALL_ID,
        NAMESPACE,
        "spawn_agent",
        &json!({
            "message": SPAWN_TASK,
            "task_name": "worker",
            "fork_turns": "none",
        })
        .to_string(),
    );
    spawn_event["item"]["encrypted_function_args"] = json!([]);
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_contains(request, SPAWN_PROMPT),
        sse(vec![
            ev_response_created("resp-parent-spawn"),
            spawn_event,
            ev_completed("resp-parent-spawn"),
        ]),
    )
    .await;
    let initial_child_response = mount_response_once_match(
        &server,
        |request: &wiremock::Request| {
            request_is_child(request) && request_contains(request, SPAWN_TASK)
        },
        sse_response(sse(vec![
            ev_response_created("resp-child-initial"),
            ev_assistant_message("msg-child-initial", CHILD_RESULT),
            ev_completed("resp-child-initial"),
        ]))
        .set_delay(Duration::from_secs(1)),
    )
    .await;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_contains(request, SPAWN_CALL_ID),
        sse(vec![
            ev_response_created("resp-parent-spawn-finished"),
            ev_completed("resp-parent-spawn-finished"),
        ]),
    )
    .await;

    let mut queue_event = ev_function_call_with_namespace(
        QUEUE_CALL_ID,
        NAMESPACE,
        "send_message",
        &json!({"target": "worker", "message": QUEUED_MESSAGE}).to_string(),
    );
    queue_event["item"]["encrypted_function_args"] = json!([]);
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| {
            request_contains(request, QUEUE_PROMPT)
                && !request_contains(request, "Message Type: FINAL_ANSWER")
        },
        sse(vec![
            ev_response_created("resp-parent-wait"),
            ev_function_call_with_namespace(WAIT_CALL_ID, NAMESPACE, "wait_agent", "{}"),
            ev_completed("resp-parent-wait"),
        ]),
    )
    .await;
    let queue_parent_response = mount_sse_once_match(
        &server,
        |request: &wiremock::Request| {
            request_contains(request, QUEUE_PROMPT)
                && request_contains(request, "Message Type: FINAL_ANSWER")
        },
        sse(vec![
            ev_response_created("resp-parent-queue"),
            queue_event,
            ev_completed("resp-parent-queue"),
        ]),
    )
    .await;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_contains(request, QUEUE_CALL_ID),
        sse(vec![
            ev_response_created("resp-parent-queue-finished"),
            ev_completed("resp-parent-queue-finished"),
        ]),
    )
    .await;

    let mut followup_event = ev_function_call_with_namespace(
        FOLLOWUP_CALL_ID,
        NAMESPACE,
        "followup_task",
        &json!({"target": "worker", "message": FOLLOWUP_TASK}).to_string(),
    );
    followup_event["item"]["encrypted_function_args"] = json!([]);
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_contains(request, FOLLOWUP_PROMPT),
        sse(vec![
            ev_response_created("resp-parent-followup"),
            followup_event,
            ev_completed("resp-parent-followup"),
        ]),
    )
    .await;
    let followup_child_response = mount_sse_once_match(
        &server,
        |request: &wiremock::Request| {
            request_is_child(request) && request_contains(request, FOLLOWUP_TASK)
        },
        sse(vec![
            ev_response_created("resp-child-followup"),
            ev_completed("resp-child-followup"),
        ]),
    )
    .await;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| request_contains(request, FOLLOWUP_CALL_ID),
        sse(vec![
            ev_response_created("resp-parent-followup-finished"),
            ev_completed("resp-parent-followup-finished"),
        ]),
    )
    .await;

    let mut builder = test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::Collab)
            .expect("enable collaboration");
        config
            .features
            .enable(Feature::MultiAgentV2)
            .expect("enable MultiAgentV2");
        config.multi_agent_v2.task_payload = MultiAgentTaskPayload::Plaintext;
        config.multi_agent_v2.tool_namespace = Some(NAMESPACE.to_string());
        config.model_provider.name = "DeepSeek".to_string();
        config.model_provider_id = "deepseek".to_string();
        config
            .model_providers
            .insert("deepseek".to_string(), config.model_provider.clone());
    });
    let test = builder.build_with_auto_env(&server).await?;
    let root_thread_id = test.session_configured.thread_id;
    test.submit_turn(SPAWN_PROMPT).await?;

    wait_for_matching_request(
        &initial_child_response,
        SPAWN_TASK,
        CapturedRequestOrigin::Child,
    )
    .await?;
    let child_requests_before_queue = server
        .received_requests()
        .await
        .expect("captured requests")
        .into_iter()
        .filter(|request| is_child_request(request, root_thread_id))
        .count();

    let completion_envelope = format!(
        "Message Type: FINAL_ANSWER\nTask name: /root\nSender: /root/worker\nPayload:\n{CHILD_RESULT}"
    );
    test.submit_turn(QUEUE_PROMPT).await?;
    sleep(Duration::from_millis(100)).await;
    let child_requests_after_queue = server
        .received_requests()
        .await
        .expect("captured requests")
        .into_iter()
        .filter(|request| is_child_request(request, root_thread_id))
        .count();
    assert_eq!(child_requests_after_queue, child_requests_before_queue);

    let queue_request = wait_for_matching_request(
        &queue_parent_response,
        &completion_envelope,
        CapturedRequestOrigin::Root,
    )
    .await?;
    assert_eq!(
        collaboration_user_texts(&queue_request),
        vec![completion_envelope],
    );

    test.submit_turn(FOLLOWUP_PROMPT).await?;
    let followup_request = wait_for_matching_request(
        &followup_child_response,
        FOLLOWUP_TASK,
        CapturedRequestOrigin::Child,
    )
    .await?;
    let queued_envelope = format!(
        "Message Type: MESSAGE\nTask name: /root/worker\nSender: /root\nPayload:\n{QUEUED_MESSAGE}"
    );
    let followup_envelope = format!(
        "Message Type: NEW_TASK\nTask name: /root/worker\nSender: /root\nPayload:\n{FOLLOWUP_TASK}"
    );
    let initial_envelope = format!(
        "Message Type: NEW_TASK\nTask name: /root/worker\nSender: /root\nPayload:\n{SPAWN_TASK}"
    );
    let all_delivered = collaboration_user_texts(&followup_request);
    assert!(all_delivered.iter().all(|text| {
        text == &initial_envelope || text == &queued_envelope || text == &followup_envelope
    }));
    assert!(
        all_delivered
            .iter()
            .filter(|text| *text == &initial_envelope)
            .count()
            <= 1
    );
    let delivered: Vec<String> = all_delivered
        .into_iter()
        .filter(|text| text == &queued_envelope || text == &followup_envelope)
        .collect();
    assert_eq!(delivered, vec![queued_envelope, followup_envelope]);

    Ok(())
}

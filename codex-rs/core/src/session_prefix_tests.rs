use codex_protocol::AgentPath;
use codex_protocol::protocol::AgentStatus;
use codex_protocol::protocol::MultiAgentTaskPayload;
use codex_utils_output_truncation::approx_token_count;

use super::COMPLETION_MESSAGE_MAX_TOKENS;
use super::ERROR_NEXT_ACTION;
use super::format_inter_agent_completion_message_for_delivery;
use codex_model_provider_info::AgentMessageRepresentation;

#[test]
fn error_completion_message_stays_below_manual_review_threshold() {
    let message = format_inter_agent_completion_message_for_delivery(
        AgentPath::root(),
        AgentPath::try_from("/root/worker").expect("valid agent path"),
        &AgentStatus::Errored("stream disconnected ".repeat(1_000)),
        MultiAgentTaskPayload::Encrypted,
        AgentMessageRepresentation::Native,
    )
    .expect("error status should produce a completion message");

    assert!(approx_token_count(&message) < COMPLETION_MESSAGE_MAX_TOKENS);
    assert!(message.contains(ERROR_NEXT_ACTION));
}

#[test]
fn plaintext_completion_stays_within_exact_utf8_byte_limit() {
    let task_name = AgentPath::root();
    let sender = AgentPath::try_from("/root/worker").expect("valid agent path");
    let message = format_inter_agent_completion_message_for_delivery(
        task_name,
        sender,
        &AgentStatus::Completed(Some("🙂".repeat(2_000))),
        MultiAgentTaskPayload::Plaintext,
        AgentMessageRepresentation::Native,
    )
    .expect("completed status should produce a completion message");

    assert!(
        message.len()
            <= crate::inter_agent_request_projection::MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES
    );
    assert!(message.contains("Task name: /root"));
    assert!(message.contains("Sender: /root/worker"));
    assert!(message.contains("truncated"));
}

#[test]
fn plaintext_completion_rejects_an_envelope_that_cannot_fit() {
    let sender = AgentPath::try_from(format!(
        "/root/{}",
        "a".repeat(
            crate::inter_agent_request_projection::MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES
        )
    ))
    .expect("syntactically valid long agent path");

    let message = format_inter_agent_completion_message_for_delivery(
        AgentPath::root(),
        sender,
        &AgentStatus::Completed(Some("done".to_string())),
        MultiAgentTaskPayload::Plaintext,
        AgentMessageRepresentation::Native,
    );

    assert_eq!(message, None);
}

#[test]
fn compatibility_completion_is_bounded_in_encrypted_mode() {
    let message = format_inter_agent_completion_message_for_delivery(
        AgentPath::root(),
        AgentPath::try_from("/root/worker").expect("valid agent path"),
        &AgentStatus::Completed(Some("result ".repeat(2_000))),
        MultiAgentTaskPayload::Encrypted,
        AgentMessageRepresentation::UserMessage,
    )
    .expect("compatibility completion should render");

    assert!(
        message.len()
            <= crate::inter_agent_request_projection::MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES
    );
    assert!(message.contains("truncated"));
}

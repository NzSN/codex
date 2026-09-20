use super::*;
use crate::inter_agent_request_projection::MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES;
use pretty_assertions::assert_eq;

fn child_path() -> AgentPath {
    AgentPath::try_from("/root/child").expect("valid child path")
}

#[test]
fn encrypted_mode_native_delivery_preserves_legacy_unbounded_plaintext() {
    let payload = "x".repeat(MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES + 1);

    let communication = AgentMessage::Plaintext(payload)
        .into_communication(
            AgentPath::root(),
            child_path(),
            MessageDeliveryMode::TriggerTurn,
            MultiAgentTaskPayload::Encrypted,
            AgentMessageRepresentation::Native,
        )
        .expect("legacy native plaintext should remain accepted");

    assert!(communication.content.len() > MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES);
    assert_eq!(communication.encrypted_content, None);
}

#[test]
fn compatibility_delivery_rejects_unbounded_plaintext_in_encrypted_mode() {
    let error = AgentMessage::Plaintext("x".repeat(MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES + 1))
        .into_communication(
            AgentPath::root(),
            child_path(),
            MessageDeliveryMode::TriggerTurn,
            MultiAgentTaskPayload::Encrypted,
            AgentMessageRepresentation::UserMessage,
        )
        .expect_err("compatibility plaintext must remain bounded");

    assert_eq!(
        error,
        "plaintext collaboration message exceeds the 4096-byte rendered limit"
    );
}

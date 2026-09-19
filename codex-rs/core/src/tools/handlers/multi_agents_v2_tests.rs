use super::agent_message_from_tool;
use crate::agent::control::AgentMessage;
use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolCallSource;
use base64::Engine as _;
use codex_features::MultiAgentV2MessageDelivery;
use pretty_assertions::assert_eq;

#[test]
fn plaintext_delivery_accepts_readable_task_and_rejects_fernet_ciphertext() {
    let message = agent_message_from_tool(
        "compute 2 + 2".to_string(),
        &ToolCallSource::DirectPlaintextMessage,
        MultiAgentV2MessageDelivery::PlaintextCompatible,
    );
    assert!(matches!(message, Ok(AgentMessage::Plaintext(text)) if text == "compute 2 + 2"));

    let mut encoded = vec![0; 57];
    encoded[0] = 0x80;
    let ciphertext = base64::engine::general_purpose::URL_SAFE.encode(encoded);
    let result = agent_message_from_tool(
        ciphertext,
        &ToolCallSource::DirectPlaintextMessage,
        MultiAgentV2MessageDelivery::PlaintextCompatible,
    );
    let Err(FunctionCallError::RespondToModel(error)) = result else {
        panic!("ciphertext should be rejected in plaintext mode");
    };
    assert_eq!(
        error,
        "Plaintext collaboration received an encrypted task body"
    );
}

use super::*;
use codex_protocol::models::AgentMessageInputContent;
use codex_protocol::models::ContentItem;
use pretty_assertions::assert_eq;

fn agent_message(content: Vec<AgentMessageInputContent>) -> ResponseItem {
    ResponseItem::AgentMessage {
        id: Some(ResponseItemId::with_suffix("amsg", "one")),
        author: "/root".to_string(),
        recipient: "/root/child".to_string(),
        content,
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn compatibility_projection_preserves_order_and_maps_id_stably() {
    let text = "Message Type: MESSAGE\nTask name: /root/child\nSender: /root\nPayload:\nhello";
    let ordinary = ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: "ordinary".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    };
    let mut first = vec![
        ordinary.clone(),
        agent_message(vec![AgentMessageInputContent::InputText {
            text: text.to_string(),
        }]),
    ];
    let mut second = first.clone();

    project_inter_agent_communications(
        &mut first,
        AgentMessageRepresentation::UserMessage,
        MultiAgentTaskPayload::Plaintext,
    )
    .expect("project plaintext communication");
    project_inter_agent_communications(
        &mut second,
        AgentMessageRepresentation::UserMessage,
        MultiAgentTaskPayload::Plaintext,
    )
    .expect("project plaintext communication again");

    assert_eq!(first, second);
    assert_eq!(first[0], ordinary);
    let ResponseItem::Message {
        id, role, content, ..
    } = &first[1]
    else {
        panic!("expected projected user message");
    };
    assert_eq!(
        id,
        &Some(compatible_message_id(&ResponseItemId::with_suffix(
            "amsg", "one"
        )))
    );
    assert_eq!(role, "user");
    assert_eq!(
        content,
        &vec![ContentItem::InputText {
            text: text.to_string(),
        }]
    );
}

#[test]
fn encrypted_communication_is_rejected_for_compatibility_provider() {
    let mut input = vec![agent_message(vec![
        AgentMessageInputContent::InputText {
            text: "attribution".to_string(),
        },
        AgentMessageInputContent::EncryptedContent {
            encrypted_content: "opaque".to_string(),
        },
    ])];

    let error = project_inter_agent_communications(
        &mut input,
        AgentMessageRepresentation::UserMessage,
        MultiAgentTaskPayload::Encrypted,
    )
    .expect_err("encrypted content must not be relabeled");

    assert_eq!(
        error.to_string(),
        "model provider cannot consume encrypted inter-agent communication"
    );
}

#[test]
fn compatibility_projection_preserves_multipart_text_boundaries() {
    let mut input = vec![agent_message(vec![
        AgentMessageInputContent::InputText {
            text: "attribution".to_string(),
        },
        AgentMessageInputContent::InputText {
            text: "payload".to_string(),
        },
    ])];

    project_inter_agent_communications(
        &mut input,
        AgentMessageRepresentation::UserMessage,
        MultiAgentTaskPayload::Encrypted,
    )
    .expect("project multipart plaintext communication");

    let ResponseItem::Message { content, .. } = &input[0] else {
        panic!("expected projected user message");
    };
    assert_eq!(
        content,
        &vec![ContentItem::InputText {
            text: "attribution\npayload".to_string(),
        }]
    );
}

#[test]
fn plaintext_mode_rejects_oversized_native_history() {
    let mut input = vec![agent_message(vec![AgentMessageInputContent::InputText {
        text: "x".repeat(MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES + 1),
    }])];

    let error = project_inter_agent_communications(
        &mut input,
        AgentMessageRepresentation::Native,
        MultiAgentTaskPayload::Plaintext,
    )
    .expect_err("oversized plaintext history must fail before request");

    assert_eq!(
        error.to_string(),
        "plaintext collaboration message exceeds the 4096-byte rendered limit"
    );
}

#[test]
fn compatibility_projection_is_bounded_in_encrypted_mode() {
    let mut input = vec![agent_message(vec![AgentMessageInputContent::InputText {
        text: "x".repeat(MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES + 1),
    }])];

    let error = project_inter_agent_communications(
        &mut input,
        AgentMessageRepresentation::UserMessage,
        MultiAgentTaskPayload::Encrypted,
    )
    .expect_err("compatibility wrapper must always be bounded");

    assert_eq!(
        error.to_string(),
        "plaintext collaboration message exceeds the 4096-byte rendered limit"
    );
}

#[test]
fn encrypted_mode_preserves_native_agent_messages() {
    let original = agent_message(vec![AgentMessageInputContent::EncryptedContent {
        encrypted_content: "opaque".to_string(),
    }]);
    let mut input = vec![original.clone()];

    project_inter_agent_communications(
        &mut input,
        AgentMessageRepresentation::Native,
        MultiAgentTaskPayload::Encrypted,
    )
    .expect("native provider accepts legacy encrypted communication");

    assert_eq!(input, vec![original]);
}

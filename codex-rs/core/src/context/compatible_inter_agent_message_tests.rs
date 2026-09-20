use super::*;
use codex_protocol::models::ContentItem;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;

#[test]
fn renders_attributed_content_as_a_classified_user_message() {
    let content = "Message Type: MESSAGE\nTask name: /root/child\nSender: /root\nPayload:\nhello";

    assert_eq!(
        ContextualUserFragment::into(CompatibleInterAgentMessage::new(content)),
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: content.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: Some(
                InternalChatMessageMetadataPassthrough {
                    content_item_kinds: Some(vec![ContentItemKind(
                        "multi_agent.compatible_inter_agent_message".to_string(),
                    )]),
                    ..Default::default()
                },
            ),
        },
    );
}

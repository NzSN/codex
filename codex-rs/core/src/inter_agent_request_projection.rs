//! Projects canonical inter-agent history into the receiving provider's request format.

use crate::context::CompatibleInterAgentMessage;
use crate::context::ContextualUserFragment;
use codex_model_provider_info::AgentMessageRepresentation;
use codex_protocol::ResponseItemId;
use codex_protocol::error::CodexErr;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::plaintext_agent_message_content;
use codex_protocol::protocol::MultiAgentTaskPayload;
use uuid::Uuid;

pub(crate) const MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES: usize = 4_096;

pub(crate) fn project_inter_agent_communications(
    input: &mut [ResponseItem],
    representation: AgentMessageRepresentation,
    task_payload: MultiAgentTaskPayload,
) -> Result<(), CodexErr> {
    for item in input {
        let ResponseItem::AgentMessage { id, content, .. } = item else {
            continue;
        };
        if representation == AgentMessageRepresentation::Native
            && task_payload == MultiAgentTaskPayload::Encrypted
        {
            continue;
        }
        let text = plaintext_agent_message_content(content).ok_or_else(|| {
            CodexErr::InvalidRequest(
                "model provider cannot consume encrypted inter-agent communication".to_string(),
            )
        })?;
        if task_payload == MultiAgentTaskPayload::Plaintext
            || representation == AgentMessageRepresentation::UserMessage
        {
            validate_plaintext_fragment_size(&text)?;
        }
        if representation == AgentMessageRepresentation::UserMessage {
            let projected_id = id.as_ref().map(compatible_message_id);
            let mut projected =
                ContextualUserFragment::into(CompatibleInterAgentMessage::new(text));
            projected.set_id(projected_id);
            *item = projected;
        }
    }
    Ok(())
}

pub(crate) fn validate_plaintext_fragment_size(content: &str) -> Result<(), CodexErr> {
    if content.len() > MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES {
        return Err(CodexErr::InvalidRequest(
            "plaintext collaboration message exceeds the 4096-byte rendered limit".to_string(),
        ));
    }
    Ok(())
}

fn compatible_message_id(id: &ResponseItemId) -> ResponseItemId {
    ResponseItemId::with_suffix(
        "msg",
        Uuid::new_v5(&Uuid::NAMESPACE_OID, id.as_str().as_bytes()),
    )
}

#[cfg(test)]
#[path = "inter_agent_request_projection_tests.rs"]
mod tests;

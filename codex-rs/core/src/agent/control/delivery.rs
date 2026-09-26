//! Converts agent messages into attributed input while preserving their wake mode.

use super::LocalAgentControl;
use crate::agent::types::AgentMessage;
use crate::agent::types::MessageDeliveryMode;
use crate::context::ContextualUserFragment;
use crate::context::InterAgentCompletionMessage;
use crate::context::InterAgentMessage;
use crate::context::InterAgentMessageType;
use crate::inter_agent_request_projection::validate_plaintext_fragment_size;
use codex_model_provider_info::AgentMessageRepresentation;
use codex_protocol::AgentPath;
use codex_protocol::ThreadId;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::MultiAgentTaskPayload;

impl AgentMessage {
    pub(crate) fn validate_source_payload(
        &self,
        task_payload: MultiAgentTaskPayload,
    ) -> Result<(), String> {
        if task_payload == MultiAgentTaskPayload::Plaintext && matches!(self, Self::Encrypted(_)) {
            return Err("collaboration message was explicitly encrypted, but plaintext task payload mode requires readable message arguments".to_string());
        }
        Ok(())
    }

    pub(crate) fn validate_plaintext_size(
        &self,
        author: &AgentPath,
        recipient: &AgentPath,
        mode: MessageDeliveryMode,
    ) -> Result<(), String> {
        let Self::Plaintext(message) = self else {
            return Ok(());
        };
        validate_plaintext_fragment_size(&render_plaintext_message(
            message, author, recipient, mode,
        ))
        .map_err(|err| err.to_string())
    }

    pub(crate) fn into_communication(
        self,
        author: AgentPath,
        recipient: AgentPath,
        mode: MessageDeliveryMode,
        task_payload: MultiAgentTaskPayload,
        representation: AgentMessageRepresentation,
    ) -> Result<InterAgentCommunication, String> {
        self.validate_source_payload(task_payload)?;
        let trigger_turn = mode == MessageDeliveryMode::TriggerTurn;
        let communication = match self {
            Self::Encrypted(_) if representation == AgentMessageRepresentation::UserMessage => {
                return Err(
                    "model provider cannot consume encrypted inter-agent communication".to_string(),
                );
            }
            Self::Encrypted(message) => InterAgentCommunication::new_encrypted(
                author,
                recipient,
                Vec::new(),
                message,
                trigger_turn,
            ),
            Self::Plaintext(message) => {
                let content = render_plaintext_message(&message, &author, &recipient, mode);
                InterAgentCommunication::new(author, recipient, Vec::new(), content, trigger_turn)
            }
        };
        if task_payload == MultiAgentTaskPayload::Plaintext
            || representation == AgentMessageRepresentation::UserMessage
        {
            validate_plaintext_fragment_size(&communication.content)
                .map_err(|err| err.to_string())?;
        }
        Ok(communication)
    }
}

fn render_plaintext_message(
    message: &str,
    author: &AgentPath,
    recipient: &AgentPath,
    mode: MessageDeliveryMode,
) -> String {
    let message_type = match mode {
        MessageDeliveryMode::QueueOnly => InterAgentMessageType::Message,
        MessageDeliveryMode::TriggerTurn => InterAgentMessageType::NewTask,
    };
    InterAgentMessage::new(message_type, recipient.clone(), author.clone(), message).render()
}

pub(crate) fn validate_completion_envelope_size(
    parent: &AgentPath,
    child: &AgentPath,
) -> Result<(), String> {
    let content = InterAgentCompletionMessage::new(parent.clone(), child.clone(), "").render();
    validate_plaintext_fragment_size(&content).map_err(|err| err.to_string())
}

impl LocalAgentControl {
    pub(crate) async fn agent_delivery_policy(
        &self,
        target: ThreadId,
    ) -> CodexResult<(MultiAgentTaskPayload, AgentMessageRepresentation)> {
        let state = self.upgrade()?;
        let receiver_thread = state.get_thread(target).await?;
        let receiver_config = receiver_thread.session.get_config().await;
        Ok((
            receiver_config.multi_agent_v2.task_payload,
            receiver_config
                .model_provider
                .agent_message_representation(),
        ))
    }
}

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod tests;

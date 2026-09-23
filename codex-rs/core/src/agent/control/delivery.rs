//! Delivers captured agent input without exposing local loading and eviction to callers.
//!
//! Target checks precede reload, and queue-only messages retain their non-waking semantics.

use super::LocalAgentControl;
use crate::agent::api::AgentInput;
use crate::agent::api::DeliveryReceipt;
use crate::agent::api::SendRequest;
use crate::agent::types::AgentMessage;
use crate::agent::types::MessageDeliveryMode;
use crate::agent_communication::AgentCommunicationContext;
use crate::agent_communication::AgentCommunicationKind;
use crate::context::ContextualUserFragment;
use crate::context::InterAgentCompletionMessage;
use crate::context::InterAgentMessage;
use crate::context::InterAgentMessageType;
use crate::inter_agent_request_projection::validate_plaintext_fragment_size;
use codex_model_provider_info::AgentMessageRepresentation;
use codex_protocol::AgentPath;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
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

    /// Resolves and delivers captured input, restoring an evicted runtime when necessary.
    pub(crate) async fn send(&self, request: SendRequest) -> CodexResult<DeliveryReceipt> {
        let SendRequest {
            caller,
            target,
            resume_config,
            input,
            mut start_options,
        } = request;
        let target = self.resolve_target(caller, &target)?;
        let (metadata, submission_id) = match input {
            AgentInput::UserInput(input) => {
                let receiver = self.get_agent_metadata(target);
                if receiver.is_some() {
                    self.ensure_v2_agent_loaded(resume_config, target, /*parent*/ None)
                        .await?;
                }
                let submission_id = self.send_input(target, input, start_options).await?;
                (receiver.unwrap_or_default(), submission_id)
            }
            AgentInput::Message { message, mode } => {
                let receiver = self.ensure_agent_known(target)?;
                let author = self
                    .ensure_agent_known(caller)?
                    .agent_path
                    .unwrap_or_else(AgentPath::root);
                if mode == MessageDeliveryMode::TriggerTurn
                    && receiver.agent_path.as_ref().is_some_and(AgentPath::is_root)
                {
                    return Err(CodexErr::UnsupportedOperation(
                        "Follow-up tasks can't target the root agent".to_string(),
                    ));
                }
                let receiver_path = receiver.agent_path.clone().ok_or_else(|| {
                    CodexErr::UnsupportedOperation(
                        "target agent is missing an agent_path".to_string(),
                    )
                })?;
                message
                    .validate_source_payload(resume_config.multi_agent_v2.task_payload)
                    .map_err(CodexErr::UnsupportedOperation)?;
                if resume_config.multi_agent_v2.task_payload == MultiAgentTaskPayload::Plaintext {
                    message
                        .validate_plaintext_size(&author, &receiver_path, mode)
                        .map_err(CodexErr::UnsupportedOperation)?;
                }
                let sender_task_payload = resume_config.multi_agent_v2.task_payload;
                self.ensure_v2_agent_loaded(resume_config, target, /*parent*/ None)
                    .await?;
                let (receiver_task_payload, receiver_representation) =
                    self.agent_delivery_policy(target).await?;
                if receiver_task_payload != sender_task_payload {
                    return Err(CodexErr::UnsupportedOperation("target agent task payload mode does not match the sender's multi-agent tree".to_string()));
                }
                let communication = message
                    .into_communication(
                        author,
                        receiver_path,
                        mode,
                        receiver_task_payload,
                        receiver_representation,
                    )
                    .map_err(CodexErr::UnsupportedOperation)?;
                let kind = match mode {
                    MessageDeliveryMode::QueueOnly => {
                        start_options.parent_turn_id = None;
                        AgentCommunicationKind::Message
                    }
                    MessageDeliveryMode::TriggerTurn => AgentCommunicationKind::Followup,
                };
                let submission_id = self
                    .send_inter_agent_communication(
                        target,
                        communication,
                        AgentCommunicationContext::new(kind, caller),
                        start_options,
                    )
                    .await?;
                (receiver, submission_id)
            }
        };
        Ok(DeliveryReceipt {
            thread_id: target,
            metadata,
            submission_id,
        })
    }
}

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod tests;

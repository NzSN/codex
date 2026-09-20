//! Delivers V2 messages without exposing local loading and eviction to callers.
//!
//! Target checks precede reload, and queue-only messages retain their non-waking semantics.

use super::AgentControl;
use crate::TurnStartOptions;
use crate::agent::child_config::build_agent_resume_config;
use crate::agent_communication::AgentCommunicationContext;
use crate::agent_communication::AgentCommunicationKind;
use crate::context::ContextualUserFragment;
use crate::context::InterAgentCompletionMessage;
use crate::context::InterAgentMessage;
use crate::context::InterAgentMessageType;
use crate::inter_agent_request_projection::validate_plaintext_fragment_size;
use crate::session::turn_context::TurnContext;
use codex_model_provider_info::AgentMessageRepresentation;
use codex_protocol::AgentPath;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::MultiAgentTaskPayload;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageDeliveryMode {
    QueueOnly,
    TriggerTurn,
}

/// Keeps model-provided encrypted content distinct from text that needs a context wrapper.
pub(crate) enum AgentMessage {
    Plaintext(String),
    Encrypted(String),
}

impl AgentMessage {
    pub(crate) fn validate_source_payload(
        &self,
        task_payload: MultiAgentTaskPayload,
    ) -> Result<(), String> {
        if task_payload == MultiAgentTaskPayload::Plaintext && matches!(self, Self::Encrypted(_)) {
            return Err(
                "collaboration message was explicitly encrypted, but plaintext task payload mode requires readable message arguments"
                    .to_string(),
            );
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
        let content = render_plaintext_message(message, author, recipient, mode);
        validate_plaintext_fragment_size(&content).map_err(|err| err.to_string())
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

/// Separates request validation from agent runtime failures so adapters retain their error text.
#[derive(Debug)]
pub(crate) enum MessageDeliveryError {
    InvalidRequest(String),
    Agent(CodexErr),
}

impl AgentControl {
    pub(crate) async fn agent_delivery_policy(
        &self,
        target: ThreadId,
    ) -> Result<(MultiAgentTaskPayload, AgentMessageRepresentation), CodexErr> {
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

    /// Checks and delivers to a resolved target, restoring an evicted runtime when necessary.
    ///
    /// The caller resolves tool-facing names separately so it can attribute failures and
    /// interruptions to the target before delivery starts.
    pub(crate) async fn deliver_message(
        &self,
        caller: ThreadId,
        turn: &TurnContext,
        target: ThreadId,
        message: AgentMessage,
        mode: MessageDeliveryMode,
    ) -> Result<AgentPath, MessageDeliveryError> {
        let receiver_agent = self
            .ensure_agent_known(target)
            .map_err(MessageDeliveryError::Agent)?;
        if mode == MessageDeliveryMode::TriggerTurn
            && receiver_agent
                .agent_path
                .as_ref()
                .is_some_and(AgentPath::is_root)
        {
            return Err(MessageDeliveryError::InvalidRequest(
                "Follow-up tasks can't target the root agent".to_string(),
            ));
        }
        let receiver_agent_path = receiver_agent.agent_path.clone().ok_or_else(|| {
            MessageDeliveryError::InvalidRequest(
                "target agent is missing an agent_path".to_string(),
            )
        })?;
        let author = turn
            .session_source
            .get_agent_path()
            .unwrap_or_else(AgentPath::root);
        message
            .validate_source_payload(turn.config.multi_agent_v2.task_payload)
            .map_err(MessageDeliveryError::InvalidRequest)?;
        if turn.config.multi_agent_v2.task_payload == MultiAgentTaskPayload::Plaintext {
            message
                .validate_plaintext_size(&author, &receiver_agent_path, mode)
                .map_err(MessageDeliveryError::InvalidRequest)?;
        }
        let resume_config =
            build_agent_resume_config(turn).map_err(MessageDeliveryError::InvalidRequest)?;
        self.ensure_v2_agent_loaded(resume_config, target, /*parent*/ None)
            .await
            .map_err(MessageDeliveryError::Agent)?;
        let (receiver_task_payload, receiver_representation) = self
            .agent_delivery_policy(target)
            .await
            .map_err(MessageDeliveryError::Agent)?;
        if receiver_task_payload != turn.config.multi_agent_v2.task_payload {
            return Err(MessageDeliveryError::InvalidRequest(
                "target agent task payload mode does not match the sender's multi-agent tree"
                    .to_string(),
            ));
        }
        let communication = message
            .into_communication(
                author,
                receiver_agent_path.clone(),
                mode,
                receiver_task_payload,
                receiver_representation,
            )
            .map_err(MessageDeliveryError::InvalidRequest)?;
        let kind = match mode {
            MessageDeliveryMode::QueueOnly => AgentCommunicationKind::Message,
            MessageDeliveryMode::TriggerTurn => AgentCommunicationKind::Followup,
        };
        let context = AgentCommunicationContext::new(kind, caller);
        let parent_turn_id =
            matches!(mode, MessageDeliveryMode::TriggerTurn).then(|| turn.sub_id.clone());
        self.send_inter_agent_communication(
            target,
            communication,
            context,
            TurnStartOptions {
                parent_turn_id,
                root_turn_id: turn.turn_metadata_state.root_turn_id(),
                turn_trigger: turn.turn_metadata_state.current_turn_trigger(),
                cyber_access_program: turn.cyber_access_program,
                ..Default::default()
            },
        )
        .await
        .map_err(MessageDeliveryError::Agent)?;
        Ok(receiver_agent_path)
    }
}

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod tests;

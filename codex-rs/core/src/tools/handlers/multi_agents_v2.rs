//! Implements the MultiAgentV2 collaboration tool surface.

use crate::agent::AgentStatus;
use crate::agent::agent_resolver::resolve_agent_target;
use crate::agent::control::AgentMessage;
use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::multi_agents_common::*;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use base64::Engine as _;
use codex_protocol::items::CollabAgentTool;
use codex_protocol::items::CollabAgentToolCallItem;
use codex_protocol::items::CollabAgentToolCallStatus;
use codex_protocol::items::SubAgentActivityItem;
use codex_protocol::items::TurnItem;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::SubAgentActivityKind;
use codex_tools::ToolName;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;

pub(crate) use followup_task::Handler as FollowupTaskHandler;
pub(crate) use interrupt_agent::Handler as InterruptAgentHandler;
pub(crate) use list_agents::Handler as ListAgentsHandler;
pub(crate) use send_message::Handler as SendMessageHandler;
pub(crate) use spawn::Handler as SpawnAgentHandler;
pub(crate) use wait::Handler as WaitAgentHandler;

mod analytics;
mod followup_task;
mod interrupt_agent;
mod list_agents;
mod message_tool;
mod send_message;
mod spawn;
pub(crate) mod wait;

#[cfg(test)]
#[path = "multi_agents_v2_tests.rs"]
mod tests;

pub(crate) async fn emit_sub_agent_activity(
    session: &crate::session::session::Session,
    turn: &crate::session::turn_context::TurnContext,
    item: SubAgentActivityItem,
) {
    let item = TurnItem::SubAgentActivity(item);
    session.emit_turn_item_started(turn, &item).await;
    session.emit_turn_item_completed(turn, item).await;
}

fn agent_message_from_tool(
    message: String,
    source: &crate::tools::context::ToolCallSource,
    message_delivery: codex_features::MultiAgentV2MessageDelivery,
) -> Result<AgentMessage, FunctionCallError> {
    match (message_delivery, source) {
        (
            codex_features::MultiAgentV2MessageDelivery::PlaintextCompatible,
            crate::tools::context::ToolCallSource::Direct,
        ) => Err(FunctionCallError::RespondToModel(
            "Plaintext collaboration requires a readable message; the provider returned encrypted tool arguments"
                .to_string(),
        )),
        (
            codex_features::MultiAgentV2MessageDelivery::PlaintextCompatible,
            crate::tools::context::ToolCallSource::DirectPlaintextMessage
            | crate::tools::context::ToolCallSource::CodeMode { .. },
        ) => {
            // The unencrypted schema is the contract. Reject the known OpenAI
            // ciphertext format if a backend still returns it without metadata.
            let looks_encrypted = message.starts_with("gAAAAA")
                && base64::engine::general_purpose::URL_SAFE
                    .decode(&message)
                    .is_ok_and(|decoded| decoded.len() >= 57 && decoded[0] == 0x80);
            if looks_encrypted {
                Err(FunctionCallError::RespondToModel(
                    "Plaintext collaboration received an encrypted task body".to_string(),
                ))
            } else {
                Ok(AgentMessage::Plaintext(message))
            }
        }
        (
            codex_features::MultiAgentV2MessageDelivery::Encrypted,
            crate::tools::context::ToolCallSource::DirectPlaintextMessage,
        ) => Ok(AgentMessage::Plaintext(message)),
        (codex_features::MultiAgentV2MessageDelivery::Encrypted, _) => {
            Ok(AgentMessage::Encrypted(message))
        }
    }
}

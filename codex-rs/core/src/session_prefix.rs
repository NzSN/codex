use codex_model_provider_info::AgentMessageRepresentation;
use codex_protocol::AgentPath;
use codex_protocol::protocol::AgentStatus;
use codex_protocol::protocol::MultiAgentTaskPayload;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_output_truncation::truncate_text;

use crate::context::ContextualUserFragment;
use crate::context::InterAgentCompletionMessage;

const COMPLETION_MESSAGE_MAX_TOKENS: usize = 1_000;
const COMPLETION_MESSAGE_ENVELOPE_TOKEN_RESERVE: usize = 100;
const ERROR_MAX_TOKENS: usize =
    COMPLETION_MESSAGE_MAX_TOKENS - COMPLETION_MESSAGE_ENVELOPE_TOKEN_RESERVE;
const ERROR_NEXT_ACTION: &str = "This agent's turn failed. If you still need this agent, use the available collaboration tools to give it another task.";

// Helpers for model-visible session state markers that are stored in user-role
// messages but are not user intent.

// TODO(jif) unify with structured schema
pub(crate) fn format_inter_agent_completion_message_for_delivery(
    task_name: AgentPath,
    sender: AgentPath,
    status: &AgentStatus,
    task_payload: MultiAgentTaskPayload,
    representation: AgentMessageRepresentation,
) -> Option<String> {
    let payload = match status {
        AgentStatus::Completed(Some(message)) => message.clone(),
        AgentStatus::Completed(None) => String::new(),
        AgentStatus::Errored(error) => {
            let error = truncate_text(error, TruncationPolicy::Tokens(ERROR_MAX_TOKENS));
            format!("Agent errored: {error}\n\n{ERROR_NEXT_ACTION}")
        }
        AgentStatus::Shutdown => "Agent shut down.".to_string(),
        AgentStatus::NotFound => "Agent was not found.".to_string(),
        AgentStatus::PendingInit | AgentStatus::Running | AgentStatus::Interrupted => return None,
    };
    if task_payload == MultiAgentTaskPayload::Encrypted
        && representation == AgentMessageRepresentation::Native
    {
        return Some(InterAgentCompletionMessage::new(task_name, sender, payload).render());
    }

    let payload = truncate_text(&payload, TruncationPolicy::Tokens(ERROR_MAX_TOKENS));
    render_bounded_plaintext_completion(task_name, sender, &payload)
}

fn render_bounded_plaintext_completion(
    task_name: AgentPath,
    sender: AgentPath,
    payload: &str,
) -> Option<String> {
    let render = |payload: &str| {
        InterAgentCompletionMessage::new(task_name.clone(), sender.clone(), payload).render()
    };
    let empty = render("");
    if empty.len()
        > crate::inter_agent_request_projection::MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES
    {
        return None;
    }
    let untruncated = render(payload);
    if untruncated.len()
        <= crate::inter_agent_request_projection::MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES
    {
        return Some(untruncated);
    }

    let envelope_bytes = empty.len();
    let mut payload_budget =
        crate::inter_agent_request_projection::MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES
            .saturating_sub(envelope_bytes);
    loop {
        let truncated = truncate_text(payload, TruncationPolicy::Bytes(payload_budget));
        let rendered = render(&truncated);
        let overflow = rendered.len().saturating_sub(
            crate::inter_agent_request_projection::MAX_PLAINTEXT_COLLABORATION_FRAGMENT_BYTES,
        );
        if overflow == 0 {
            return Some(rendered);
        }
        if payload_budget == 0 {
            return Some(empty);
        }
        payload_budget = payload_budget.saturating_sub(overflow.max(1));
    }
}

#[cfg(test)]
#[path = "session_prefix_tests.rs"]
mod tests;

pub(crate) fn format_subagent_context_line(
    agent_reference: &str,
    agent_nickname: Option<&str>,
) -> String {
    match agent_nickname.filter(|nickname| !nickname.is_empty()) {
        Some(agent_nickname) => format!("- {agent_reference}: {agent_nickname}"),
        None => format!("- {agent_reference}"),
    }
}

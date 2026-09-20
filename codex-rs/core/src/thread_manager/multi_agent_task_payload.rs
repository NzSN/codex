use crate::config::Config;
use codex_history::InitialHistory;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;

pub(super) fn restore_resumed_multi_agent_task_payload(
    config: &mut Config,
    initial_history: &InitialHistory,
) -> CodexResult<()> {
    if !matches!(initial_history, InitialHistory::Resumed(_)) {
        return Ok(());
    }

    let persisted = initial_history.get_multi_agent_task_payload();
    let configured = config.multi_agent_v2.task_payload;
    if config.multi_agent_v2.task_payload_locked && configured != persisted {
        return Err(CodexErr::InvalidRequest(format!(
            "cannot resume a multi-agent tree stored with task_payload = \"{persisted}\" while features.multi_agent_v2.task_payload is configured as \"{configured}\"; start a fresh tree instead"
        )));
    }
    config.multi_agent_v2.task_payload = persisted;
    Ok(())
}

#[cfg(test)]
#[path = "multi_agent_task_payload_tests.rs"]
mod tests;

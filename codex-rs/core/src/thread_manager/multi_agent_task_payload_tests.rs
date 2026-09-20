use super::*;
use crate::config::test_config;
use codex_history::ResumedHistory;
use codex_history::RolloutItem;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::protocol::MultiAgentTaskPayload;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::SessionMetaLine;
use pretty_assertions::assert_eq;
use std::sync::Arc;

fn resumed_history_with_task_payload(task_payload: MultiAgentTaskPayload) -> InitialHistory {
    let thread_id = ThreadId::new();
    InitialHistory::Resumed(ResumedHistory {
        conversation_id: thread_id,
        history: Arc::new(vec![RolloutItem::SessionMeta(SessionMetaLine {
            meta: SessionMeta {
                id: thread_id,
                session_id: thread_id.into(),
                multi_agent_task_payload: task_payload,
                ..SessionMeta::default()
            },
            git: None,
        })]),
        rollout_path: None,
    })
}

#[tokio::test]
async fn resume_restores_persisted_multi_agent_task_payload() {
    let mut config = test_config().await;
    let history = resumed_history_with_task_payload(MultiAgentTaskPayload::Plaintext);

    restore_resumed_multi_agent_task_payload(&mut config, &history)
        .expect("restore persisted task payload");

    assert_eq!(
        config.multi_agent_v2.task_payload,
        MultiAgentTaskPayload::Plaintext
    );
}

#[tokio::test]
async fn resume_accepts_matching_configured_multi_agent_task_payload() {
    let mut config = test_config().await;
    config.multi_agent_v2.task_payload = MultiAgentTaskPayload::Plaintext;
    config.multi_agent_v2.task_payload_locked = true;
    let history = resumed_history_with_task_payload(MultiAgentTaskPayload::Plaintext);

    restore_resumed_multi_agent_task_payload(&mut config, &history)
        .expect("accept matching configured task payload");
}

#[tokio::test]
async fn resume_rejects_conflicting_multi_agent_task_payload() {
    let mut config = test_config().await;
    config.multi_agent_v2.task_payload = MultiAgentTaskPayload::Plaintext;
    config.multi_agent_v2.task_payload_locked = true;
    let history = resumed_history_with_task_payload(MultiAgentTaskPayload::Encrypted);

    let error = restore_resumed_multi_agent_task_payload(&mut config, &history)
        .expect_err("reject conflicting configured task payload");

    let CodexErrorDetails::InvalidRequest(message) = error.details() else {
        panic!("unexpected resume error: {error}");
    };
    assert_eq!(
        message,
        "cannot resume a multi-agent tree stored with task_payload = \"encrypted\" while features.multi_agent_v2.task_payload is configured as \"plaintext\"; start a fresh tree instead"
    );
}

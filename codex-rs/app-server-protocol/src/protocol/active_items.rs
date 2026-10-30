//! Live activity is independent of transcript pagination and replay-buffer retention.
use super::common::ServerNotification;
use super::v2::ItemStartedNotification;
use super::v2::ThreadItem;

#[derive(Debug, Clone, Default)]
pub struct ActiveItems {
    items: Vec<ItemStartedNotification>,
}

impl ActiveItems {
    pub fn items(&self) -> &[ItemStartedNotification] {
        &self.items
    }

    pub fn note(&mut self, notification: &ServerNotification) {
        match notification {
            ServerNotification::ItemStarted(started) if is_tool(&started.item) => {
                if let Some(existing) = self.items.iter_mut().find(|item| {
                    item.turn_id == started.turn_id && item.item.id() == started.item.id()
                }) {
                    existing.item = started.item.clone();
                } else {
                    self.items.push(started.clone());
                }
            }
            ServerNotification::ItemCompleted(completed) => self.items.retain(|item| {
                item.turn_id != completed.turn_id || item.item.id() != completed.item.id()
            }),
            ServerNotification::TurnStarted(turn) => {
                self.items.retain(|item| item.turn_id == turn.turn.id);
            }
            ServerNotification::TurnCompleted(turn) => {
                self.items.retain(|item| item.turn_id != turn.turn.id);
            }
            ServerNotification::ThreadClosed(_) => self.items.clear(),
            _ => {}
        }
    }
}

fn is_tool(item: &ThreadItem) -> bool {
    matches!(
        item,
        ThreadItem::CommandExecution { .. }
            | ThreadItem::McpToolCall { .. }
            | ThreadItem::DynamicToolCall { .. }
            | ThreadItem::FileChange { .. }
            | ThreadItem::WebSearch(_)
            | ThreadItem::ImageGeneration(_)
            | ThreadItem::ImageView { .. }
            | ThreadItem::CollabAgentToolCall { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::super::v2::ItemCompletedNotification;
    use super::*;

    fn start(id: &str) -> ItemStartedNotification {
        serde_json::from_value(serde_json::json!({
            "threadId":"thread", "turnId":"turn", "startedAtMs":123,
            "item":{"type":"dynamicToolCall", "id":id,"namespace":"tools",
                "tool":"work","arguments":{},"status":"inProgress",
                "contentItems":null,"success":null,"durationMs":null}
        }))
        .unwrap()
    }

    #[test]
    fn active_items_parallel_update_completion_and_original_start_time() {
        let mut state = ActiveItems::default();
        let first = start("a");
        state.note(&ServerNotification::ItemStarted(first.clone()));
        state.note(&ServerNotification::ItemStarted(start("b")));
        let mut update = first.clone();
        update.started_at_ms = 999;
        state.note(&ServerNotification::ItemStarted(update));
        assert_eq!(state.items().len(), 2);
        assert_eq!(state.items()[0].started_at_ms, 123);
        state.note(&ServerNotification::ItemCompleted(
            ItemCompletedNotification {
                thread_id: "thread".into(),
                turn_id: "turn".into(),
                item: first.item,
                completed_at_ms: 1000,
            },
        ));
        assert_eq!(state.items().len(), 1);
        assert_eq!(state.items()[0].item.id(), "b");
    }

    #[test]
    fn active_items_clear_on_thread_close_and_ignore_non_tools() {
        let mut state = ActiveItems::default();
        state.note(&ServerNotification::ItemStarted(start("a")));
        state.note(&ServerNotification::ThreadClosed(
            super::super::v2::ThreadClosedNotification {
                thread_id: "thread".into(),
            },
        ));
        assert!(state.items().is_empty());
    }

    #[test]
    fn active_items_builder_snapshot_is_independent_of_history_pages() {
        use codex_protocol::items::{DynamicToolCallItem, DynamicToolCallStatus, TurnItem};
        use codex_protocol::protocol::{EventMsg, ItemCompletedEvent, ItemStartedEvent};
        let mut builder = crate::ThreadHistoryBuilder::new();
        let thread_id = codex_protocol::ThreadId::new();
        builder.handle_event(&EventMsg::TurnStarted(
            codex_protocol::protocol::TurnStartedEvent {
                turn_attribution: None,
                turn_id: "turn".into(),
                root_turn_id: None,
                trace_id: None,
                started_at: Some(0),
                model_context_window: None,
                collaboration_mode_kind: Default::default(),
            },
        ));
        let mut item = TurnItem::DynamicToolCall(DynamicToolCallItem {
            id: "running".into(),
            namespace: Some("tools".into()),
            tool: "work".into(),
            arguments: serde_json::json!({}),
            status: DynamicToolCallStatus::InProgress,
            content_items: None,
            success: None,
            error: None,
            duration: None,
        });
        builder.handle_event(&EventMsg::ItemStarted(ItemStartedEvent {
            thread_id,
            turn_id: "turn".into(),
            item: item.clone(),
            started_at_ms: 123,
        }));
        let snapshot = builder.active_items(&thread_id.to_string());
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].turn_id, "turn");
        assert_eq!(snapshot[0].started_at_ms, 123);
        if let TurnItem::DynamicToolCall(call) = &mut item {
            call.status = DynamicToolCallStatus::Completed;
        }
        builder.handle_event(&EventMsg::ItemCompleted(ItemCompletedEvent {
            thread_id,
            turn_id: "turn".into(),
            item,
            started_at_ms: Some(123),
            completed_at_ms: 999,
        }));
        assert!(builder.active_items(&thread_id.to_string()).is_empty());
    }
}

/// Status-bearing tool items can also seed state from a resumed turn snapshot.
pub fn is_in_progress_tool(item: &ThreadItem) -> bool {
    use super::v2::*;
    if matches!(item, ThreadItem::WebSearch(search) if search.action.is_none())
        || matches!(item, ThreadItem::ImageGeneration(image) if image.status.is_empty() || image.status == "in_progress")
    {
        return true;
    }
    matches!(
        item,
        ThreadItem::CommandExecution {
            status: CommandExecutionStatus::InProgress,
            ..
        } | ThreadItem::McpToolCall {
            status: McpToolCallStatus::InProgress,
            ..
        } | ThreadItem::DynamicToolCall {
            status: DynamicToolCallStatus::InProgress,
            ..
        } | ThreadItem::FileChange {
            status: PatchApplyStatus::InProgress,
            ..
        } | ThreadItem::CollabAgentToolCall {
            status: CollabAgentToolCallStatus::InProgress,
            ..
        }
    )
}

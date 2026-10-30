//! Retain dynamic tool state in either renderer; the app emits only finished rows to terminal history.

use super::*;

impl ChatWidget {
    pub(super) fn on_dynamic_tool_item(&mut self, item: ThreadItem) {
        let started = matches!(
            &item,
            ThreadItem::DynamicToolCall {
                status: codex_app_server_protocol::DynamicToolCallStatus::InProgress,
                ..
            }
        );
        self.defer_or_handle(
            item,
            |interrupts, item| {
                if started {
                    interrupts.push_item_started(item);
                } else {
                    interrupts.push_item_completed(item);
                }
            },
            Self::handle_dynamic_tool_item_now,
        );
    }

    pub(crate) fn handle_dynamic_tool_item_now(&mut self, item: ThreadItem) {
        let ThreadItem::DynamicToolCall { id, .. } = &item else {
            return;
        };
        let id = id.clone();
        if let Some(cell) = self.transcript.dynamic_calls.get(&id) {
            cell.update_from_item(item);
            if !cell.is_active() {
                self.transcript.dynamic_calls.remove(&id);
            }
        } else if let Some(cell) = history_cell::DynamicToolCallCell::from_item(item) {
            self.flush_answer_stream_with_separator();
            if cell.is_active() {
                self.transcript
                    .dynamic_calls
                    .insert(cell.call_id().to_owned(), cell.clone());
            }
            self.add_to_history(cell);
        }
        self.request_redraw();
    }

    pub(super) fn finish_dynamic_activity(&mut self) {
        self.transcript.active_tools = Default::default();
        self.transcript.active_tool_rows.clear();
        for (_, cell) in self.transcript.dynamic_calls.drain() {
            cell.mark_interrupted();
        }
    }
}

impl ChatWidget {
    pub(super) fn active_tool_has_primary_row(&self, id: &str) -> bool {
        self.transcript.active_cell.as_ref().is_some_and(|cell| {
            cell.activity_ids().iter().any(|key| {
                key.split_once(':')
                    .is_some_and(|(_, item_id)| item_id == id)
            })
        })
    }

    pub(super) fn refresh_active_tool_rows(&mut self) {
        if self.external_writer_view {
            self.transcript.active_tool_rows.clear();
            return;
        }
        self.transcript.active_tool_rows = self
            .transcript
            .active_tools
            .items()
            .iter()
            .map(|started| -> Box<dyn HistoryCell> {
                let animations =
                    self.local_settings.tui.animations && self.local_settings.tui.effects.progress;
                match &started.item {
                    ThreadItem::CommandExecution {
                        id,
                        command,
                        command_actions,
                        source,
                        ..
                    } => {
                        let (command, parsed) =
                            super::exec_state::command_execution_command_and_parsed(
                                command,
                                command_actions,
                            );
                        return Box::new(crate::exec_cell::new_active_exec_command(
                            id.clone(),
                            command,
                            parsed,
                            *source,
                            None,
                            animations,
                        ));
                    }
                    ThreadItem::McpToolCall {
                        id,
                        server,
                        tool,
                        arguments,
                        ..
                    } => {
                        return Box::new(history_cell::new_active_mcp_tool_call(
                            id.clone(),
                            history_cell::McpInvocation {
                                server: server.clone(),
                                tool: tool.clone(),
                                arguments: Some(arguments.clone()),
                            },
                            animations,
                        ));
                    }
                    ThreadItem::DynamicToolCall { .. } => {
                        if let Some(cell) =
                            history_cell::DynamicToolCallCell::from_item(started.item.clone())
                        {
                            return Box::new(cell);
                        }
                    }
                    _ => {}
                }
                let label = match &started.item {
                    ThreadItem::CommandExecution { command, .. } => format!("Running {command}"),
                    ThreadItem::McpToolCall { server, tool, .. } => {
                        format!("Calling {server}/{tool}")
                    }
                    ThreadItem::DynamicToolCall {
                        namespace, tool, ..
                    } => format!(
                        "Calling {}{tool}",
                        namespace
                            .as_ref()
                            .map(|n| format!("{n}."))
                            .unwrap_or_default()
                    ),
                    ThreadItem::CollabAgentToolCall { tool, .. } => {
                        use codex_app_server_protocol::CollabAgentTool;
                        match tool {
                            CollabAgentTool::Wait => "Waiting for agents",
                            CollabAgentTool::SpawnAgent => "Starting agent",
                            CollabAgentTool::SendInput => "Sending message to agent",
                            CollabAgentTool::CloseAgent => "Closing agent",
                            CollabAgentTool::ResumeAgent => "Resuming agent",
                            CollabAgentTool::SendMessage => "Sending message to agent",
                            CollabAgentTool::FollowupTask => "Assigning follow-up task",
                            CollabAgentTool::InterruptAgent => "Interrupting agent",
                            CollabAgentTool::ListAgents => "Checking agents",
                        }
                        .to_string()
                    }
                    ThreadItem::FileChange { .. } => "Applying patch".to_string(),
                    ThreadItem::WebSearch(_) => "Searching the web".to_string(),
                    ThreadItem::ImageGeneration(_) => "Generating image".to_string(),
                    ThreadItem::ImageView { .. } => "Opening image".to_string(),
                    _ => "Running tool".to_string(),
                };
                Box::new(history_cell::PlainHistoryCell::new(vec![
                    ratatui::text::Line::from(format!("• {label}")),
                ]))
            })
            .collect();
        self.bump_active_cell_revision();
        self.request_redraw();
    }
}

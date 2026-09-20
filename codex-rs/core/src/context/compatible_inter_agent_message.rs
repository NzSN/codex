use super::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

/// An attributed agent message represented as user input for compatible providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompatibleInterAgentMessage {
    content: String,
}

impl CompatibleInterAgentMessage {
    pub(crate) fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
        }
    }
}

impl ContextualUserFragment for CompatibleInterAgentMessage {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("multi_agent.compatible_inter_agent_message".to_string())
    }

    fn role(&self) -> &'static str {
        "user"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("", "")
    }

    fn body(&self) -> String {
        self.content.clone()
    }
}

#[cfg(test)]
#[path = "compatible_inter_agent_message_tests.rs"]
mod tests;

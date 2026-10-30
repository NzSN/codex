pub use codex_api::ResponseEvent;
use codex_protocol::ResponseItemId;
use codex_protocol::error::Result;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::DEFAULT_IMAGE_DETAIL;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_tools::ToolSpec;
use futures::Stream;
use serde_json::Value;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// API request payload for a single model turn
#[derive(Debug, Clone)]
pub struct Prompt {
    /// Conversation context input items.
    pub input: Vec<ResponseItem>,

    /// Tool definitions to inject into this request, empty when supplied by history.
    pub(crate) tools: Arc<[ToolSpec]>,

    /// Whether parallel tool calls are permitted for this prompt.
    pub(crate) parallel_tool_calls: bool,

    pub base_instructions: BaseInstructions,

    /// Optional the output schema for the model's response.
    pub output_schema: Option<Value>,

    /// Whether the Responses API should strictly validate `output_schema`.
    pub output_schema_strict: bool,

    pub(crate) cyber_access_program: Option<codex_protocol::turn_input::CyberAccessProgram>,
}

impl Default for Prompt {
    fn default() -> Self {
        Self {
            input: Vec::new(),
            tools: Arc::default(),
            parallel_tool_calls: false,
            base_instructions: BaseInstructions::default(),
            output_schema: None,
            output_schema_strict: true,
            cyber_access_program: None,
        }
    }
}

impl Prompt {
    pub(crate) fn get_formatted_input_for_request(
        &self,
        model_info: &ModelInfo,
    ) -> Vec<ResponseItem> {
        let mut input = self.input.clone();
        normalize_image_details(&mut input, model_info);
        input
    }
}

/// Custom Responses providers commonly require every tool result to have a
/// matching call. Project host-delivered notifications into call/result pairs
/// only in the request copy, keeping their contents at tool-result authority.
pub(crate) fn pair_standalone_tool_outputs(input: &mut Vec<ResponseItem>) -> Result<()> {
    if !input
        .iter()
        .any(|item| matches!(item, ResponseItem::FunctionCallOutput { call_id: None, .. }))
    {
        return Ok(());
    }
    let mut call_ids: HashSet<String> = input
        .iter()
        .filter_map(|item| match item {
            ResponseItem::FunctionCall { call_id, .. }
            | ResponseItem::CustomToolCall { call_id, .. }
            | ResponseItem::CustomToolCallOutput { call_id, .. } => Some(call_id.clone()),
            ResponseItem::FunctionCallOutput { call_id, .. }
            | ResponseItem::LocalShellCall { call_id, .. }
            | ResponseItem::ToolSearchCall { call_id, .. }
            | ResponseItem::ToolSearchOutput { call_id, .. } => call_id.clone(),
            _ => None,
        })
        .collect();
    let namespace = Uuid::new_v5(&Uuid::NAMESPACE_OID, b"codex/standalone-tool-output");
    let mut projected = Vec::with_capacity(input.len());
    for mut item in std::mem::take(input) {
        if matches!(item, ResponseItem::FunctionCallOutput { call_id: None, .. }) {
            // Persisted IDs keep retries and resumed requests stable. Legacy
            // items without IDs use their payload, with collisions disambiguated below.
            let identity = match item.id() {
                Some(id) => id.as_str().as_bytes().to_vec(),
                None => serde_json::to_vec(&item)?,
            };
            let suffix = Uuid::new_v5(&namespace, &identity);
            let base_call_id = format!("call_codex_delivery_{suffix}");
            let mut call_id = base_call_id.clone();
            let mut collision = 0;
            while !call_ids.insert(call_id.clone()) {
                collision += 1;
                call_id = format!("{base_call_id}_{collision}");
            }
            if let ResponseItem::FunctionCallOutput {
                call_id: output_call_id,
                name,
                namespace,
                ..
            } = &mut item
            {
                projected.push(ResponseItem::FunctionCall {
                    encrypted_content: None,
                    status: None,
                    id: Some(ResponseItemId::with_suffix("fc", &call_id)),
                    name: name
                        .clone()
                        .unwrap_or_else(|| "codex_notification".to_string()),
                    namespace: namespace.clone(),
                    arguments: "{}".to_string(),
                    encrypted_function_args: None,
                    call_id: call_id.clone(),
                    internal_chat_message_metadata_passthrough: None,
                });
                *output_call_id = Some(call_id);
            }
        }
        projected.push(item);
    }
    *input = projected;
    Ok(())
}

fn normalize_image_details(items: &mut [ResponseItem], model_info: &ModelInfo) {
    for item in items {
        match item {
            ResponseItem::Message { content, .. } => {
                for content_item in content {
                    if let ContentItem::InputImage { detail, .. } = content_item {
                        normalize_image_detail(detail, model_info);
                    }
                }
            }
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => {
                if let Some(content) = output.content_items_mut() {
                    for content_item in content {
                        if let FunctionCallOutputContentItem::InputImage { detail, .. } =
                            content_item
                        {
                            normalize_image_detail(detail, model_info);
                        }
                    }
                }
            }
            ResponseItem::AdditionalTools { .. }
            | ResponseItem::Reasoning { .. }
            | ResponseItem::AgentMessage { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::FunctionCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
            | ResponseItem::Compaction { .. }
            | ResponseItem::ConfigurationUpdate { .. }
            | ResponseItem::CompactionTrigger { .. }
            | ResponseItem::ContextCompaction { .. }
            | ResponseItem::Other => {}
        }
    }
}

fn normalize_image_detail(detail: &mut Option<ImageDetail>, model_info: &ModelInfo) {
    if model_info.use_responses_lite {
        *detail = None;
    } else if *detail == Some(ImageDetail::Original) && !model_info.supports_image_detail_original {
        *detail = Some(DEFAULT_IMAGE_DETAIL);
    }
}

pub struct ResponseStream {
    pub(crate) routing_affinity: Option<codex_http_client::ChatGptRoutingAffinity>,
    pub(crate) rx_event: mpsc::Receiver<Result<ResponseEvent>>,
    pub(crate) interrupt: Option<oneshot::Sender<codex_api::ResponseInterrupt>>,
    /// Signals the mapper task that the consumer stopped polling before the
    /// provider stream reached its own terminal event.
    pub(crate) consumer_dropped: CancellationToken,
}

impl Stream for ResponseStream {
    type Item = Result<ResponseEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx_event.poll_recv(cx)
    }
}

impl Drop for ResponseStream {
    fn drop(&mut self) {
        self.consumer_dropped.cancel();
    }
}

#[cfg(test)]
#[path = "client_common_tests.rs"]
mod tests;

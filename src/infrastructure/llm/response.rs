use serde::{Deserialize, Serialize};
use std::fmt;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    Other(String),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}
impl Usage {
    pub fn total(&self) -> Option<u32> {
        self.prompt_tokens.checked_add(self.completion_tokens)
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub model: String,
    pub content: Option<String>,
    /// Response-only diagnostics; never part of a request message.
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: FinishReason,
    pub usage: Option<Usage>,
}
impl fmt::Debug for ToolCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolCall")
            .field("id_bytes", &self.id.len())
            .field("name_bytes", &self.name.len())
            .field("arguments_bytes", &self.arguments.len())
            .finish()
    }
}
impl fmt::Debug for FinishReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stop => f.write_str("Stop"),
            Self::ToolCalls => f.write_str("ToolCalls"),
            Self::Length => f.write_str("Length"),
            Self::ContentFilter => f.write_str("ContentFilter"),
            Self::Other(reason) => f
                .debug_tuple("Other")
                .field(&format_args!("bytes={}", reason.len()))
                .finish(),
        }
    }
}
impl fmt::Debug for CompletionResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompletionResponse")
            .field("model_bytes", &self.model.len())
            .field("content_bytes", &self.content.as_ref().map(String::len))
            .field(
                "reasoning_bytes",
                &self.reasoning_content.as_ref().map(String::len),
            )
            .field("reasoning_tokens", &self.reasoning_tokens)
            .field("tool_call_count", &self.tool_calls.len())
            .field("finish_reason", &self.finish_reason)
            .field("has_usage", &self.usage.is_some())
            .finish()
    }
}

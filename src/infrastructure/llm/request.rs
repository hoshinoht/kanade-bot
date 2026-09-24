use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: Option<String>,
        tool_calls: Vec<ToolCallRequest>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: String,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputSchema {
    pub name: String,
    pub schema: Value,
    pub strict: bool,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub output_schema: Option<OutputSchema>,
    pub max_output_tokens: u32,
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::System { content } => f
                .debug_struct("System")
                .field("content_bytes", &content.len())
                .finish(),
            Self::User { content } => f
                .debug_struct("User")
                .field("content_bytes", &content.len())
                .finish(),
            Self::Assistant {
                content,
                tool_calls,
            } => f
                .debug_struct("Assistant")
                .field("content_bytes", &content.as_ref().map(String::len))
                .field("tool_call_count", &tool_calls.len())
                .finish(),
            Self::Tool {
                tool_call_id,
                content,
            } => f
                .debug_struct("Tool")
                .field("tool_call_id_bytes", &tool_call_id.len())
                .field("content_bytes", &content.len())
                .finish(),
        }
    }
}
impl fmt::Debug for ToolCallRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolCallRequest")
            .field("id_bytes", &self.id.len())
            .field("name_bytes", &self.name.len())
            .field("arguments_bytes", &self.arguments.len())
            .finish()
    }
}
impl fmt::Debug for ToolDefinition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolDefinition")
            .field("name_bytes", &self.name.len())
            .field(
                "description_bytes",
                &self.description.as_ref().map(String::len),
            )
            .finish()
    }
}
impl fmt::Debug for OutputSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutputSchema")
            .field("name_bytes", &self.name.len())
            .field("strict", &self.strict)
            .finish()
    }
}
impl fmt::Debug for ChatRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatRequest")
            .field("model_bytes", &self.model.len())
            .field("message_count", &self.messages.len())
            .field("tool_count", &self.tools.len())
            .field("has_output_schema", &self.output_schema.is_some())
            .field("max_output_tokens", &self.max_output_tokens)
            .finish()
    }
}

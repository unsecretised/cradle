//! Cradle's provider-neutral conversation format.
//!
//! It mirrors the Anthropic Messages API, which has the richest shape. Blocks Cradle does
//! not interpret (thinking, redacted thinking, fallback markers) are kept as
//! [`Block::Opaque`] and sent back unchanged, which the API requires for thinking blocks.

use std::ops::AddAssign;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    /// A mid-conversation operator message (mode changes). Never from the model or a tool.
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Block>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Message {
            role: Role::User,
            content: vec![Block::Text { text: text.into() }],
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Message {
            role: Role::System,
            content: vec![Block::Text { text: text.into() }],
        }
    }

    /// Concatenated text blocks.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_uses(&self) -> impl Iterator<Item = ToolUse<'_>> {
        self.content.iter().filter_map(|b| match b {
            Block::ToolUse {
                id,
                name,
                input,
                invalid_json,
            } => Some(ToolUse {
                id,
                name,
                input,
                invalid_json: invalid_json.as_deref(),
            }),
            _ => None,
        })
    }
}

/// A borrowed view of a [`Block::ToolUse`].
#[derive(Debug, Clone, Copy)]
pub struct ToolUse<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub input: &'a Value,
    /// Set when the streamed input was not valid JSON. The tool must not run.
    pub invalid_json: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invalid_json: Option<String>,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
    /// A provider block passed back verbatim (e.g. `thinking` with its `signature`).
    Opaque {
        value: Value,
    },
}

/// A tool as offered to the model.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Copy)]
pub struct ModelRequest<'a> {
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
    pub max_tokens: u32,
}

/// Incremental output while a response streams.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    TextDelta(String),
    ThinkingDelta(String),
    ToolUseStarted { name: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    /// Output was cut off. Tool calls in this turn may be truncated and must not run.
    MaxTokens,
    Refusal {
        category: Option<String>,
        explanation: Option<String>,
    },
    PauseTurn,
    Other(String),
}

impl StopReason {
    pub(crate) fn from_anthropic(reason: &str, details: Option<&Value>) -> Self {
        match reason {
            "end_turn" | "stop_sequence" => StopReason::EndTurn,
            "tool_use" => StopReason::ToolUse,
            "max_tokens" | "model_context_window_exceeded" => StopReason::MaxTokens,
            "pause_turn" => StopReason::PauseTurn,
            "refusal" => StopReason::Refusal {
                category: details
                    .and_then(|d| d.get("category"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                explanation: details
                    .and_then(|d| d.get("explanation"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
            other => StopReason::Other(other.to_owned()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl AddAssign for Usage {
    fn add_assign(&mut self, other: Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
    }
}

/// One complete model response.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub message: Message,
    pub stop: StopReason,
    pub usage: Usage,
}

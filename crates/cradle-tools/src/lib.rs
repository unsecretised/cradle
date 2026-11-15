//! The agent's only way to act on the project. See `.agents/roadmap/FEATURE_TOOLS.md`.
//!
//! Every path goes through [`PathGuard`]. Policy (which mode may use which tool, and what
//! needs approval) lives in `cradle-core`. This crate only knows how to do things safely.

pub mod command;
pub mod fs;
pub mod guard;
pub mod hygiene;
pub mod specs;
pub mod vars;

pub use guard::{GuardError, PathGuard, Resolved, WriteClass};
pub use specs::{
    Access, ActivityKind, FileRange, MAX_OPTIONS, Question, QuestionKind, ToolActivity, ToolCall,
    ToolDefinition, definitions, parse_call,
};
pub use vars::VarStore;

/// A tool failure, reported back to the model as an error result.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ToolError(pub String);

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        ToolError(message.into())
    }
}

impl From<GuardError> for ToolError {
    fn from(e: GuardError) -> Self {
        ToolError(e.to_string())
    }
}

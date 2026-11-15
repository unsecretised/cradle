//! The agent loop, modes, prompts, trust, sessions and the command reviewer.
//!
//! See `.agents/architecture.md`.

pub mod agent;
pub mod modes;
pub mod prompt;
pub mod review;
pub mod session;
pub mod soul;
pub mod trust;

pub use agent::{
    AgentCommand, AgentEvent, AgentHandle, AgentSetup, Answer, Approval, ApprovalRequest, FileStat,
    QuestionRequest, ReviewPolicy, spawn,
};
pub use modes::{ActiveMode, Clash, ClashChoice, ModeSet};
pub use review::Verdict;

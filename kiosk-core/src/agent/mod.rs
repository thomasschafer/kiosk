use serde::{Deserialize, Serialize};
use std::fmt;

/// Represents the kind of AI coding agent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    Unknown,
}

/// Represents the current state of an AI coding agent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentState {
    /// Agent is actively working (spinner, processing)
    Running,
    /// Agent needs user action (permission prompt, input prompt)
    Waiting,
    /// Agent is at prompt, not doing anything
    Idle,
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentState::Running => write!(f, "Running"),
            AgentState::Waiting => write!(f, "Waiting"),
            AgentState::Idle => write!(f, "Idle"),
        }
    }
}

pub mod detect;

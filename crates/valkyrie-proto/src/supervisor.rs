//! What the supervisor did (backlog #180, protocol 43): each step it offered and
//! you answered, or a policy took without asking, and each build order answered,
//! newest first, for the supervisor page.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One thing the supervisor did, or offered and got an answer to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorEntry {
    /// Seconds since the epoch.
    pub at: u64,
    /// The repository it was in.
    pub project: PathBuf,
    /// One line: "you took: fix CI on PR #101", "policy #12: started #45".
    pub said: String,
    /// The policy that took it without asking; `None` when you answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<u32>,
    /// The pull request it was about, as a URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
}

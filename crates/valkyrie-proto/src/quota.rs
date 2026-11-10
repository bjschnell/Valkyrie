//! How much of the agent's plan is used up (protocol 28), as Claude Code tells its
//! status line: read by `valk statusline`, held by the daemon, pushed to queue
//! watchers so a home screen can say whether there's room for another session.

use serde::{Deserialize, Serialize};

/// The latest reading, from whichever Claude session last drew its status line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quota {
    /// When it was read (seconds since the epoch): readings only come while a
    /// Claude session runs, so a client shows how old it is.
    pub at: u64,
    /// The plan's windows, shortest first.
    pub limits: Vec<QuotaLimit>,
}

/// One rate-limit window, e.g. the five-hour one or the week.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaLimit {
    /// Claude Code's own name for it: `five_hour`, `seven_day`, `seven_day_opus`, …
    pub name: String,
    /// Share of the window used, 0 to 100.
    pub used: u8,
    /// When the window resets (seconds since the epoch), if said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<u64>,
}

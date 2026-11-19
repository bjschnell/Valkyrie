//! The repository wiki (backlog #210): pages agents write for agents about how
//! the code is built, kept in the context store. The TUI and the web app list
//! and read them, and start the agent that writes them (protocol 46).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One page. In a listing `body` is empty; `WikiPage` fills it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WikiPage {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    /// The paths it covers, relative to the repository root.
    #[serde(default)]
    pub scope: Vec<String>,
    /// The commit it was written at, short.
    #[serde(default)]
    pub commit: Option<String>,
    /// When it was written, Unix seconds.
    #[serde(default)]
    pub updated: u64,
    /// Why it may be out of date, if it may.
    #[serde(default)]
    pub stale: Option<String>,
    #[serde(default)]
    pub body: String,
}

/// A repository's pages, as `Wiki` replies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WikiListing {
    /// The repository the pages are for.
    pub root: PathBuf,
    pub pages: Vec<WikiPage>,
}

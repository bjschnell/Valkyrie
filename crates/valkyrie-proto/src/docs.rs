//! Docs (ADR-0021): markdown agents write for a person, in a folder the user picks.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What a doc is for. A front matter field, not a folder, so agents never pick one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DocKind {
    Plan,
    Research,
    Report,
    #[default]
    Note,
}

impl DocKind {
    pub const ALL: [DocKind; 4] = [Self::Plan, Self::Research, Self::Report, Self::Note];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Research => "research",
            Self::Report => "report",
            Self::Note => "note",
        }
    }

    pub fn parse(s: &str) -> Option<DocKind> {
        Self::ALL.into_iter().find(|k| k.as_str() == s.trim())
    }
}

/// A doc to write. Who wrote it, and from which session and commit, the daemon
/// works out itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewDoc {
    /// Where the writer is: a doc from inside a repository goes in its folder.
    pub cwd: PathBuf,
    pub title: String,
    #[serde(default)]
    pub kind: DocKind,
    /// Agent-level, at the top of the docs folder, wherever `cwd` is.
    #[serde(default)]
    pub global: bool,
    pub body: String,
}

/// A doc as listed or read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Doc {
    /// Its `valk-id`; 0 for a file of the user's that hasn't been read through
    /// Valkyrie yet.
    pub id: u32,
    pub kind: DocKind,
    pub title: String,
    /// Relative to the docs folder.
    pub path: String,
    /// The agent that wrote it, or `human`.
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub created: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Who edited it since, one line each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edited: Vec<String>,
    /// The whole file's hash, which an edit names as its base; with `text` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// The whole file, front matter and all; only when read or written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

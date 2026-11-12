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
    /// What the web app opens it by (ADR-0021 §5): its `valk-id`, or for one of the
    /// user's own docs with no id, a handle the daemon made from its path. Never a
    /// path itself. Only in a list or a web read.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    /// An agent wrote or edited it since the user last opened the Docs list. Only
    /// in a list.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unread: bool,
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

/// One of a doc's past versions (ADR-0021 §4): the file as it was before an edit
/// replaced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocVersion {
    /// What `DocHistory` and `RestoreDoc` name it by.
    pub version: String,
    /// When it was replaced, in seconds.
    pub at: u64,
    /// Who wrote it: its last `edited` line, or its author.
    #[serde(default)]
    pub by: String,
    /// Its size in bytes.
    #[serde(default)]
    pub size: u64,
}

/// A doc the user deleted, kept so it can be put back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletedDoc {
    /// What `UndeleteDoc` takes.
    pub token: String,
    /// When it was deleted, in seconds.
    pub at: u64,
    /// Where it was, relative to the docs folder.
    pub path: String,
    pub title: String,
}

/// Where a comment on a doc stands (ADR-0021 §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CommentState {
    /// Not yet told to the session that wrote the doc: it's gone, or hasn't
    /// prompted since.
    #[default]
    Open,
    /// Told to that session at its prompt; waiting on it to edit the doc.
    Sent,
    /// That session edited the doc after it was told.
    Answered,
    /// The user closed it.
    Resolved,
}

/// A comment the user left on a doc, or on one block of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocComment {
    /// Its number within the doc, from 1.
    pub id: u32,
    /// When it was left, in seconds.
    pub at: u64,
    /// The block it's about, as it read then; none for the whole doc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    pub text: String,
    #[serde(default)]
    pub state: CommentState,
    /// The session it was told to, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_to: Option<String>,
    /// When it last changed state, in seconds.
    #[serde(default)]
    pub changed: u64,
}

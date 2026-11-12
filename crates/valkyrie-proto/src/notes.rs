//! Agent notes (ADR-0017): short, unreviewed texts a tier below decisions, in the
//! same store. Never injected anywhere; an agent reads one only when it asks.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Where a note stands. Only a live one is ever served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NoteStatus {
    #[default]
    Live,
    /// A correction replaced it (`replaced_by`). Kept only so the chain reads.
    Replaced,
    /// A decision accepted from it now carries what it said (ADR-0017 §7).
    Promoted,
}

impl NoteStatus {
    pub const ALL: [NoteStatus; 3] = [Self::Live, Self::Replaced, Self::Promoted];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Replaced => "replaced",
            Self::Promoted => "promoted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// One note, as stored in `<state dir>/context/<project>/notes/NNNN.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: u32,
    /// The repository it belongs to. Not stored in the file: the directory says.
    pub project: PathBuf,
    pub text: String,
    pub status: NoteStatus,
    pub created: u64,
    /// `agent`, `user` or `valkyrie`.
    pub by: String,
    /// The Valkyrie session it was written in, or `none`.
    pub session: String,
    #[serde(default)]
    pub conversation: Option<String>,
    /// The checkout's `HEAD` when it was written, read by the daemon itself.
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// The files its text names, found when it was written.
    #[serde(default)]
    pub anchors: Vec<String>,
    /// The failure signature it came from, for a note the retro wrote.
    #[serde(default)]
    pub sig: Option<String>,
    #[serde(default)]
    pub replaces: Option<u32>,
    #[serde(default)]
    pub replaced_by: Option<u32>,
    /// Why it may be out of date (ADR-0017 §6). Worked out when it's read, never
    /// stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale: Option<String>,
}

/// A note to write, as a client asks for it. Where it came from is the daemon's to
/// work out, never the client's (ADR-0017 rule 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewNote {
    pub cwd: PathBuf,
    pub text: String,
    /// The note this one corrects: it becomes `replaced` in the same write.
    #[serde(default)]
    pub replaces: Option<u32>,
    /// Write it even though it matches a deleted note. Only a human may.
    #[serde(default)]
    pub force: bool,
}

/// What `valk notes rm` leaves behind in `notes/deleted.jsonl`, so nothing writes the
/// same note again (ADR-0017 rule 4). The text itself is gone: only hashes of its
/// words are kept, enough to recognise it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tombstone {
    pub id: u32,
    #[serde(default)]
    pub sig: Option<String>,
    /// Hashes of the words that carry its meaning, sorted.
    pub words: Vec<String>,
    /// `agent` or `user`.
    pub by: String,
    pub session: String,
    pub at: u64,
}

/// A project's notes, every status, with its tombstones.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotesListing {
    pub notes: Vec<Note>,
    pub deleted: Vec<Tombstone>,
}

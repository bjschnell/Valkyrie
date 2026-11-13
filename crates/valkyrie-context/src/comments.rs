//! Comments on docs (ADR-0021 §6): what the user says about a doc, or one block of
//! it, kept beside the docs in `<state dir>/docs/comments/<valk-id>.jsonl`, never
//! in the markdown, so the file stays clean and commenting is never an edit.
//!
//! The file is a log: a comment, then what became of it (told to the session
//! that wrote the doc, answered by that session's edit, or closed by the user).
//! A comment's state is the last of those.

use crate::shown;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use valkyrie_proto::{CommentState, DocComment};

/// Longest comment, in bytes.
pub const MAX_TEXT: usize = 4 * 1024;
/// Longest quote kept, in characters: enough to find the block again.
const MAX_QUOTE: usize = 500;
/// Most comments on one doc.
const MAX_COMMENTS: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Event {
    Comment {
        id: u32,
        at: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quote: Option<String>,
        text: String,
    },
    /// Told to `session` (its name), whose agent's conversation is `conversation`.
    Sent {
        id: u32,
        at: u64,
        session: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation: Option<String>,
    },
    Answered {
        id: u32,
        at: u64,
    },
    Resolved {
        id: u32,
        at: u64,
    },
}

/// A comment as folded from the log, with whom it was told to.
#[derive(Debug, Clone)]
struct Folded {
    comment: DocComment,
    conversation: Option<String>,
}

/// The comments on every doc, under `<state dir>/docs/comments`.
pub struct Comments {
    dir: PathBuf,
}

impl Comments {
    pub fn new(state: &Path) -> Self {
        Self {
            dir: state.join("docs").join("comments"),
        }
    }

    fn file(&self, doc: u32) -> PathBuf {
        self.dir.join(format!("{doc}.jsonl"))
    }

    fn events(&self, doc: u32) -> Vec<Event> {
        std::fs::read_to_string(self.file(doc))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    fn fold(&self, doc: u32) -> Vec<Folded> {
        let mut out: Vec<Folded> = Vec::new();
        for event in self.events(doc) {
            let (id, at) = match &event {
                Event::Comment { id, at, .. }
                | Event::Sent { id, at, .. }
                | Event::Answered { id, at }
                | Event::Resolved { id, at } => (*id, *at),
            };
            if let Event::Comment { quote, text, .. } = event {
                if out.iter().all(|f| f.comment.id != id) {
                    out.push(Folded {
                        comment: DocComment {
                            id,
                            at,
                            quote,
                            text,
                            state: CommentState::Open,
                            sent_to: None,
                            changed: at,
                        },
                        conversation: None,
                    });
                }
                continue;
            }
            let Some(f) = out.iter_mut().find(|f| f.comment.id == id) else {
                continue;
            };
            // Resolved is final; the rest follow each other.
            if f.comment.state == CommentState::Resolved {
                continue;
            }
            f.comment.changed = at;
            match event {
                Event::Sent {
                    session,
                    conversation,
                    ..
                } => {
                    f.comment.state = CommentState::Sent;
                    f.comment.sent_to = Some(session);
                    f.conversation = conversation;
                }
                Event::Answered { .. } => f.comment.state = CommentState::Answered,
                Event::Resolved { .. } => f.comment.state = CommentState::Resolved,
                Event::Comment { .. } => {}
            }
        }
        out
    }

    /// The comments on `doc`, oldest first.
    pub fn list(&self, doc: u32) -> Vec<DocComment> {
        self.fold(doc).into_iter().map(|f| f.comment).collect()
    }

    fn append(&self, doc: u32, event: &Event) -> Result<()> {
        valkyrie_proto::private_dir_all(&self.dir)?;
        let mut line = serde_json::to_string(event)?;
        line.push('\n');
        let path = self.file(doc);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        file.write_all(line.as_bytes())?;
        Ok(())
    }

    /// Adds the user's comment on `doc`: on the block `quote` reads, or the whole
    /// doc with none.
    pub fn add(&self, doc: u32, quote: Option<&str>, text: &str, now: u64) -> Result<DocComment> {
        let text = shown(text, true).trim().to_string();
        if text.is_empty() {
            bail!("a comment needs some text");
        }
        if text.len() > MAX_TEXT {
            bail!("a comment is at most {} KB", MAX_TEXT / 1024);
        }
        let quote = quote
            .map(|q| {
                let q = crate::file::one_line(&shown(q, false));
                let q = q.trim();
                match q.chars().count() > MAX_QUOTE {
                    true => format!("{}…", q.chars().take(MAX_QUOTE).collect::<String>()),
                    false => q.to_string(),
                }
            })
            .filter(|q| !q.is_empty());
        let had = self.fold(doc);
        if had.len() >= MAX_COMMENTS {
            bail!("doc {doc} has {MAX_COMMENTS} comments already");
        }
        let id = had.iter().map(|f| f.comment.id).max().unwrap_or(0) + 1;
        self.append(
            doc,
            &Event::Comment {
                id,
                at: now,
                quote: quote.clone(),
                text: text.clone(),
            },
        )?;
        Ok(DocComment {
            id,
            at: now,
            quote,
            text,
            state: CommentState::Open,
            sent_to: None,
            changed: now,
        })
    }

    /// The open comment `id` on `doc`, if it's still open: to tell its session.
    pub fn open_one(&self, doc: u32, id: u32) -> Option<DocComment> {
        self.list(doc)
            .into_iter()
            .find(|c| c.id == id && c.state == CommentState::Open)
    }

    /// Notes that comment `id` on `doc` was told to `session`.
    pub fn sent(
        &self,
        doc: u32,
        id: u32,
        session: &str,
        conversation: Option<&str>,
        now: u64,
    ) -> Result<()> {
        self.append(
            doc,
            &Event::Sent {
                id,
                at: now,
                session: session.to_string(),
                conversation: conversation.map(str::to_string),
            },
        )
    }

    /// `doc` was edited by the session `session` (`conversation`): every comment
    /// told to it and not yet answered is answered. Returns how many.
    pub fn edited_by(
        &self,
        doc: u32,
        session: Option<&str>,
        conversation: Option<&str>,
        now: u64,
    ) -> Result<usize> {
        let answered: Vec<u32> = self
            .fold(doc)
            .into_iter()
            .filter(|f| f.comment.state == CommentState::Sent)
            .filter(|f| match (&f.conversation, conversation) {
                (Some(told), Some(by)) => told == by,
                // Without a conversation id to go by, the session's name.
                (None, _) => session.is_some() && f.comment.sent_to.as_deref() == session,
                (Some(_), None) => false,
            })
            .map(|f| f.comment.id)
            .collect();
        for &id in &answered {
            self.append(doc, &Event::Answered { id, at: now })?;
        }
        Ok(answered.len())
    }

    /// The user closes comment `id` on `doc`.
    pub fn resolve(&self, doc: u32, id: u32, now: u64) -> Result<()> {
        if !self.list(doc).iter().any(|c| c.id == id) {
            bail!("doc {doc} has no comment {id}");
        }
        self.append(doc, &Event::Resolved { id, at: now })
    }
}

/// What the session that wrote a doc is told of a comment on it, at its next
/// prompt: the user's words, about its own doc, and how to answer.
pub fn told(doc: u32, title: &str, comment: &DocComment, valk: &str) -> String {
    let about = match &comment.quote {
        Some(quote) => format!(", about “{quote}”"),
        None => String::new(),
    };
    format!(
        "The user commented on your doc “{title}” (doc {doc}){about}: {}\n\
         These are the user's words about a doc you wrote. If it calls for a change, \
         answer by editing the doc: `{valk} doc read {doc}`, then `{valk} doc edit {doc} --base <hash> -`.",
        comment.text
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state dir of its own, removed when dropped.
    struct Dir(PathBuf);

    impl Dir {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn store() -> (Dir, Comments) {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("valk-comments-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let comments = Comments::new(&dir);
        (Dir(dir), comments)
    }

    #[test]
    fn a_comment_starts_open_and_keeps_its_quote() {
        let (_dir, c) = store();
        let one = c
            .add(3, Some("## Plan\u{202e}"), "  why this order?  ", 10)
            .unwrap();
        assert_eq!(one.id, 1);
        assert_eq!(one.quote.as_deref(), Some("## Plan"));
        assert_eq!(one.text, "why this order?");
        let two = c.add(3, None, "whole doc", 11).unwrap();
        assert_eq!(two.id, 2);
        assert_eq!(two.quote, None);
        let listed = c.list(3);
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|c| c.state == CommentState::Open));
        assert!(c.list(4).is_empty(), "another doc's comments are its own");
    }

    #[test]
    fn empty_or_huge_comments_are_refused() {
        let (_dir, c) = store();
        assert!(c.add(1, None, " \n", 1).is_err());
        assert!(c.add(1, None, &"x".repeat(MAX_TEXT + 1), 1).is_err());
        let long = c
            .add(1, Some(&"q".repeat(MAX_QUOTE + 50)), "ok", 1)
            .unwrap();
        assert_eq!(long.quote.unwrap().chars().count(), MAX_QUOTE + 1);
    }

    #[test]
    fn only_the_session_it_was_told_to_answers_it() {
        let (_dir, c) = store();
        c.add(5, None, "first", 1).unwrap();
        c.add(5, None, "second", 2).unwrap();
        c.sent(5, 1, "#4 docs", Some("conv-a"), 3).unwrap();
        let listed = c.list(5);
        assert_eq!(listed[0].state, CommentState::Sent);
        assert_eq!(listed[0].sent_to.as_deref(), Some("#4 docs"));
        assert_eq!(listed[1].state, CommentState::Open);

        // Another session's edit answers nothing.
        assert_eq!(
            c.edited_by(5, Some("#9 other"), Some("conv-b"), 4).unwrap(),
            0
        );
        assert_eq!(c.edited_by(5, Some("#4 docs"), None, 4).unwrap(), 0);
        assert_eq!(
            c.edited_by(5, Some("#4 docs"), Some("conv-a"), 5).unwrap(),
            1
        );
        let listed = c.list(5);
        assert_eq!(listed[0].state, CommentState::Answered);
        assert_eq!(listed[0].changed, 5);
        assert_eq!(
            listed[1].state,
            CommentState::Open,
            "never told, so not answered"
        );
    }

    #[test]
    fn without_a_conversation_the_name_answers() {
        let (_dir, c) = store();
        c.add(5, None, "first", 1).unwrap();
        c.sent(5, 1, "#4 docs", None, 2).unwrap();
        assert_eq!(
            c.edited_by(5, Some("#4 docs"), Some("conv-a"), 3).unwrap(),
            1
        );
    }

    #[test]
    fn resolved_is_final() {
        let (_dir, c) = store();
        c.add(2, None, "never mind", 1).unwrap();
        c.resolve(2, 1, 2).unwrap();
        c.sent(2, 1, "#4 docs", Some("conv-a"), 3).unwrap();
        assert_eq!(c.list(2)[0].state, CommentState::Resolved);
        assert!(c.open_one(2, 1).is_none());
        assert!(c.resolve(2, 7, 4).is_err());
    }

    #[test]
    fn a_torn_line_is_skipped() {
        let (dir, c) = store();
        c.add(8, None, "kept", 1).unwrap();
        let path = dir.path().join("docs/comments/8.jsonl");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"t\":\"comm");
        std::fs::write(&path, text).unwrap();
        assert_eq!(c.list(8).len(), 1);
        assert_eq!(c.add(8, None, "next", 2).unwrap().id, 2);
    }

    #[test]
    fn the_told_line_frames_it_as_the_users_words() {
        let comment = DocComment {
            id: 1,
            at: 0,
            quote: Some("Ship it Friday".into()),
            text: "make it Monday".into(),
            state: CommentState::Open,
            sent_to: None,
            changed: 0,
        };
        let line = told(4, "Release plan", &comment, "valk");
        assert!(line.starts_with("The user commented on your doc “Release plan” (doc 4), about “Ship it Friday”: make it Monday"));
        assert!(line.contains("`valk doc read 4`"));
    }
}

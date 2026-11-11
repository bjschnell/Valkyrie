//! Docs (ADR-0021): the daemon writes and edits them for agents, with who and where
//! from worked out from the caller, and lets only the user move the folder.

use crate::Registry;
use crate::context::{Peer, now_secs};
use crate::session::Session;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use valkyrie_context::docs::{Author, Deleted, Origin, Saved};
use valkyrie_proto::{NewDoc, Reply};

/// The user, writing from the web app. Vouched for, so not an agent; and not
/// the session `valk web` happens to run in, which would claim the edit.
fn you() -> Author {
    Author {
        by: "human".into(),
        ..Author::default()
    }
}

/// A save's reply, logged when it wrote.
fn saved_reply(saved: Saved, what: &str) -> Reply {
    match saved {
        Saved::Done(doc) => {
            tracing::info!(id = doc.id, path = doc.path, "{what}");
            Reply::DocSaved {
                doc: Box::new(doc),
                conflict: false,
            }
        }
        Saved::Conflict(doc) => Reply::DocSaved {
            doc: Box::new(doc),
            conflict: true,
        },
    }
}

impl Registry {
    /// Who is writing, from the connecting process: never from what it claims.
    fn author(&self, peer: Option<Peer>, cwd: Option<&std::path::Path>) -> Author {
        let caller = self.caller(peer);
        let session = caller.session.as_ref();
        Author {
            by: caller.agent.clone().unwrap_or_else(|| "human".into()),
            session: session.map(|s| s.info().name),
            conversation: session.and_then(|s| s.conversation()),
            commit: cwd.and_then(valkyrie_context::project::head),
            item: session
                .and_then(|s| self.item_of(s.info().id))
                .map(|i| i.id),
        }
    }

    pub(crate) fn write_doc(&self, new: NewDoc, peer: Option<Peer>) -> Result<Reply> {
        let author = self.author(peer, Some(&new.cwd));
        let doc = self.docs.lock().unwrap().write(&new, &author, now_secs())?;
        tracing::info!(id = doc.id, path = doc.path, by = author.by, "doc written");
        Ok(Reply::Doc { doc: Box::new(doc) })
    }

    pub(crate) fn read_doc(&self, which: &str) -> Result<Reply> {
        let doc = self.docs.lock().unwrap().read(which)?;
        Ok(Reply::Doc { doc: Box::new(doc) })
    }

    pub(crate) fn edit_doc(
        &self,
        id: u32,
        base: &str,
        body: &str,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        let author = self.author(peer, None);
        let doc = self
            .docs
            .lock()
            .unwrap()
            .edit(id, base, body, &author, now_secs())?;
        tracing::info!(id, path = doc.path, by = author.by, "doc edited");
        // An edit by the session a comment was told to answers it (ADR-0021 §6).
        if author.session.is_some() || author.conversation.is_some() {
            let docs = self.docs.lock().unwrap();
            match docs.comments().edited_by(
                id,
                author.session.as_deref(),
                author.conversation.as_deref(),
                now_secs(),
            ) {
                Ok(0) => {}
                Ok(n) => tracing::info!(id, n, "doc comments answered"),
                Err(e) => tracing::warn!(id, "doc comments: {e:#}"),
            }
        }
        Ok(Reply::Doc { doc: Box::new(doc) })
    }

    /// The user's save from the web app's editor (ADR-0021 §7). Only a human
    /// saves this way; agents edit by id with a base hash (`edit_doc`).
    pub(crate) fn save_doc(
        &self,
        key: &str,
        base: &str,
        body: &str,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        self.vouch(peer, "saving a doc from the web app")?;
        let author = you();
        let saved = self
            .docs
            .lock()
            .unwrap()
            .save_body(key, base, body, &author, now_secs())?;
        Ok(saved_reply(saved, "doc saved"))
    }

    /// A doc's past versions, and one's text when asked.
    pub(crate) fn doc_history(&self, key: &str, version: Option<&str>) -> Result<Reply> {
        let mut docs = self.docs.lock().unwrap();
        let versions = docs.versions(key)?;
        let text = version.map(|v| docs.version_text(key, v)).transpose()?;
        Ok(Reply::DocHistory { versions, text })
    }

    /// The user puts a past version back.
    pub(crate) fn restore_doc(
        &self,
        key: &str,
        base: &str,
        version: &str,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        self.vouch(peer, "restoring a doc")?;
        let author = you();
        let saved = self
            .docs
            .lock()
            .unwrap()
            .restore(key, base, version, &author, now_secs())?;
        Ok(saved_reply(saved, "doc restored"))
    }

    /// The user deletes a doc; the daemon never does on its own (ADR-0021 §4).
    pub(crate) fn delete_doc(&self, key: &str, base: &str, peer: Option<Peer>) -> Result<Reply> {
        self.vouch(peer, "deleting a doc")?;
        match self.docs.lock().unwrap().delete(key, base, now_secs())? {
            Deleted::Done(token) => {
                tracing::info!(key, token, "doc deleted");
                Ok(Reply::DocDeleted { token })
            }
            Deleted::Conflict(doc) => Ok(Reply::DocSaved {
                doc,
                conflict: true,
            }),
        }
    }

    /// The docs the user deleted that can still be put back.
    pub(crate) fn deleted_docs(&self) -> Result<Reply> {
        Ok(Reply::DeletedDocs {
            docs: self.docs.lock().unwrap().deleted(),
        })
    }

    /// Puts a doc the user deleted back.
    pub(crate) fn undelete_doc(&self, token: &str, peer: Option<Peer>) -> Result<Reply> {
        self.vouch(peer, "putting back a deleted doc")?;
        let doc = self.docs.lock().unwrap().undelete(token)?;
        tracing::info!(token, path = doc.path, "doc put back");
        Ok(Reply::Doc { doc: Box::new(doc) })
    }

    /// The comments on the doc with this key; none on one without a `valk-id`.
    pub(crate) fn doc_comments(&self, key: &str) -> Result<Reply> {
        let mut docs = self.docs.lock().unwrap();
        let id = docs.open(key)?.id;
        let comments = match id {
            0 => Vec::new(),
            id => docs.comments().list(id),
        };
        Ok(Reply::DocComments { comments, to: None })
    }

    /// The user's comment on a doc (ADR-0021 §6). Only a human comments. If the
    /// session that wrote the doc is still running, it hears it at its next
    /// prompt; otherwise the comment stays open on the doc.
    pub(crate) fn comment_doc(
        &self,
        key: &str,
        quote: Option<&str>,
        text: &str,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        self.vouch(peer, "commenting on a doc")?;
        let mut docs = self.docs.lock().unwrap();
        let origin = docs.origin(key)?;
        let comment = docs.comments().add(origin.id, quote, text, now_secs())?;
        let to = self.author_session(&origin);
        if let Some(session) = &to {
            session.doc_commented(origin.id, comment.id);
        }
        let to = to.map(|s| s.info().name);
        tracing::info!(doc = origin.id, comment = comment.id, to, "doc commented");
        Ok(Reply::DocComments {
            comments: docs.comments().list(origin.id),
            to,
        })
    }

    /// The user closes a comment.
    pub(crate) fn resolve_comment(
        &self,
        key: &str,
        comment: u32,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        self.vouch(peer, "closing a doc comment")?;
        let mut docs = self.docs.lock().unwrap();
        let origin = docs.origin(key)?;
        docs.comments().resolve(origin.id, comment, now_secs())?;
        Ok(Reply::DocComments {
            comments: docs.comments().list(origin.id),
            to: None,
        })
    }

    /// The running session that wrote a doc: by its agent's conversation, or by
    /// the session's name when the doc has none. Only Claude's for now; Codex's
    /// prompt hook comes later (ADR-0021, Not now).
    fn author_session(&self, origin: &Origin) -> Option<Arc<Session>> {
        if origin.by == "human" {
            return None;
        }
        self.listed()
            .into_iter()
            .filter(|s| !s.exited() && s.agent_behind().as_deref() == Some("claude"))
            .find(|s| match (&origin.conversation, &origin.session) {
                (Some(conversation), _) => s.conversation().as_ref() == Some(conversation),
                (None, Some(name)) => s.info().name == *name,
                (None, None) => false,
            })
    }

    /// What `session`'s agent is told of comments the user left on its docs
    /// since its last prompt, one line each. Each is marked sent; one the user
    /// closed meanwhile is skipped.
    pub(crate) fn comments_for(&self, session: &Session) -> Vec<String> {
        let queued = session.take_comments();
        if queued.is_empty() {
            return Vec::new();
        }
        let name = session.info().name;
        let conversation = session.conversation();
        let mut docs = self.docs.lock().unwrap();
        let comments = docs.comments();
        let now = now_secs();
        queued
            .into_iter()
            .filter_map(|(doc, id)| {
                let comment = comments.open_one(doc, id)?;
                let title = docs.origin(&doc.to_string()).ok()?.title;
                if let Err(e) = comments.sent(doc, id, &name, conversation.as_deref(), now) {
                    tracing::warn!(doc, id, "doc comment: {e:#}");
                }
                Some(valkyrie_context::comments::told(
                    doc, &title, &comment, "valk",
                ))
            })
            .collect()
    }

    /// The web app's read: by the key a list gave out, writing nothing (ADR-0021 §5).
    pub(crate) fn open_doc(&self, key: &str) -> Result<Reply> {
        let doc = self.docs.lock().unwrap().open(key)?;
        Ok(Reply::Doc { doc: Box::new(doc) })
    }

    pub(crate) fn docs(&self, count_only: bool) -> Result<Reply> {
        let mut docs = self.docs.lock().unwrap();
        let at = now_secs();
        if count_only {
            return Ok(Reply::Docs {
                unread: docs.unread_count()?,
                docs: Vec::new(),
                root: docs.root(),
                at,
            });
        }
        let list = docs.list()?;
        Ok(Reply::Docs {
            unread: valkyrie_context::docs::Docs::unread(&list),
            docs: list,
            root: docs.root(),
            at,
        })
    }

    /// The user opened the Docs list: what agents wrote until now counts as read.
    pub(crate) fn docs_seen(&self, at: u64, peer: Option<Peer>) -> Result<Reply> {
        self.vouch(peer, "marking docs read")?;
        self.docs.lock().unwrap().seen(at, now_secs())?;
        Ok(Reply::Done)
    }

    /// Where docs go. Setting it is the user's (ADR-0021 §1): an agent that could
    /// aim it at `~/.ssh` would have the web app serve it.
    pub(crate) fn docs_root(&self, root: Option<PathBuf>, peer: Option<Peer>) -> Result<Reply> {
        let mut docs = self.docs.lock().unwrap();
        let Some(root) = root else {
            let how = if docs.root_is_set() {
                ""
            } else {
                " (the default)"
            };
            return Ok(Reply::Text {
                text: format!("{}{how}", docs.root().display()),
            });
        };
        self.vouch(peer, "moving the docs folder")?;
        let root = (!root.as_os_str().is_empty()).then_some(root);
        let set = docs.set_root(root.as_deref())?;
        tracing::info!(root = %set.display(), "docs folder set");
        Ok(Reply::Text {
            text: set.display().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{AS_CALLER, Caller};
    use valkyrie_proto::{CommentState, DocKind, Size, SpawnSpec};

    fn as_caller(agent: Option<&str>, session: Option<Arc<Session>>) {
        let agent = agent.map(str::to_string);
        AS_CALLER.with(|c| {
            *c.borrow_mut() = Some(Box::new(move || Caller {
                agent: agent.clone(),
                session: session.clone(),
                bridge: false,
                desktop: false,
            }))
        });
    }

    fn states(reply: Result<Reply>) -> Vec<CommentState> {
        match reply.unwrap() {
            Reply::DocComments { comments, .. } => comments.iter().map(|c| c.state).collect(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_comment_reaches_the_session_that_wrote_the_doc_once_and_its_edit_answers_it() {
        let dir =
            std::env::temp_dir().join(format!("valkyrie-doc-comments-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (registry, _) = crate::new_registry(
            dir.join("v.sock"),
            &dir.join("state"),
            std::path::Path::new("/bin/true"),
            1,
            0,
            7,
            false,
        )
        .unwrap();
        let spawn = |conversation: &str| {
            let spec = SpawnSpec {
                command: vec!["sleep".into(), "30".into()],
                cwd: Some(dir.clone()),
                name: None,
                size: Size { cols: 40, rows: 10 },
                env: Vec::new(),
                worktree: None,
                hidden: false,
            };
            let Reply::Session { info } = crate::spawn(&registry, spec).unwrap() else {
                panic!("no session");
            };
            let session = registry.get(info.id).unwrap();
            session.driven_by("claude");
            session.set_conversation(Some(conversation.into()));
            session
        };
        let (author, other) = (spawn("conv-a"), spawn("conv-b"));

        as_caller(Some("claude"), Some(author.clone()));
        let new = NewDoc {
            cwd: dir.clone(),
            title: "Release plan".into(),
            kind: DocKind::Plan,
            global: true,
            body: "# Plan\n\nShip it Friday.\n".into(),
        };
        let Reply::Doc { doc } = registry.write_doc(new, None).unwrap() else {
            panic!("no doc");
        };
        let key = doc.id.to_string();

        // Only a human comments.
        let refused = registry.comment_doc(&key, None, "from an agent", None);
        assert!(
            refused.is_err_and(|e| e.to_string().contains("is for the user")),
            "an agent commented"
        );

        as_caller(None, None);
        let Reply::DocComments { comments, to } = registry
            .comment_doc(&key, Some("Ship it Friday."), "make it Monday", None)
            .unwrap()
        else {
            panic!("no comments");
        };
        assert_eq!(comments[0].state, CommentState::Open);
        assert_eq!(to, Some(author.info().name), "not queued to the author");
        assert!(
            registry.comments_for(&other).is_empty(),
            "told another session"
        );
        let told = registry.comments_for(&author);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(
            told[0].starts_with("The user commented on your doc “Release plan”")
                && told[0].contains("about “Ship it Friday.”: make it Monday")
                && told[0].contains(&format!("valk doc edit {key} --base")),
            "{}",
            told[0]
        );
        assert!(registry.comments_for(&author).is_empty(), "told twice");
        assert_eq!(states(registry.doc_comments(&key)), [CommentState::Sent]);

        // Another session's edit isn't the answer; the author's is.
        as_caller(Some("claude"), Some(other.clone()));
        let Reply::Doc { doc } = registry
            .edit_doc(
                doc.id,
                doc.hash.as_deref().unwrap(),
                "# Plan\n\nShip it Friday!\n",
                None,
            )
            .unwrap()
        else {
            panic!("no doc");
        };
        assert_eq!(states(registry.doc_comments(&key)), [CommentState::Sent]);
        as_caller(Some("claude"), Some(author.clone()));
        registry
            .edit_doc(
                doc.id,
                doc.hash.as_deref().unwrap(),
                "# Plan\n\nShip it Monday.\n",
                None,
            )
            .unwrap();
        assert_eq!(
            states(registry.doc_comments(&key)),
            [CommentState::Answered]
        );

        // With nobody running the conversation that wrote it, a comment stays open;
        // the user can close it.
        author.set_conversation(Some("conv-later".into()));
        as_caller(None, None);
        let said = states(registry.comment_doc(&key, None, "and the changelog?", None));
        assert_eq!(said, [CommentState::Answered, CommentState::Open]);
        assert!(registry.comments_for(&author).is_empty());
        as_caller(Some("claude"), Some(author.clone()));
        assert!(
            registry.resolve_comment(&key, 2, None).is_err(),
            "an agent closed it"
        );
        as_caller(None, None);
        let said = states(registry.resolve_comment(&key, 2, None));
        assert_eq!(said, [CommentState::Answered, CommentState::Resolved]);

        AS_CALLER.with(|c| *c.borrow_mut() = None);
        author.kill();
        other.kill();
        // On Windows a killed session can still hold its files a moment (#113).
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_comment_the_user_closed_before_the_prompt_isnt_told() {
        let dir = std::env::temp_dir().join(format!("valkyrie-doc-closed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (registry, _) = crate::new_registry(
            dir.join("v.sock"),
            &dir.join("state"),
            std::path::Path::new("/bin/true"),
            1,
            0,
            7,
            false,
        )
        .unwrap();
        let spec = SpawnSpec {
            command: vec!["sleep".into(), "30".into()],
            cwd: Some(dir.clone()),
            name: None,
            size: Size { cols: 40, rows: 10 },
            env: Vec::new(),
            worktree: None,
            hidden: false,
        };
        let Reply::Session { info } = crate::spawn(&registry, spec).unwrap() else {
            panic!("no session");
        };
        let author = registry.get(info.id).unwrap();
        author.driven_by("claude");
        author.set_conversation(Some("conv-a".into()));
        as_caller(Some("claude"), Some(author.clone()));
        let new = NewDoc {
            cwd: dir.clone(),
            title: "Notes".into(),
            kind: DocKind::Note,
            global: true,
            body: "Some notes.\n".into(),
        };
        let Reply::Doc { doc } = registry.write_doc(new, None).unwrap() else {
            panic!("no doc");
        };
        let key = doc.id.to_string();
        as_caller(None, None);
        registry.comment_doc(&key, None, "oops", None).unwrap();
        registry.resolve_comment(&key, 1, None).unwrap();
        assert!(
            registry.comments_for(&author).is_empty(),
            "told a closed comment"
        );
        AS_CALLER.with(|c| *c.borrow_mut() = None);
        author.kill();
        // On Windows a killed session can still hold its files a moment (#113).
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_user_saves_restores_and_deletes_from_the_web_app() {
        let dir = std::env::temp_dir().join(format!("valkyrie-doc-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (registry, _) = crate::new_registry(
            dir.join("v.sock"),
            &dir.join("state"),
            std::path::Path::new("/bin/true"),
            1,
            0,
            7,
            false,
        )
        .unwrap();
        as_caller(Some("claude"), None);
        let new = NewDoc {
            cwd: dir.clone(),
            title: "Notes".into(),
            kind: DocKind::Note,
            global: true,
            body: "first\n".into(),
        };
        let Reply::Doc { doc } = registry.write_doc(new, None).unwrap() else {
            panic!("no doc");
        };
        let (key, base) = (doc.id.to_string(), doc.hash.clone().unwrap());
        for refused in [
            registry.save_doc(&key, &base, "agent\n", None),
            registry.restore_doc(&key, &base, "1", None),
            registry.delete_doc(&key, &base, None),
            registry.undelete_doc("1", None),
        ] {
            assert!(
                refused.is_err_and(|e| e.to_string().contains("is for the user")),
                "an agent got through"
            );
        }

        as_caller(None, None);
        let Reply::DocSaved { doc, conflict } =
            registry.save_doc(&key, &base, "mine\n", None).unwrap()
        else {
            panic!("no save");
        };
        assert!(!conflict);
        assert!(
            doc.edited.iter().any(|e| e.starts_with("human, ")),
            "{:?}",
            doc.edited
        );
        // The stale hash is now a conflict, with what's on disk.
        let Reply::DocSaved { doc: now, conflict } =
            registry.save_doc(&key, &base, "again\n", None).unwrap()
        else {
            panic!("no save");
        };
        assert!(conflict);
        assert_eq!(now.hash, doc.hash);

        let Reply::DocHistory { versions, text } = registry.doc_history(&key, None).unwrap() else {
            panic!("no history");
        };
        assert_eq!(versions.len(), 1);
        assert!(text.is_none());
        let Reply::DocSaved { doc, conflict } = registry
            .restore_doc(
                &key,
                doc.hash.as_deref().unwrap(),
                &versions[0].version,
                None,
            )
            .unwrap()
        else {
            panic!("no restore");
        };
        assert!(!conflict);
        assert!(doc.text.unwrap().ends_with("first\n"));

        let Reply::DocDeleted { token } = registry
            .delete_doc(&key, doc.hash.as_deref().unwrap(), None)
            .unwrap()
        else {
            panic!("no delete");
        };
        let Reply::Doc { doc: back } = registry.undelete_doc(&token, None).unwrap() else {
            panic!("not put back");
        };
        assert_eq!(back.key, key);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

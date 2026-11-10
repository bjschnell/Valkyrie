//! Docs (ADR-0021): the daemon writes and edits them for agents, with who and where
//! from worked out from the caller, and lets only the user move the folder.

use crate::Registry;
use crate::context::{Peer, now_secs};
use anyhow::Result;
use std::path::PathBuf;
use valkyrie_context::docs::Author;
use valkyrie_proto::{NewDoc, Reply};

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
        Ok(Reply::Doc { doc: Box::new(doc) })
    }

    pub(crate) fn docs(&self) -> Result<Reply> {
        let docs = self.docs.lock().unwrap();
        Ok(Reply::Docs {
            docs: docs.list()?,
            root: docs.root(),
        })
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

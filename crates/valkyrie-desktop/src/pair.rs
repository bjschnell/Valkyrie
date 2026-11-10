//! The window pairs itself the way `valk web open` pairs a browser (ADR-0020 §5).
//!
//! The daemon vouches first, so an agent that starts the app gets no paired
//! device. Only then is a link minted, for the window to load; the page trades
//! its code for a device token that stays in the webview's storage. The app is
//! then one device in `valk web devices`, and `valk web revoke` removes it.
//!
//! A launch mints at most one link, however often the page says it isn't paired:
//! a page can't pull pairing codes on demand.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use valkyrie_proto::client::Client;

/// What the daemon is asked to vouch for; it names this in its refusal.
const WHAT: &str = "pairing the desktop app";

/// One launch's right to pair.
#[derive(Default)]
pub struct Pairing {
    minted: AtomicBool,
}

impl Pairing {
    pub fn new() -> Self {
        Self::default()
    }

    /// A link that pairs the window with the server at `url`, or `None` when this
    /// launch already had one. A refusal (an agent started the app) or a daemon
    /// that can't be reached uses the launch's one try too.
    pub async fn link(&self, socket: &Path, url: &str) -> Result<Option<String>> {
        if self.minted.swap(true, Ordering::SeqCst) {
            return Ok(None);
        }
        vouched(socket).await?;
        let store = valkyrie_web::store()?;
        Ok(Some(valkyrie_web::pairing_link(&store, url)?))
    }
}

/// Asks the daemon whether a person, not an agent, is behind this process.
async fn vouched(socket: &Path) -> Result<()> {
    let (client, _pushes) = Client::connect(socket)
        .await
        .with_context(|| format!("reach the daemon on {}", socket.display()))?;
    let protocol = client.hello().await?;
    if protocol != valkyrie_proto::PROTOCOL {
        bail!(
            "the running daemon speaks protocol {protocol}, this app {}; run `valk upgrade`",
            valkyrie_proto::PROTOCOL
        );
    }
    client.vouch(WHAT).await
}

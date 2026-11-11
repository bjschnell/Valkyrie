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
use valkyrie_proto::client::Client;

/// What the daemon is asked to vouch for; it names this in its refusal.
const WHAT: &str = valkyrie_proto::VOUCH_DESKTOP_PAIRING;

/// One launch's right to pair.
#[derive(Default)]
pub struct Pairing {
    /// The daemon has answered this launch's ask, yes or no. Held across the
    /// ask, so two at once can't both get through.
    asked: tokio::sync::Mutex<bool>,
}

impl Pairing {
    pub fn new() -> Self {
        Self::default()
    }

    /// A link that pairs the window with the server at `url`, or `None` when this
    /// launch already had its answer.
    ///
    /// The daemon's answer is what uses the launch's one try, a refusal (an agent
    /// started the app) as much as a yes. A daemon that can't be reached yet, at
    /// login or in the middle of an upgrade, decided nothing and minted nothing:
    /// that can be asked again.
    pub async fn link(&self, socket: &Path, url: &str) -> Result<Option<String>> {
        let mut asked = self.asked.lock().await;
        if *asked {
            return Ok(None);
        }
        let client = daemon(socket).await?;
        *asked = true;
        client.vouch(WHAT).await?;
        let store = valkyrie_web::store()?;
        Ok(Some(valkyrie_web::pairing_link(&store, url)?))
    }
}

/// The daemon, once it has said it speaks this app's protocol.
async fn daemon(socket: &Path) -> Result<Client> {
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
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_daemon_that_cant_be_reached_leaves_the_try_unused() {
        let pairing = Pairing::new();
        let nowhere = std::env::temp_dir().join(format!("valk-no-daemon-{}", std::process::id()));
        for _ in 0..2 {
            // An error both times, never "this launch had its one".
            assert!(
                pairing
                    .link(&nowhere, "http://127.0.0.1:8790")
                    .await
                    .is_err()
            );
        }
        assert!(!*pairing.asked.lock().await);
    }
}

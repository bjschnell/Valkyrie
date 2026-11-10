//! Getting to a local `valk web` the window can load (ADR-0020 §3, §4).
//!
//! The window loads `http://127.0.0.1:<port>`: always that spelling, since the
//! device token lives in per-origin storage. Before it does, the shell makes sure
//! something answers there and that it is this user's own `valk web`
//! (`valkyrie_web::proof`). When nothing answers it runs `valk setup web --local`
//! as a separate process: the same service, for this machine only.

use anyhow::{Context, Result};
use std::fmt;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;
use valkyrie_web::proof::Local;

/// The port `valk setup web` uses unless told otherwise.
pub const DEFAULT_PORT: u16 = 8790;
/// How long a service that was just set up or started gets to answer.
const START_WAIT: Duration = Duration::from_secs(15);
const START_POLL: Duration = Duration::from_millis(300);

/// What the shell does to the machine, apart from deciding when (`ensure`).
pub trait Host {
    /// Asks what listens on `addr` to prove it's this user's `valk web`.
    fn check(&self, addr: SocketAddr) -> impl Future<Output = Result<Local>>;
    /// Sets `valk web` up as a service for this machine only, on `addr`.
    fn setup(&self, addr: SocketAddr) -> impl Future<Output = Result<()>>;
    /// Where a `valk web` of this user's last said it listens, if one did.
    fn recorded(&self) -> Option<SocketAddr>;
}

/// Why the window can't load the web app. Each is shown to the user as it reads.
#[derive(Debug)]
pub enum StartError {
    /// Something else holds the port, or a `valk web` that can't prove itself.
    Stranger(SocketAddr),
    /// `valk setup web --local` failed; its own words.
    Setup(String),
    /// The service was set up, and nothing answered in time.
    Silent(SocketAddr),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stranger(addr) => write!(
                f,
                "Something answers on {addr}, and it isn't your valk web. If it is an older \
                 valk web, restart it; otherwise stop what holds the port."
            ),
            Self::Setup(why) => write!(f, "Couldn't set up valk web on this machine: {why}"),
            Self::Silent(addr) => write!(
                f,
                "valk web was set up, but nothing answers on {addr}. `valk setup web --local` \
                 in a terminal says where its logs are."
            ),
        }
    }
}

impl std::error::Error for StartError {}

/// The address the window loads: 127.0.0.1 and the port, whatever the server
/// called itself.
pub fn url(addr: SocketAddr) -> String {
    format!("http://{}:{}", Ipv4Addr::LOCALHOST, addr.port())
}

fn local(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

/// Makes sure this user's `valk web` answers locally, and says where.
///
/// A `valk web` that is already running is used where it is, whichever port it
/// was set up on. Only when none answers is one set up, on the default port. A
/// stranger on the port is never loaded and never set up over.
pub async fn ensure(host: &impl Host) -> Result<SocketAddr, StartError> {
    let check = |addr| async move {
        // A check that couldn't be made is no proof.
        host.check(addr).await.unwrap_or(Local::Stranger)
    };
    // Where a valk web last said it was: the user may have set another port.
    if let Some(recorded) = host.recorded().map(|addr| local(addr.port()))
        && check(recorded).await == Local::Ours
    {
        return Ok(recorded);
    }
    let addr = local(DEFAULT_PORT);
    match check(addr).await {
        Local::Ours => return Ok(addr),
        Local::Stranger => return Err(StartError::Stranger(addr)),
        Local::Nothing => {}
    }
    host.setup(addr)
        .await
        .map_err(|e| StartError::Setup(format!("{e:#}")))?;
    let deadline = tokio::time::Instant::now() + START_WAIT;
    loop {
        match check(addr).await {
            Local::Ours => return Ok(addr),
            Local::Stranger => return Err(StartError::Stranger(addr)),
            Local::Nothing if tokio::time::Instant::now() >= deadline => {
                return Err(StartError::Silent(addr));
            }
            Local::Nothing => tokio::time::sleep(START_POLL).await,
        }
    }
}

/// The real machine: `valk` as a separate process, and the proof over loopback.
pub struct Valk {
    /// The `valk` to run.
    pub exe: PathBuf,
}

impl Valk {
    /// The `valk` the installer put beside this app, else the one where
    /// `install.sh` and `install.ps1` put it, else whatever `valk` the PATH has.
    /// An app started from a dock or a menu has a bare PATH, so that comes last.
    pub fn find() -> Self {
        let name = format!("valk{}", std::env::consts::EXE_SUFFIX);
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.join(&name)));
        let home = std::env::var_os(if cfg!(windows) {
            "LOCALAPPDATA"
        } else {
            "HOME"
        })
        .map(PathBuf::from)
        .map(|home| {
            if cfg!(windows) {
                home.join("Programs").join("Valkyrie").join(&name)
            } else {
                home.join(".local").join("bin").join(&name)
            }
        });
        let exe = [beside, home]
            .into_iter()
            .flatten()
            .find(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from(&name));
        Self { exe }
    }
}

impl Host for Valk {
    async fn check(&self, addr: SocketAddr) -> Result<Local> {
        valkyrie_web::proof::check(addr).await
    }

    async fn setup(&self, addr: SocketAddr) -> Result<()> {
        let out = tokio::process::Command::new(&self.exe)
            .args(["setup", "web", "--local", "--listen"])
            .arg(addr.to_string())
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .with_context(|| format!("run {}", self.exe.display()))?;
        if !out.status.success() {
            let said = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("{}", said.trim().trim_start_matches("Error: "));
        }
        Ok(())
    }

    fn recorded(&self) -> Option<SocketAddr> {
        valkyrie_web::listening()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    /// A machine in a given state: what answers where, and what a setup changes.
    #[derive(Default)]
    struct Fake {
        ports: RefCell<HashMap<u16, Local>>,
        recorded: Option<SocketAddr>,
        /// What the default port answers once a setup has run.
        after_setup: Option<Local>,
        setup_fails: bool,
        setups: Cell<u32>,
    }

    impl Fake {
        fn with(ports: &[(u16, Local)]) -> Self {
            Self {
                ports: RefCell::new(ports.iter().copied().collect()),
                ..Self::default()
            }
        }
    }

    impl Host for Fake {
        async fn check(&self, addr: SocketAddr) -> Result<Local> {
            assert!(addr.ip().is_loopback(), "asked beyond this machine: {addr}");
            Ok(*self
                .ports
                .borrow()
                .get(&addr.port())
                .unwrap_or(&Local::Nothing))
        }

        async fn setup(&self, addr: SocketAddr) -> Result<()> {
            self.setups.set(self.setups.get() + 1);
            if self.setup_fails {
                anyhow::bail!("no systemd user session");
            }
            if let Some(then) = self.after_setup {
                self.ports.borrow_mut().insert(addr.port(), then);
            }
            Ok(())
        }

        fn recorded(&self) -> Option<SocketAddr> {
            self.recorded
        }
    }

    #[tokio::test]
    async fn a_running_valk_web_is_used_as_it_is() {
        let host = Fake::with(&[(DEFAULT_PORT, Local::Ours)]);
        assert_eq!(ensure(&host).await.unwrap(), local(DEFAULT_PORT));
        assert_eq!(host.setups.get(), 0);
    }

    #[tokio::test]
    async fn one_on_another_port_is_found_by_what_it_recorded() {
        let mut host = Fake::with(&[(9001, Local::Ours)]);
        // It listens on every address: the window still loads 127.0.0.1.
        host.recorded = Some("0.0.0.0:9001".parse().unwrap());
        let addr = ensure(&host).await.unwrap();
        assert_eq!(url(addr), "http://127.0.0.1:9001");
        assert_eq!(host.setups.get(), 0);
    }

    #[tokio::test]
    async fn a_stale_record_falls_back_to_the_default_port() {
        let mut host = Fake::with(&[(DEFAULT_PORT, Local::Ours), (9001, Local::Stranger)]);
        host.recorded = Some("127.0.0.1:9001".parse().unwrap());
        assert_eq!(ensure(&host).await.unwrap(), local(DEFAULT_PORT));
    }

    #[tokio::test]
    async fn nothing_running_is_set_up_for_this_machine() {
        let mut host = Fake::with(&[]);
        host.after_setup = Some(Local::Ours);
        assert_eq!(ensure(&host).await.unwrap(), local(DEFAULT_PORT));
        assert_eq!(host.setups.get(), 1);
    }

    #[tokio::test]
    async fn a_stranger_is_never_loaded_or_set_up_over() {
        let host = Fake::with(&[(DEFAULT_PORT, Local::Stranger)]);
        let err = ensure(&host).await.unwrap_err();
        assert!(matches!(err, StartError::Stranger(_)), "{err}");
        assert_eq!(host.setups.get(), 0);

        // One that takes the port between the setup and the check.
        let mut host = Fake::with(&[]);
        host.after_setup = Some(Local::Stranger);
        let err = ensure(&host).await.unwrap_err();
        assert!(matches!(err, StartError::Stranger(_)), "{err}");
    }

    #[tokio::test]
    async fn a_failed_setup_says_why() {
        let mut host = Fake::with(&[]);
        host.setup_fails = true;
        let err = ensure(&host).await.unwrap_err();
        assert!(err.to_string().contains("no systemd user session"), "{err}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_service_that_never_answers_is_given_up_on() {
        let host = Fake::with(&[]);
        let err = ensure(&host).await.unwrap_err();
        assert!(matches!(err, StartError::Silent(_)), "{err}");
        assert_eq!(host.setups.get(), 1);
    }
}

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
/// How long `valk setup web --local` gets: it waits on the service manager, and
/// on the service for ten seconds of its own.
const SETUP_WAIT: Duration = Duration::from_secs(60);

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
    /// The check itself couldn't be made (the state directory can't be used,
    /// say): nothing is known about the port, so nothing is done to it.
    Check(String),
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
            Self::Check(why) => write!(f, "Couldn't check for valk web on this machine: {why}"),
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
/// A `valk web` that is already running is used where it is: the default port
/// first, then wherever one last said it listens (a service set up on another
/// port). Only when neither answers is one set up, on the default port. A
/// stranger on the port is never loaded and never set up over.
pub async fn ensure(host: &impl Host) -> Result<SocketAddr, StartError> {
    let check = |addr| async move {
        host.check(addr)
            .await
            .map_err(|e| StartError::Check(format!("{e:#}")))
    };
    // The record is only a hint, and only of a port: the window loads 127.0.0.1.
    let elsewhere = || {
        host.recorded()
            .map(|addr| local(addr.port()))
            .filter(|addr| addr.port() != DEFAULT_PORT && addr.port() != 0)
    };
    let addr = local(DEFAULT_PORT);
    let at_default = check(addr).await?;
    if at_default == Local::Ours {
        return Ok(addr);
    }
    if let Some(other) = elsewhere()
        && check(other).await? == Local::Ours
    {
        return Ok(other);
    }
    if at_default == Local::Stranger {
        return Err(StartError::Stranger(addr));
    }
    let setup = host.setup(addr).await;
    // A service that was already set up is started as it is, on whatever port
    // it was given: once it's up it says where, whatever the setup made of it.
    let deadline = tokio::time::Instant::now() + START_WAIT;
    loop {
        match check(addr).await? {
            Local::Ours => return Ok(addr),
            Local::Stranger => return Err(StartError::Stranger(addr)),
            Local::Nothing => {}
        }
        if let Some(other) = elsewhere()
            && check(other).await? == Local::Ours
        {
            return Ok(other);
        }
        if let Err(e) = &setup {
            return Err(StartError::Setup(format!("{e:#}")));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(StartError::Silent(addr));
        }
        tokio::time::sleep(START_POLL).await;
    }
}

/// The real machine: `valk` as a separate process, and the proof over loopback.
pub struct Valk {
    /// The `valk` to run.
    pub exe: PathBuf,
}

impl Valk {
    /// The `valk` beside this app, else the one where `install.sh` and
    /// `install.ps1` put a release, else cargo's, else whatever the PATH has.
    /// An app started from a dock or a menu has a bare PATH, so that comes last.
    pub fn find() -> Self {
        let name = crate::install::name();
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.join(&name)));
        // Cargo's last: `install.sh` from a checkout puts it there, where a
        // dock's PATH doesn't reach.
        let exe = [beside, crate::install::home(), crate::install::cargo_bin()]
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
        let mut valk = tokio::process::Command::new(&self.exe);
        valk.args(["setup", "web", "--local", "--listen"])
            .arg(addr.to_string())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        // No console window flashing up beside the app's own.
        #[cfg(windows)]
        valk.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let out = tokio::time::timeout(SETUP_WAIT, valk.output())
            .await
            .map_err(|_| anyhow::anyhow!("{} didn't finish", self.exe.display()))?
            .with_context(|| format!("run {}", self.exe.display()))?;
        if !out.status.success() {
            anyhow::bail!("{}", first_line(&String::from_utf8_lossy(&out.stderr)));
        }
        Ok(())
    }

    fn recorded(&self) -> Option<SocketAddr> {
        valkyrie_web::listening()
    }
}

/// What a failed `valk` said, without the rest: an older one that doesn't know
/// `--local` prints its whole usage after the line that matters.
fn first_line(stderr: &str) -> String {
    let line = stderr
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("it failed");
    let line = line.trim();
    line.strip_prefix("Error: ")
        .or_else(|| line.strip_prefix("error: "))
        .unwrap_or(line)
        .to_owned()
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
        /// A setup starts a service that was already there, on this port.
        setup_starts: Option<u16>,
        /// The check can't be made at all.
        check_fails: bool,
        checks: RefCell<Vec<u16>>,
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
            if self.check_fails {
                anyhow::bail!("refusing to use /state/web: must be a directory with mode 0700");
            }
            self.checks.borrow_mut().push(addr.port());
            Ok(*self
                .ports
                .borrow()
                .get(&addr.port())
                .unwrap_or(&Local::Nothing))
        }

        async fn setup(&self, addr: SocketAddr) -> Result<()> {
            self.setups.set(self.setups.get() + 1);
            if let Some(port) = self.setup_starts {
                self.ports.borrow_mut().insert(port, Local::Ours);
            }
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
    async fn the_default_port_comes_before_a_record() {
        // A `valk web` tried out on another port wrote the record last; the
        // service on the default port is still the one the window is paired with.
        let mut host = Fake::with(&[(DEFAULT_PORT, Local::Ours), (9001, Local::Ours)]);
        host.recorded = Some("127.0.0.1:9001".parse().unwrap());
        assert_eq!(ensure(&host).await.unwrap(), local(DEFAULT_PORT));
        assert_eq!(*host.checks.borrow(), [DEFAULT_PORT]);
    }

    #[tokio::test]
    async fn a_record_that_proves_nothing_is_no_way_round_a_stranger() {
        let mut host = Fake::with(&[(DEFAULT_PORT, Local::Stranger), (9001, Local::Stranger)]);
        host.recorded = Some("127.0.0.1:9001".parse().unwrap());
        let err = ensure(&host).await.unwrap_err();
        assert!(matches!(err, StartError::Stranger(_)), "{err}");
        assert_eq!(host.setups.get(), 0);
        // A record of the default port, or of no port, isn't asked again.
        for record in ["127.0.0.1:8790", "127.0.0.1:0"] {
            let mut host = Fake::with(&[(DEFAULT_PORT, Local::Stranger)]);
            host.recorded = Some(record.parse().unwrap());
            ensure(&host).await.unwrap_err();
            assert_eq!(*host.checks.borrow(), [DEFAULT_PORT]);
        }
    }

    /// The user's service is set up on another port and stopped. The setup
    /// starts it as it is, then fails: nothing answers on the port it was asked
    /// for. The service is up all the same, and said where.
    #[tokio::test]
    async fn a_stopped_service_on_another_port_is_found_once_started() {
        let mut host = Fake::with(&[]);
        host.recorded = Some("127.0.0.1:9001".parse().unwrap());
        host.setup_starts = Some(9001);
        host.setup_fails = true;
        assert_eq!(ensure(&host).await.unwrap(), local(9001));
        assert_eq!(host.setups.get(), 1);
    }

    #[tokio::test]
    async fn a_check_that_cant_be_made_says_so_and_touches_nothing() {
        let mut host = Fake::with(&[]);
        host.check_fails = true;
        let err = ensure(&host).await.unwrap_err();
        assert!(matches!(err, StartError::Check(_)), "{err}");
        assert!(err.to_string().contains("mode 0700"), "{err}");
        assert_eq!(host.setups.get(), 0);
    }

    #[test]
    fn a_failed_setup_is_told_in_one_line() {
        assert_eq!(
            first_line("Error: something already listens on 127.0.0.1:8790\n\nCaused by:\n  x"),
            "something already listens on 127.0.0.1:8790"
        );
        assert_eq!(
            first_line("error: unexpected argument '--local' found\n\nUsage: valk setup web"),
            "unexpected argument '--local' found"
        );
        assert_eq!(first_line(""), "it failed");
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

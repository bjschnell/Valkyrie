//! `valk setup web`: `valk web` as a background service the OS keeps running, so
//! the web app outlives the terminal that started it (DESIGN §8.10). A systemd user
//! unit on Linux, a launchd agent on macOS. Phones pair afterwards with
//! `valk web pair`, which the setup runs once at the end.

use anyhow::{Context, Result, bail};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use valkyrie_web::tailscale;

/// The service's name: the systemd unit, or the launchd label.
#[cfg(target_os = "linux")]
pub const DEFAULT_NAME: &str = "valk-web";
#[cfg(not(target_os = "linux"))]
pub const DEFAULT_NAME: &str = "dev.valkyrie.web";

/// The environment the service runs with, copied from the shell that sets it up.
/// Services start with a bare one, and the daemon (which `valk web` starts when none
/// is running, say at boot) passes it on to every session.
const KEPT_ENV: &[&str] = &["PATH", "SHELL", "LANG", "LC_ALL", "XDG_STATE_HOME"];

pub struct Setup {
    pub name: String,
    pub exe: PathBuf,
    pub socket: PathBuf,
    pub listen: SocketAddr,
    pub url: Option<String>,
    pub dry_run: bool,
    pub remove: bool,
}

impl Setup {
    /// `valk web`'s command line in the service.
    fn command(&self) -> Vec<String> {
        let mut args = vec![
            self.exe.display().to_string(),
            "--socket".into(),
            self.socket.display().to_string(),
            "web".into(),
            "--listen".into(),
            self.listen.to_string(),
        ];
        if let Some(url) = &self.url {
            args.extend(["--url".into(), url.clone()]);
        }
        args
    }

    fn env(&self) -> Vec<(String, String)> {
        KEPT_ENV
            .iter()
            .filter_map(|&k| Some((k.to_owned(), std::env::var(k).ok()?)))
            .collect()
    }
}

/// The valk to run: the one on PATH when it's this same binary, since that path
/// (often a symlink into a build) stays put while the build it points at changes.
pub fn default_exe() -> Result<PathBuf> {
    let me = std::env::current_exe()?;
    let real = std::fs::canonicalize(&me)?;
    let on_path = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("valk"))
            .find(|p| std::fs::canonicalize(p).is_ok_and(|p| p == real))
    });
    Ok(on_path.unwrap_or(me))
}

pub fn run(setup: Setup) -> Result<()> {
    let platform = Platform::here()?;
    let file = platform.file(&setup.name)?;
    if setup.remove {
        if setup.dry_run {
            println!("would stop {} and delete {}", setup.name, file.display());
            return Ok(());
        }
        platform.stop(&setup.name);
        match std::fs::remove_file(&file) {
            Ok(()) => println!("removed {}", file.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("{} isn't installed", setup.name)
            }
            Err(e) => return Err(e).context(format!("remove {}", file.display())),
        }
        platform.reload();
        return Ok(());
    }

    let text = platform.render(&setup);
    if setup.dry_run {
        println!("# {}\n{text}", file.display());
        return Ok(());
    }
    // A `valk web` left running in a terminal would hold the port, and the service
    // would fail to start over and over.
    if !platform.running(&setup.name) && std::net::TcpListener::bind(setup.listen).is_err() {
        bail!(
            "something already listens on {} (a `valk web` in a terminal?); stop it first",
            setup.listen
        );
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    std::fs::write(&file, text).with_context(|| format!("write {}", file.display()))?;
    platform.start(&setup.name, &file)?;
    println!("installed {}: {}", setup.name, file.display());
    if !wait_for(setup.listen, Duration::from_secs(10)) {
        bail!(
            "the service started but nothing answers on {}; {}",
            setup.listen,
            platform.logs(&setup.name)
        );
    }
    println!(
        "valk web runs on {}, and starts again at login",
        setup.listen
    );
    platform.notes();

    let url = valkyrie_web::public_url(setup.url.clone(), setup.listen);
    if url.starts_with("https://") && setup.listen.ip().is_loopback() {
        let port = setup.listen.port();
        if tailscale::serving(port) {
            println!("tailscale serve already forwards {url} to it");
        } else {
            match tailscale::serve(port) {
                Ok(()) => println!("tailscale serve now forwards {url} to it"),
                Err(e) => println!(
                    "couldn't run `tailscale serve --bg {port}`: {e}\n  \
                     on Linux, let your user configure Tailscale once with \
                     `sudo tailscale set --operator=$USER`, then run this again"
                ),
            }
        }
        if tailscale::tailnet().is_some_and(|t| !t.certs) {
            println!("{}", tailscale::CERTS_HELP);
        }
    }
    println!("logs: {}", platform.logs(&setup.name));
    valkyrie_web::print_pairing(&valkyrie_web::store()?, &url)
}

fn wait_for(addr: SocketAddr, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Runs a service manager command; its own message on failure.
fn manage(program: &str, args: &[&str]) -> Result<()> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("run {program}"))?;
    if !out.status.success() {
        bail!(
            "{program} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

fn succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

enum Platform {
    Systemd,
    Launchd,
}

impl Platform {
    fn here() -> Result<Self> {
        if cfg!(target_os = "linux") {
            Ok(Self::Systemd)
        } else if cfg!(target_os = "macos") {
            Ok(Self::Launchd)
        } else {
            bail!(
                "`valk setup web` knows systemd (Linux) and launchd (macOS); on this \
                 system, run `valk web` from your own service manager"
            )
        }
    }

    fn file(&self, name: &str) -> Result<PathBuf> {
        Ok(match self {
            Self::Systemd => std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home().join(".config"))
                .join("systemd/user")
                .join(format!("{name}.service")),
            Self::Launchd => home()
                .join("Library/LaunchAgents")
                .join(format!("{name}.plist")),
        })
    }

    fn render(&self, setup: &Setup) -> String {
        match self {
            Self::Systemd => systemd_unit(&setup.command(), &setup.env()),
            Self::Launchd => launchd_plist(
                &setup.name,
                &setup.command(),
                &setup.env(),
                &valkyrie_proto::state_dir().join("web.log"),
            ),
        }
    }

    fn running(&self, name: &str) -> bool {
        match self {
            Self::Systemd => succeeds("systemctl", &["--user", "is-active", "--quiet", name]),
            Self::Launchd => succeeds("launchctl", &["print", &launchd_target(name)]),
        }
    }

    /// Starts it, or restarts it on the new file when it already runs.
    fn start(&self, name: &str, file: &Path) -> Result<()> {
        match self {
            Self::Systemd => {
                manage("systemctl", &["--user", "daemon-reload"])?;
                manage("systemctl", &["--user", "enable", name])?;
                manage("systemctl", &["--user", "restart", name])
            }
            Self::Launchd => {
                self.stop(name);
                manage(
                    "launchctl",
                    &["bootstrap", &launchd_domain(), &file.display().to_string()],
                )
            }
        }
    }

    /// Stops it and keeps it from starting at login. Not running is fine.
    fn stop(&self, name: &str) {
        match self {
            Self::Systemd => {
                let _ = manage("systemctl", &["--user", "disable", "--now", name]);
            }
            Self::Launchd => {
                let _ = manage("launchctl", &["bootout", &launchd_target(name)]);
            }
        }
    }

    fn reload(&self) {
        if let Self::Systemd = self {
            let _ = manage("systemctl", &["--user", "daemon-reload"]);
        }
    }

    fn logs(&self, name: &str) -> String {
        match self {
            Self::Systemd => format!("journalctl --user -u {name} -f"),
            Self::Launchd => format!(
                "tail -f {}",
                valkyrie_proto::state_dir().join("web.log").display()
            ),
        }
    }

    /// What the user may still want to know.
    fn notes(&self) {
        if let Self::Systemd = self {
            let user = std::env::var("USER").unwrap_or_default();
            let linger = Command::new("loginctl")
                .args(["show-user", &user, "--property=Linger", "--value"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "yes")
                .unwrap_or(true);
            if !linger {
                println!(
                    "  it stops when you log out; to keep it running from boot: \
                     loginctl enable-linger"
                );
            }
        }
    }
}

fn launchd_domain() -> String {
    // SAFETY: getuid cannot fail.
    format!("gui/{}", unsafe { libc::getuid() })
}

fn launchd_target(name: &str) -> String {
    format!("{}/{name}", launchd_domain())
}

/// A systemd user unit. The daemon `valk web` may start belongs in the unit's
/// cgroup, so stopping or restarting the unit ends only the main process
/// (`KillMode=process`): never the daemon and its sessions.
fn systemd_unit(command: &[String], env: &[(String, String)]) -> String {
    let exec = command
        .iter()
        .map(|a| systemd_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    let env: String = env
        .iter()
        .map(|(k, v)| format!("Environment={}\n", systemd_quote(&format!("{k}={v}"))))
        .collect();
    format!(
        "# Made by `valk setup web`; `valk setup web --remove` takes it away.\n\
         [Unit]\n\
         Description=Valkyrie web app (valk web)\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         ExecStart={exec}\n\
         {env}\
         KillMode=process\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

/// One systemd word: quoted when it has to be, `%` and `\` escaped.
fn systemd_quote(word: &str) -> String {
    let escaped = word
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    if word.is_empty() || word.contains(|c: char| c.is_whitespace() || "\"'\\;$".contains(c)) {
        format!("\"{escaped}\"")
    } else {
        escaped
    }
}

/// A launchd agent: starts at login, comes back if it fails, and leaves the
/// daemon it may start alone when it exits (`AbandonProcessGroup`).
fn launchd_plist(name: &str, command: &[String], env: &[(String, String)], log: &Path) -> String {
    let args: String = command
        .iter()
        .map(|a| format!("    <string>{}</string>\n", xml(a)))
        .collect();
    let env: String = env
        .iter()
        .map(|(k, v)| {
            format!(
                "    <key>{}</key>\n    <string>{}</string>\n",
                xml(k),
                xml(v)
            )
        })
        .collect();
    let log = xml(&log.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Made by `valk setup web`; `valk setup web --remove` takes it away. -->
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{name}</string>
  <key>ProgramArguments</key>
  <array>
{args}  </array>
  <key>EnvironmentVariables</key>
  <dict>
{env}  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>5</integer>
  <key>AbandonProcessGroup</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
        name = xml(name)
    )
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Setup {
        Setup {
            name: "valk-web".into(),
            exe: "/home/u/.local/bin/valk".into(),
            socket: "/home/u/My State/run/h.sock".into(),
            listen: "127.0.0.1:8790".parse().unwrap(),
            url: None,
            dry_run: false,
            remove: false,
        }
    }

    #[test]
    fn writes_a_systemd_unit() {
        let env = [("PATH".to_owned(), "/usr/bin:/bin".to_owned())];
        let unit = systemd_unit(&setup().command(), &env);
        assert!(unit.contains(
            "ExecStart=/home/u/.local/bin/valk --socket \"/home/u/My State/run/h.sock\" web --listen 127.0.0.1:8790\n"
        ));
        assert!(unit.contains("Environment=PATH=/usr/bin:/bin\n"));
        assert!(unit.contains("KillMode=process\n"));
        assert!(unit.contains("WantedBy=default.target\n"));
    }

    #[test]
    fn quotes_systemd_words() {
        assert_eq!(systemd_quote("plain"), "plain");
        assert_eq!(systemd_quote("50%"), "50%%");
        assert_eq!(systemd_quote("a b"), "\"a b\"");
        assert_eq!(systemd_quote("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(systemd_quote(""), "\"\"");
    }

    #[test]
    fn writes_a_launchd_plist() {
        let mut s = setup();
        s.url = Some("https://a.ts.net/?x=1&y=2".into());
        let env = [("SHELL".to_owned(), "/bin/zsh".to_owned())];
        let plist = launchd_plist(
            "dev.valkyrie.web",
            &s.command(),
            &env,
            Path::new("/Users/u/.local/state/valkyrie/web.log"),
        );
        assert!(plist.contains("<string>dev.valkyrie.web</string>"));
        assert!(plist.contains("    <string>/home/u/My State/run/h.sock</string>\n"));
        assert!(plist.contains("<string>https://a.ts.net/?x=1&amp;y=2</string>"));
        assert!(plist.contains("    <key>SHELL</key>\n    <string>/bin/zsh</string>\n"));
        assert!(plist.contains("<key>AbandonProcessGroup</key>\n  <true/>"));
    }
}

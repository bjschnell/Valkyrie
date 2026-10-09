//! `valk setup web`: `valk web` as a background service the OS keeps running, so
//! the web app outlives the terminal that started it (DESIGN §8.10). A systemd user
//! unit on Linux, a launchd agent on macOS, a Task Scheduler task on Windows. Phones pair afterwards with
//! `valk web pair`, which the setup runs once at the end.

use anyhow::{Context, Result, bail};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use valkyrie_web::tailscale;

/// The service's name: the systemd unit, the launchd label, or the task.
#[cfg(target_os = "linux")]
pub const DEFAULT_NAME: &str = "valk-web";
#[cfg(target_os = "macos")]
pub const DEFAULT_NAME: &str = "dev.valkyrie.web";
#[cfg(windows)]
pub const DEFAULT_NAME: &str = "Valkyrie web";

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
    let real = valkyrie_proto::canonical(&me)?;
    let name = format!("valk{}", std::env::consts::EXE_SUFFIX);
    let on_path = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(&name))
            .find(|p| valkyrie_proto::canonical(p).is_ok_and(|p| p == real))
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

    let text = platform.render(&setup)?;
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
    std::fs::write(&file, platform.encode(&text))
        .with_context(|| format!("write {}", file.display()))?;
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
    valkyrie_proto::home_dir()
}

enum Platform {
    Systemd,
    Launchd,
    TaskScheduler,
}

impl Platform {
    fn here() -> Result<Self> {
        if cfg!(target_os = "linux") {
            Ok(Self::Systemd)
        } else if cfg!(target_os = "macos") {
            Ok(Self::Launchd)
        } else if cfg!(windows) {
            Ok(Self::TaskScheduler)
        } else {
            bail!(
                "`valk setup web` knows systemd (Linux), launchd (macOS) and Task \
                 Scheduler (Windows); on this system, run `valk web` from your own \
                 service manager"
            )
        }
    }

    /// The file's bytes: Task Scheduler reads its XML as UTF-16.
    fn encode(&self, text: &str) -> Vec<u8> {
        match self {
            Self::TaskScheduler => [0xfeffu16]
                .into_iter()
                .chain(text.encode_utf16())
                .flat_map(u16::to_le_bytes)
                .collect(),
            _ => text.as_bytes().to_vec(),
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
            // The task lives in Task Scheduler; this is the definition it came from.
            Self::TaskScheduler => valkyrie_proto::state_dir().join(format!("{name}.xml")),
        })
    }

    fn render(&self, setup: &Setup) -> Result<String> {
        let log = valkyrie_proto::state_dir().join("web.log");
        Ok(match self {
            Self::Systemd => systemd_unit(&setup.command(), &setup.env()),
            Self::Launchd => launchd_plist(&setup.name, &setup.command(), &setup.env(), &log),
            Self::TaskScheduler => task_xml(&setup.command(), &setup.env(), &log, &windows_user())?,
        })
    }

    fn running(&self, name: &str) -> bool {
        match self {
            Self::Systemd => succeeds("systemctl", &["--user", "is-active", "--quiet", name]),
            Self::Launchd => succeeds("launchctl", &["print", &launchd_target(name)]),
            Self::TaskScheduler => Command::new("schtasks")
                .args(["/Query", "/TN", name, "/FO", "CSV", "/NH"])
                .output()
                .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("Running")),
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
            Self::TaskScheduler => {
                let _ = manage("schtasks", &["/End", "/TN", name]);
                let file = file.display().to_string();
                manage("schtasks", &["/Create", "/TN", name, "/XML", &file, "/F"])?;
                manage("schtasks", &["/Run", "/TN", name])
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
            Self::TaskScheduler => {
                let _ = manage("schtasks", &["/End", "/TN", name]);
                let _ = manage("schtasks", &["/Delete", "/TN", name, "/F"]);
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
            Self::TaskScheduler => format!(
                "Get-Content -Wait '{}'",
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
    #[cfg(unix)]
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    #[cfg(windows)]
    let uid = 0;
    format!("gui/{uid}")
}

/// `DOMAIN\user`, whom the task's logon trigger is for.
fn windows_user() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{user}"),
        _ => user,
    }
}

/// A Task Scheduler task: starts at `user`'s logon and comes back if it fails.
/// A console program started by a task opens a window, so it runs in a headless
/// console host, through `cmd` for the log. Windows has no `KillMode=process`:
/// ending the task ends its job, and with it a daemon `valk web` started that
/// could not break away from it.
fn task_xml(
    command: &[String],
    env: &[(String, String)],
    log: &Path,
    user: &str,
) -> Result<String> {
    let words = command
        .iter()
        .map(|word| cmd_quote(word))
        .collect::<Result<Vec<_>>>()?;
    let log = cmd_quote(&log.display().to_string())?;
    let environment = env
        .iter()
        .map(|(key, value)| {
            anyhow::ensure!(
                KEPT_ENV.contains(&key.as_str()),
                "unsupported service environment variable {key}"
            );
            Ok(format!("set {} && ", cmd_quote(&format!("{key}={value}"))?))
        })
        .collect::<Result<String>>()?;
    let line = format!("{environment}{} >> {log} 2>&1", words.join(" "));
    let arguments = format!("--headless cmd.exe /d /v:off /s /c \"{line}\"");
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<!-- Made by `valk setup web`; `valk setup web --remove` takes it away. -->
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Valkyrie web app (valk web)</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>999</Count>
    </RestartOnFailure>
    <Hidden>true</Hidden>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>conhost.exe</Command>
      <Arguments>{arguments}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        user = xml(user),
        arguments = xml(&arguments),
    ))
}

/// One word for `cmd /s /c "…"`, in double quotes so `&`, `|` and the like are
/// plain text. Nothing in quotes escapes `"` or `%` from cmd, so those are refused.
fn cmd_quote(word: &str) -> Result<String> {
    if word.contains(['"', '%', '\r', '\n', '\0']) {
        bail!("can't pass {word:?} through cmd.exe; pick one without \" or %");
    }
    Ok(format!("\"{word}\""))
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
    fn writes_a_task_that_runs_headless_and_logs() {
        let mut s = setup();
        s.exe = r"C:\Users\u\AppData\Local\Programs\Valkyrie\valk.exe".into();
        s.url = Some("https://a.ts.net/?x=1&y=2".into());
        let log = Path::new(r"C:\Users\u\AppData\Local\Valkyrie\web.log");
        let task = task_xml(&s.command(), &[], log, r"PC\u").unwrap();
        assert!(task.contains("<UserId>PC\\u</UserId>"));
        assert!(task.contains(
            "<Arguments>--headless cmd.exe /d /v:off /s /c &quot;&quot;C:\\Users\\u\\AppData\\Local\\Programs\\Valkyrie\\valk.exe&quot; &quot;--socket&quot;"
        ));
        assert!(task.contains("&quot;https://a.ts.net/?x=1&amp;y=2&quot;"));
        assert!(task.contains(
            " &gt;&gt; &quot;C:\\Users\\u\\AppData\\Local\\Valkyrie\\web.log&quot; 2&gt;&amp;1&quot;</Arguments>"
        ));
        assert!(cmd_quote("50%").is_err());
        assert!(cmd_quote("line\nbreak").is_err());
        let env = [("PATH".into(), r"C:\Program Files\nodejs;C:\Windows".into())];
        let task = task_xml(&s.command(), &env, log, r"PC\u").unwrap();
        assert!(
            task.contains(r"set &quot;PATH=C:\Program Files\nodejs;C:\Windows&quot; &amp;&amp; ")
        );
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

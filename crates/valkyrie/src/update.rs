//! `valk upgrade` for a release build: fetch the release for this platform and put it
//! in place of this binary; `main::upgrade` then hands the daemon to it. Downloads go
//! through `curl` and archives through `tar`, which every platform a release ships for
//! carries (Windows since 10 1803), so valk links no TLS of its own.

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "bjschnell/Valkyrie";
/// The commit a release build was made from (release.yml sets it); a build from
/// source has none and never downloads a release over itself.
const COMMIT: Option<&str> = option_env!("VALK_COMMIT");
const TARGET: &str = env!("VALK_TARGET");
pub const VERSION: &str = env!("VALK_LONG_VERSION");

struct Release {
    tag: String,
    /// What the tag points at. A release is replaced by moving its tag, so the
    /// commit, not the name, says whether this binary is the release's.
    commit: String,
}

/// What `valk upgrade` does once the binary is settled.
pub enum Next {
    /// Hand the daemon to this binary (`None`: the one running).
    Handoff(Option<PathBuf>),
    /// This is the release already; the daemon may still run an older one.
    Current,
    /// Nothing more (a check, or the release's builds aren't up yet).
    Stop,
}

/// Brings this binary up to the release `tag` (default: the latest). With `check`,
/// only reports.
pub fn update(check: bool, tag: Option<&str>) -> Result<Next> {
    let Some(ours) = COMMIT else {
        let note = "this valk was built from source, so it updates with its checkout \
                    (git pull, then ./install.sh)";
        if check || tag.is_some() {
            bail!("{note}");
        }
        println!("{note}; switching the daemon to it as it is");
        return Ok(Next::Handoff(None));
    };
    let release = find(tag)?;
    let short = &release.commit[..7];
    if release.commit == ours {
        println!("valk {VERSION} is the {} release", release.tag);
        return Ok(Next::Current);
    }
    if check {
        println!(
            "{} ({short}) is out; this is valk {VERSION}. `valk upgrade` installs it",
            release.tag
        );
        return Ok(Next::Stop);
    }
    println!("downloading {} ({short}) for {TARGET}", release.tag);
    install(&release)
}

fn find(tag: Option<&str>) -> Result<Release> {
    let tag = match tag {
        Some(tag) => tag.to_owned(),
        None => {
            let json = curl(&[&format!(
                "https://api.github.com/repos/{REPO}/releases/latest"
            )])?;
            let info: serde_json::Value =
                serde_json::from_slice(&json).context("reading GitHub's release info")?;
            info["tag_name"]
                .as_str()
                .context("GitHub named no latest release")?
                .to_owned()
        }
    };
    ensure!(
        tag.starts_with('v')
            && tag[1..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b)),
        "{tag:?} isn't a release tag such as v0.0.2"
    );
    let commit = curl(&[
        "-H",
        "Accept: application/vnd.github.sha",
        &format!("https://api.github.com/repos/{REPO}/commits/{tag}"),
    ])?;
    let commit = String::from_utf8_lossy(&commit).trim().to_owned();
    ensure!(
        commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()),
        "GitHub gave no commit for {tag}"
    );
    Ok(Release { tag, commit })
}

/// Downloads and checks the release, then puts it where this binary is.
fn install(release: &Release) -> Result<Next> {
    let exe = valkyrie_proto::canonical(&std::env::current_exe()?)?;
    let dir = exe.parent().context("this binary has no directory")?;
    // Beside the binary, so moving it into place stays on one filesystem.
    let work = dir.join(format!(".valk-upgrade-{}", std::process::id()));
    std::fs::create_dir_all(&work)
        .with_context(|| format!("can't write beside {}", exe.display()))?;
    let result = fetch(release, &work).and_then(|new| {
        let version = version_of(&new)?;
        // The tag moves before the release's builds are uploaded: for a few minutes
        // it can still serve the build this already is.
        if version == VERSION {
            println!(
                "{}'s builds for {} aren't up yet, so this is still the newest; \
                 try again in a few minutes",
                release.tag,
                &release.commit[..7]
            );
            return Ok(Next::Stop);
        }
        let installed = place(&new, &exe, &release.tag)?;
        println!("installed valk {version} at {}", installed.display());
        Ok(Next::Handoff(Some(installed)))
    });
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn fetch(release: &Release, work: &Path) -> Result<PathBuf> {
    // Linux releases are static (musl) whatever this was built as.
    let target = TARGET.replace("-linux-gnu", "-linux-musl");
    let name = if cfg!(windows) {
        format!("valk-{target}.zip")
    } else {
        format!("valk-{target}.tar.gz")
    };
    let url = format!(
        "https://github.com/{REPO}/releases/download/{}/{name}",
        release.tag
    );
    let archive = work.join(&name);
    curl(&["-o", &archive.to_string_lossy(), &url])?;
    let sums = curl(&[&format!("{url}.sha256")])?;
    let want = String::from_utf8_lossy(&sums)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let digest = Sha256::digest(std::fs::read(&archive)?);
    let got: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    ensure!(
        got == want,
        "checksum mismatch for {name}; nothing was changed"
    );
    // GNU tar, bsdtar and Windows' tar.exe (bsdtar, which reads zip) all detect the
    // format themselves.
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(work)
        .status()
        .context("unpacking needs tar")?;
    ensure!(status.success(), "couldn't unpack {name}");
    let new = work.join(if cfg!(windows) { "valk.exe" } else { "valk" });
    ensure!(new.is_file(), "{name} has no valk in it");
    Ok(new)
}

/// What `new --version` says, after `valk `; also proves the download runs here.
fn version_of(new: &Path) -> Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(new, std::fs::Permissions::from_mode(0o755))?;
    }
    let out = Command::new(new).arg("--version").output()?;
    ensure!(out.status.success(), "the downloaded valk doesn't run here");
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.trim().trim_start_matches("valk ").to_owned())
}

/// A rename over the old binary: the daemon and any open TUI keep running the file
/// they already have open, and the next exec gets the new one.
#[cfg(unix)]
fn place(new: &Path, exe: &Path, _tag: &str) -> Result<PathBuf> {
    std::fs::rename(new, exe).with_context(|| format!("couldn't replace {}", exe.display()))?;
    Ok(exe.to_owned())
}

/// Makes `valk` run `installed` from now on, once the daemon took it. On Unix it
/// already does.
#[cfg(unix)]
pub fn activate(_installed: &Path) -> Result<()> {
    Ok(())
}

/// A running .exe can't be replaced, so install.ps1 gives each version its own
/// directory, `<dir>\versions\<tag>-<32 hex>\valk.exe`, and `<dir>\valk.cmd` runs the
/// current one. This does the same.
#[cfg(windows)]
fn place(new: &Path, exe: &Path, tag: &str) -> Result<PathBuf> {
    let versions = exe.parent().and_then(Path::parent);
    let dir = versions
        .filter(|v| {
            v.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("versions"))
        })
        .and_then(Path::parent)
        .filter(|dir| dir.join("valk.cmd").is_file())
        .context("this valk wasn't installed by install.ps1; run that again to update")?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // As long as install.ps1's GUID, so the launcher keeps its length for a cmd.exe
    // still running it: cmd reads a batch file again, by offset, after each command.
    let target = dir.join(format!(r"versions\{tag}-{nanos:032x}"));
    std::fs::create_dir_all(&target)?;
    let installed = target.join("valk.exe");
    std::fs::rename(new, &installed)?;
    Ok(installed)
}

/// Points `<dir>\valk.cmd` at `installed`. After the handoff, as install.ps1 does: if
/// it fails, the old launcher still runs the valk the daemon has.
#[cfg(windows)]
pub fn activate(installed: &Path) -> Result<()> {
    let version = installed.parent().context("no version directory")?;
    let dir = version
        .parent()
        .and_then(Path::parent)
        .context("no install directory")?;
    // Same length as the old one only when the tag's is (--tag can change it); a
    // shorter or longer line leaves a running cmd.exe at a stale offset.
    let relative = format!(
        r"versions\{}",
        version.file_name().unwrap_or_default().to_string_lossy()
    );
    let launcher = format!(
        "@echo off\r\nsetlocal DisableDelayedExpansion\r\n\"%~dp0{relative}\\valk.exe\" %*\r\nexit /b %errorlevel%\r\n"
    );
    let tmp = dir.join("valk.cmd.new");
    std::fs::write(&tmp, launcher)?;
    std::fs::rename(&tmp, dir.join("valk.cmd"))?;
    Ok(())
}

fn curl(args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "--proto", "=https"])
        .args(args)
        .output()
        .context("valk upgrade downloads with curl; install curl, or rerun the install script")?;
    if !out.status.success() {
        bail!(
            "couldn't fetch {}: {}",
            args.last().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

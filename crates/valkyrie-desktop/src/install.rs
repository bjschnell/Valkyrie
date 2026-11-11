//! `valk` for someone who installed only the desktop app (ADR-0020, phase 4).
//!
//! The app's installers carry the `valk` of their release. On its first launch
//! the app puts that binary where `install.sh` and `install.ps1` would have, and
//! only if no `valk` is installed yet. From then on it is the user's install:
//! `valk upgrade` and `valk`'s own update path own it, and the app's updater
//! never touches it, so the two never fight over the binary the daemon runs.
//! Only the binary is placed: the scripts' extras (a PATH entry and a Windows
//! Terminal profile on Windows) are for someone who works in a terminal.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// `valk`, as this system names it.
pub fn name() -> String {
    format!("valk{}", std::env::consts::EXE_SUFFIX)
}

/// Where the installers put `valk`: `~/.local/bin` (`install.sh`), or
/// `%LOCALAPPDATA%\Programs\Valkyrie` (`install.ps1`).
pub fn home() -> Option<PathBuf> {
    if cfg!(windows) {
        let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
        Some(local.join("Programs").join("Valkyrie").join(name()))
    } else {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(home.join(".local").join("bin").join(name()))
    }
}

/// The `valk` on the PATH, if there is one.
pub fn on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name()))
        .find(|file| file.is_file())
}

/// Where `cargo install` puts `valk`, which is what `install.sh` does from a
/// checkout: `$CARGO_HOME/bin`, by default `~/.cargo/bin`. An app started from a
/// dock doesn't have it on its PATH.
pub fn cargo_bin() -> Option<PathBuf> {
    let cargo = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
            Some(PathBuf::from(std::env::var_os(home)?).join(".cargo"))
        })?;
    Some(cargo.join("bin").join(name()))
}

/// A `valk` installed somewhere other than the installers' place: on the PATH,
/// or cargo's. A second one beside it would be a second build for the daemon
/// and the terminal to disagree over.
pub fn elsewhere() -> Option<PathBuf> {
    on_path().or_else(|| cargo_bin().filter(|file| file.is_file()))
}

/// Puts the `bundled` valk at `home` when no valk is installed: none there, and
/// none `elsewhere` (the PATH's). Says whether it did. A valk that is already
/// installed is never replaced, however old: updating it is its own business.
pub fn place(bundled: &Path, home: &Path, elsewhere: Option<&Path>) -> Result<bool> {
    // Anything by that name counts, a link whose target is gone too: a
    // developer's link into a build tree is theirs to mend, not to write over.
    if home.symlink_metadata().is_ok() || elsewhere.is_some() || !bundled.is_file() {
        return Ok(false);
    }
    let dir = home.parent().context("the install directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    // Whole or not at all: a launch that dies mid-copy leaves no half a binary
    // under the name the next launch would run.
    let partial = dir.join(format!(".{}.new", name()));
    std::fs::copy(bundled, &partial)
        .with_context(|| format!("copy valk to {}", partial.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))?;
    }
    // Copied out of an app that was downloaded, the file carries the mark macOS
    // puts on downloads, and would be refused when run. The app it came in is
    // the one the user allowed.
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("/usr/bin/xattr")
        .args(["-d", "com.apple.quarantine"])
        .arg(&partial)
        .stderr(std::process::Stdio::null())
        .status();
    std::fs::rename(&partial, home).with_context(|| format!("install {}", home.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("valk-install-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_machine_with_no_valk_gets_the_bundled_one() {
        let dir = scratch("fresh");
        let bundled = dir.join("bundled");
        std::fs::write(&bundled, "the release's valk").unwrap();
        let home = dir.join("home").join(".local").join("bin").join(name());
        assert!(place(&bundled, &home, None).unwrap());
        assert_eq!(
            std::fs::read_to_string(&home).unwrap(),
            "the release's valk"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&home).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        // Nothing left beside it.
        assert_eq!(
            std::fs::read_dir(home.parent().unwrap()).unwrap().count(),
            1
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_valk_that_is_installed_is_left_alone() {
        let dir = scratch("kept");
        let bundled = dir.join("bundled");
        std::fs::write(&bundled, "newer").unwrap();
        let home = dir.join("bin").join(name());
        std::fs::create_dir_all(home.parent().unwrap()).unwrap();
        std::fs::write(&home, "older, and the user's").unwrap();
        assert!(!place(&bundled, &home, None).unwrap());
        assert_eq!(
            std::fs::read_to_string(&home).unwrap(),
            "older, and the user's"
        );
        // One on the PATH counts too: nothing is put beside it.
        let other = dir.join("other").join(name());
        assert!(!place(&bundled, &other, Some(&home)).unwrap());
        assert!(!other.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_link_whose_target_is_gone_is_still_someones_valk() {
        let dir = scratch("link");
        let bundled = dir.join("bundled");
        std::fs::write(&bundled, "the release's valk").unwrap();
        let home = dir.join("bin").join(name());
        std::fs::create_dir_all(home.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(dir.join("build").join("valk"), &home).unwrap();
        assert!(!place(&bundled, &home, None).unwrap());
        assert!(home.symlink_metadata().unwrap().file_type().is_symlink());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_app_run_from_source_carries_no_valk_and_places_none() {
        let dir = scratch("unbundled");
        let home = dir.join("bin").join(name());
        assert!(!place(&dir.join("resources").join(name()), &home, None).unwrap());
        assert!(!home.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

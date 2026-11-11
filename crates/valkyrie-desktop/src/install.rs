//! `valk` for someone who installed only the desktop app (ADR-0020, phase 4).
//!
//! The app's installers carry the `valk` of their release. On its first launch
//! the app puts that binary where `install.sh` and `install.ps1` would have, and
//! only if no `valk` is installed yet. From then on it is an ordinary install:
//! `valk upgrade` and `valk`'s own update path own it, and the app's updater
//! never touches it, so the two never fight over the binary the daemon runs.

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

/// Puts the `bundled` valk at `home` when no valk is installed: none there, and
/// none `elsewhere` (the PATH's). Says whether it did. A valk that is already
/// installed is never replaced, however old: updating it is its own business.
pub fn place(bundled: &Path, home: &Path, elsewhere: Option<&Path>) -> Result<bool> {
    if home.exists() || elsewhere.is_some() || !bundled.is_file() {
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

    #[test]
    fn an_app_run_from_source_carries_no_valk_and_places_none() {
        let dir = scratch("unbundled");
        let home = dir.join("bin").join(name());
        assert!(!place(&dir.join("resources").join(name()), &home, None).unwrap());
        assert!(!home.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

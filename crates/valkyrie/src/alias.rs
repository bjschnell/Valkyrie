//! `valk setup alias`: a `v` symlink beside the `valk` on `$PATH`, so either name
//! runs Valkyrie. Only made when no other `v` is on `$PATH`, so it never shadows
//! another tool (or gets shadowed by one).

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use valkyrie_proto::canonical;

pub const NAME: &str = "v";

/// What `valk setup alias` would do, by `$PATH`'s directories in order.
#[derive(Debug, PartialEq)]
enum Plan {
    /// Make `v` here, linked to the `valk` beside it.
    Link(PathBuf),
    /// This `v` already runs this valk.
    Done(PathBuf),
}

fn plan(dirs: &[PathBuf], me: &Path) -> Result<Plan> {
    let me = canonical(me)?;
    let runs_me = |p: &Path| canonical(p).is_ok_and(|p| p == me);
    let valk = dirs
        .iter()
        .map(|dir| dir.join("valk"))
        .find(|p| runs_me(p))
        .context("this valk isn't on your PATH as `valk`, so a `v` beside it wouldn't be either")?;
    for dir in dirs {
        let v = dir.join(NAME);
        if runs_me(&v) {
            return Ok(Plan::Done(v));
        }
        // Anything by that name, even a dangling link, is someone else's.
        if v.symlink_metadata().is_ok() {
            bail!(
                "{} is already on your PATH as {}; leaving it alone (valk still works)",
                NAME,
                v.display()
            );
        }
    }
    Ok(Plan::Link(valk.with_file_name(NAME)))
}

/// The `v` on `$PATH` that's a link to this valk, the only kind `--remove` deletes.
fn ours(dirs: &[PathBuf], me: &Path) -> Option<PathBuf> {
    let me = canonical(me).ok()?;
    dirs.iter().map(|dir| dir.join(NAME)).find(|v| {
        v.symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
            && canonical(v).is_ok_and(|p| p == me)
    })
}

pub fn run(remove: bool, dry_run: bool) -> Result<()> {
    if cfg!(windows) {
        bail!("`valk setup alias` is for Linux, macOS and WSL; on Windows, keep typing valk");
    }
    let me = std::env::current_exe()?;
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if remove {
        let Some(v) = ours(&dirs, &me) else {
            println!("no {NAME} alias for this valk on your PATH");
            return Ok(());
        };
        if dry_run {
            println!("would delete {}", v.display());
        } else {
            std::fs::remove_file(&v).with_context(|| format!("couldn't delete {}", v.display()))?;
            println!("removed {}", v.display());
        }
        return Ok(());
    }
    match plan(&dirs, &me)? {
        Plan::Done(v) => println!("{} already runs valk", v.display()),
        Plan::Link(v) if dry_run => println!("would link {} to valk", v.display()),
        Plan::Link(v) => {
            link(&v)?;
            println!(
                "linked {} to valk: type {NAME} for valk from now on",
                v.display()
            );
        }
    }
    Ok(())
}

/// Relative, so `v` follows whatever `valk` becomes (an upgrade, a rebuild).
#[cfg(unix)]
fn link(v: &Path) -> Result<()> {
    std::os::unix::fs::symlink("valk", v)
        .with_context(|| format!("couldn't create {}", v.display()))
}

#[cfg(windows)]
fn link(_v: &Path) -> Result<()> {
    unreachable!("run refuses on Windows")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// `bin` holds this valk, `other` comes before it on `$PATH`.
    fn setup(tag: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("valk-alias-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (other, bin) = (root.join("other"), root.join("bin"));
        std::fs::create_dir_all(&other).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let real = root.join("build-valk");
        std::fs::write(&real, "").unwrap();
        symlink(&real, bin.join("valk")).unwrap();
        (root, other, bin, real)
    }

    #[test]
    fn links_v_beside_valk_when_no_v_is_on_path() {
        let (root, other, bin, me) = setup("free");
        let dirs = [other, bin.clone()];
        assert_eq!(plan(&dirs, &me).unwrap(), Plan::Link(bin.join("v")));
        link(&bin.join("v")).unwrap();
        assert_eq!(plan(&dirs, &me).unwrap(), Plan::Done(bin.join("v")));
        assert_eq!(ours(&dirs, &me), Some(bin.join("v")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn leaves_another_v_alone_wherever_it_is_on_path() {
        let (root, other, bin, me) = setup("taken");
        std::fs::write(other.join("v"), "").unwrap();
        // Before valk's directory it would shadow the alias; after, the alias would
        // shadow it. Either way it isn't ours.
        for dirs in [[other.clone(), bin.clone()], [bin.clone(), other.clone()]] {
            let err = plan(&dirs, &me).unwrap_err().to_string();
            assert!(
                err.contains(&other.join("v").display().to_string()),
                "{err}"
            );
            assert_eq!(ours(&dirs, &me), None);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn needs_this_valk_on_path() {
        let (root, other, _bin, me) = setup("offpath");
        assert!(plan(&[other], &me).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

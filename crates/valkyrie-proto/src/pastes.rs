//! Files handed to an agent by path: an image pasted in the TUI, a file sent from
//! the web app. They go under the state dir's `pastes/`, private, and are kept a
//! week, well past the turn that reads them.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a pasted file stays.
const KEEP: Duration = Duration::from_secs(7 * 24 * 3600);
/// The most of a file's own name kept in the saved one.
const NAME_MAX: usize = 80;

pub fn dir() -> PathBuf {
    crate::state_dir().join("pastes")
}

/// Saves `bytes` in `dir` as `<ms>-<name>`, or `<ms>.<ext>` without a name, and
/// returns its path. The name keeps what the agent should see ("report.pdf"), made
/// safe to type: no spaces, quotes or separators.
pub fn save(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    // Screenshots and documents can hold anything: only you read them.
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    prune(dir);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let name = safe_name(name);
    // Two files in the same millisecond take the next free number.
    for n in 0..100 {
        let stem = match n {
            0 => ms.to_string(),
            n => format!("{ms}.{n}"),
        };
        let path = match name.starts_with('.') {
            true => dir.join(format!("{stem}{name}")),
            false => dir.join(format!("{stem}-{name}")),
        };
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&path) {
            Ok(mut file) => {
                file.write_all(bytes)?;
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free name"))
}

/// `name` with only letters, digits, `.`, `-` and `_`, the rest as `_`, and its
/// extension kept when it's cut short. A name of only an extension (`.png`) stays
/// one, so the saved file is `<ms>.png`.
pub fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut safe: String = base
        .chars()
        .map(
            |c| match c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                true => c,
                false => '_',
            },
        )
        .collect();
    if safe.chars().count() > NAME_MAX {
        let ext = safe
            .rfind('.')
            .map(|i| safe[i..].to_string())
            .filter(|e| e.chars().count() <= 12)
            .unwrap_or_default();
        let keep = NAME_MAX - ext.chars().count();
        safe = safe.chars().take(keep).collect::<String>() + &ext;
    }
    if safe.trim_matches('.').is_empty() {
        return "file".into();
    }
    safe
}

/// Removes files pasted more than `KEEP` ago.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > KEEP));
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_to_type() {
        assert_eq!(safe_name("report.pdf"), "report.pdf");
        assert_eq!(safe_name("My Shot (1).png"), "My_Shot__1_.png");
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("C:\\Users\\a\\x.txt"), "x.txt");
        assert_eq!(safe_name(".png"), ".png");
        assert_eq!(safe_name(""), "file");
        assert_eq!(safe_name(".."), "file");
        let long = format!("{}.tar.gz", "a".repeat(200));
        let cut = safe_name(&long);
        assert_eq!(cut.len(), NAME_MAX);
        assert!(cut.ends_with(".gz"));
    }

    #[test]
    fn saves_privately_without_clobbering() {
        let dir = std::env::temp_dir().join(format!("valkyrie-test-pastes-{}", std::process::id()));
        let a = save(&dir, "a b.txt", b"one").unwrap();
        let b = save(&dir, "a b.txt", b"two").unwrap();
        let png = save(&dir, ".png", b"img").unwrap();
        assert_ne!(a, b);
        assert!(
            a.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with("-a_b.txt")
        );
        assert!(!png.file_name().unwrap().to_str().unwrap().contains('-'));
        assert_eq!(std::fs::read(&b).unwrap(), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&a).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

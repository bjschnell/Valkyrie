//! Which Valkyrie features get used (backlog #176), so `valk stats` can show
//! what helps and what nobody touches. Off unless the `analytics` setting is on,
//! and kept only on this machine: one line per use in `<state dir>/usage.jsonl`,
//! naming the feature and who used it, never its arguments.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// How long a use counts.
pub const USAGE_DAYS: u64 = 90;
/// Size past which the log drops what no longer counts.
const COMPACT: u64 = 1 << 20;

/// Who used a feature.
pub const YOU: &str = "you";

/// One use of one feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Use {
    /// Seconds since the epoch.
    pub at: u64,
    /// `cli`, `mcp`, `tui` or `web`.
    pub surface: String,
    /// The subcommand (`todo add`), MCP tool (`search_decisions`) or client
    /// message (`launch_item`).
    pub feature: String,
    /// [`YOU`], or the agent that ran it (`claude`, `codex`).
    pub by: String,
    /// The project it was used in, when there was one.
    pub project: Option<PathBuf>,
}

impl Use {
    pub fn by_agent(&self) -> bool {
        self.by != YOU
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "at": self.at,
            "surface": self.surface,
            "feature": self.feature,
            "by": self.by,
        });
        if let Some(project) = &self.project {
            v["project"] = project.to_string_lossy().into();
        }
        v
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Use> {
        Some(Use {
            at: v["at"].as_u64()?,
            surface: v["surface"].as_str()?.to_owned(),
            feature: v["feature"].as_str()?.to_owned(),
            by: v["by"].as_str().unwrap_or(YOU).to_owned(),
            project: v["project"].as_str().map(PathBuf::from),
        })
    }
}

/// Whether uses are recorded: `VALK_ANALYTICS`, else `analytics` in
/// `settings.toml`; off when neither says on. Read each time, so a change in the
/// settings panel holds at once; it's one small file.
pub fn enabled() -> bool {
    if let Ok(v) = std::env::var("VALK_ANALYTICS") {
        return on(&v);
    }
    let path = valkyrie_proto::config_dir().join("settings.toml");
    enabled_in(&std::fs::read_to_string(path).unwrap_or_default())
}

/// What a `settings.toml`'s text says; the last `analytics` line wins.
pub fn enabled_in(text: &str) -> bool {
    text.lines()
        .filter_map(|l| l.split('#').next()?.split_once('='))
        .rfind(|(k, _)| k.trim() == "analytics")
        .is_some_and(|(_, v)| on(v))
}

fn on(v: &str) -> bool {
    matches!(v.trim().trim_matches('"').trim_matches('\''), "true" | "on")
}

/// `<state dir>/usage.jsonl`.
pub fn default_file() -> PathBuf {
    valkyrie_proto::state_dir().join("usage.jsonl")
}

/// Appends `u` to `path`, if [`enabled`]. Never worth an error to the caller:
/// a failed write is dropped.
pub fn record(path: &Path, u: &Use) {
    if enabled() {
        let _ = append(path, u, u.at);
    }
}

/// Appends `u` whatever the setting says. Past [`COMPACT`] bytes, what's older
/// than [`USAGE_DAYS`] is dropped first. One line is one small write, so two
/// writers' lines don't interleave.
pub fn append(path: &Path, u: &Use, now: u64) -> Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        valkyrie_proto::ensure_private_dir(dir)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > COMPACT) {
        let kept: String = read(path, now)
            .iter()
            .map(|u| format!("{}\n", u.to_json()))
            .collect();
        crate::write_atomic(path, &kept)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(format!("{}\n", u.to_json()).as_bytes())?;
    Ok(())
}

/// The uses recorded in `path` over the last [`USAGE_DAYS`] days, oldest first.
pub fn read(path: &Path, now: u64) -> Vec<Use> {
    let mut uses: Vec<Use> = std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| Use::from_json(&v))
        .filter(|u| now.saturating_sub(u.at) <= USAGE_DAYS * 86_400)
        .collect();
    uses.sort_by_key(|u| u.at);
    uses
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_use(at: u64, feature: &str, by: &str) -> Use {
        Use {
            at,
            surface: "cli".into(),
            feature: feature.into(),
            by: by.into(),
            project: Some("/repo".into()),
        }
    }

    #[test]
    fn analytics_is_off_unless_turned_on() {
        assert!(!enabled_in(""));
        assert!(!enabled_in("context = off\n"));
        assert!(!enabled_in("analytics = maybe\n"));
        assert!(enabled_in("analytics = on\n"));
        assert!(enabled_in("analytics = \"true\"  # count features\n"));
        assert!(!enabled_in("# analytics = on\n"));
        assert!(!enabled_in("analytics = on\nanalytics = off\n"));
    }

    #[test]
    fn uses_round_trip_and_old_ones_drop() {
        let dir = std::env::temp_dir().join(format!("valk-usage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("usage.jsonl");
        let now = 100 * 86_400;
        append(&path, &a_use(now - 91 * 86_400, "pings", YOU), now).unwrap();
        append(&path, &a_use(now - 5, "todo add", "claude"), now).unwrap();
        append(&path, &a_use(now - 9, "stats", YOU), now).unwrap();
        // A half-written or foreign line is skipped, not fatal.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, b"{\"at\":\n"))
            .unwrap();
        let uses = read(&path, now);
        assert_eq!(
            uses.iter().map(|u| u.feature.as_str()).collect::<Vec<_>>(),
            ["stats", "todo add"]
        );
        assert!(uses[1].by_agent());
        assert_eq!(uses[1].project.as_deref(), Some(Path::new("/repo")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

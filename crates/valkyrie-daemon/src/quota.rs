//! The plan's quota (protocol 30): the latest reading `valk statusline` sent,
//! pushed to queue watchers and kept on disk, so a restarted daemon still has one
//! before a Claude session next draws its status line.

use crate::Registry;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use valkyrie_proto::Quota;

/// The reading pushed to queue watchers.
pub(crate) type Held = Option<Arc<Quota>>;

/// A reading that says the same as the one held pushes again only this much later,
/// for its age: a status line redraws every few seconds.
const SAME_AGAIN_SECS: u64 = 60;
/// Racing status lines are seconds apart; a held reading further ahead than this
/// came from a clock that has since moved back.
const SKEW_SECS: u64 = 300;

pub(crate) fn file(state_dir: &Path) -> PathBuf {
    state_dir.join("quota.json")
}

pub(crate) fn load(path: &Path) -> Held {
    let text = std::fs::read(path).ok()?;
    serde_json::from_slice(&text).ok().map(Arc::new)
}

/// Whether `new` replaces `held`: newer, and saying something else or a minute on.
fn replaces(held: Option<&Quota>, new: &Quota) -> bool {
    match held {
        None => true,
        // Far ahead of any new one, the clock moved back: it would hold forever.
        Some(held) if held.at > new.at + SKEW_SECS => true,
        Some(held) => {
            new.at >= held.at && (new.limits != held.limits || new.at >= held.at + SAME_AGAIN_SECS)
        }
    }
}

impl Registry {
    pub(crate) fn set_quota(&self, quota: Quota) {
        let mut wrote = None;
        self.quota.send_if_modified(|held| {
            if !replaces(held.as_deref(), &quota) {
                return false;
            }
            let quota = Arc::new(quota);
            wrote = Some(quota.clone());
            *held = Some(quota);
            true
        });
        if let Some(quota) = wrote
            && let Err(e) = save(&self.quota_file, &quota)
        {
            tracing::debug!("quota not saved: {e:#}");
        }
    }
}

fn save(path: &Path, quota: &Quota) -> anyhow::Result<()> {
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(quota)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use valkyrie_proto::QuotaLimit;

    fn reading(at: u64, used: u8) -> Quota {
        Quota {
            at,
            limits: vec![QuotaLimit {
                name: "five_hour".into(),
                used,
                resets_at: Some(9),
            }],
        }
    }

    #[test]
    fn a_reading_replaces_when_it_says_more_or_a_minute_on() {
        let held = reading(1000, 20);
        assert!(replaces(None, &held));
        assert!(replaces(Some(&held), &reading(1001, 21)));
        assert!(!replaces(Some(&held), &reading(1030, 20)));
        assert!(replaces(Some(&held), &reading(1060, 20)));
        // Status lines race: an older one never wins.
        assert!(!replaces(Some(&held), &reading(999, 50)));
        // But not one from a clock that moved back.
        assert!(replaces(Some(&held), &reading(600, 20)));
    }

    #[test]
    fn a_saved_reading_loads_back() {
        let dir = std::env::temp_dir().join(format!("valk-quota-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = file(&dir);
        assert!(load(&path).is_none());
        save(&path, &reading(5, 42)).unwrap();
        assert_eq!(load(&path).as_deref(), Some(&reading(5, 42)));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

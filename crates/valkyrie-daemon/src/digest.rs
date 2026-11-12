//! The away digest (backlog #105): `ClientMsg::Digest` reads the sessions' event
//! logs (`valkyrie_context::digest`) and joins them with the backlog and the live
//! sessions. Also keeps `Presence`: when you last typed, so the digest starts
//! where you left.

use crate::Registry;
use crate::session::now_ms;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use valkyrie_context::digest::{self, Known, Presence};
use valkyrie_context::project;
use valkyrie_proto::{Digest, DigestGroup, Reply};

pub(crate) fn file(state_dir: &Path) -> PathBuf {
    state_dir.join("presence.json")
}

pub(crate) fn load(path: &Path) -> Presence {
    std::fs::read(path)
        .ok()
        .and_then(|t| serde_json::from_slice(&t).ok())
        .unwrap_or_default()
}

fn save(path: &Path, presence: &Presence) -> anyhow::Result<()> {
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(presence)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

impl Registry {
    /// You typed into a session (not an agent driving one).
    pub(crate) fn you_typed(&self) {
        let mut presence = self.presence.lock().unwrap();
        if presence.typed(now_ms())
            && let Err(e) = save(&self.presence_file, &presence)
        {
            tracing::debug!("presence not saved: {e:#}");
        }
    }

    /// What every agent did since `since_ms`, or since you were last gone, or in
    /// the last day when that's not known.
    pub(crate) async fn digest(
        &self,
        since_ms: Option<u64>,
        by: DigestGroup,
    ) -> anyhow::Result<Reply> {
        let now = now_ms();
        let left = self.presence.lock().unwrap().since(now);
        let (since, away) = match (since_ms, left) {
            (Some(since), _) => (since.min(now), false),
            (None, Some(left)) => (left, true),
            (None, None) => (now.saturating_sub(digest::FALLBACK_MS), false),
        };
        // Every log since the epoch would be a long read for one answer.
        let since = since.max(now.saturating_sub(digest::LONGEST_MS));
        let items: Vec<_> = self.items.borrow().iter().cloned().collect();
        let sessions = self.all();
        // Exited sessions stay listed until closed, but aren't running.
        let live: BTreeMap<String, u32> = sessions
            .iter()
            .filter(|s| !s.exited())
            .map(|s| (s.key(), s.id))
            .collect();
        let dir = self.host.transcript_dir.clone();
        let mut digest = tokio::task::spawn_blocking(move || {
            let mut roots: Vec<PathBuf> = items
                .iter()
                .filter_map(|i| i.project.clone())
                .chain(
                    sessions
                        .iter()
                        .map(|s| s.info().cwd)
                        .filter(|c| c.exists())
                        .map(|c| project::root(&c)),
                )
                .collect();
            roots.sort();
            roots.dedup();
            let runs = read_runs(&dir, since, now);
            let known = Known {
                items: &items,
                live: &live,
                roots: &roots,
            };
            Digest {
                since_ms: since,
                until_ms: now,
                away,
                by,
                entries: digest::entries(&runs, &known, now),
                markdown: String::new(),
            }
        })
        .await?;
        digest.markdown = digest::render(&digest, &jiff::tz::TimeZone::system());
        Ok(Reply::Digest {
            digest: Box::new(digest),
        })
    }
}

/// The runs of every log written to since `since` that started by `until`.
fn read_runs(dir: &Path, since: u64, until: u64) -> Vec<digest::Run> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut runs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".events.jsonl"))
        else {
            continue;
        };
        // `<created ns>-<id>`: one started after the window has nothing in it.
        let started = stem
            .split_once('-')
            .and_then(|(ns, _)| ns.parse::<u128>().ok())
            .map(|ns| (ns / 1_000_000) as u64);
        if started.is_some_and(|ms| ms > until) {
            continue;
        }
        let written = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64);
        if written.is_some_and(|ms| ms < since) {
            continue;
        }
        match digest::read_run(&path, since, until) {
            Ok(run) => runs.push(run),
            Err(e) => tracing::debug!("digest: {e:#}"),
        }
    }
    runs
}

#[cfg(all(test, unix))]
mod tests {
    use serde_json::json;
    use std::path::Path;
    use valkyrie_proto::{DigestGroup, Outcome, Reply};

    /// The digest reads the logs on disk, places a live session by its key, and
    /// starts where you left once your typing says you were gone.
    #[tokio::test]
    async fn the_digest_reads_the_logs_and_starts_where_you_left() {
        let dir = std::env::temp_dir().join(format!("valkyrie-digest-d-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = dir.join("state");
        std::fs::create_dir_all(state.join("sessions")).unwrap();
        let (registry, _) = crate::new_registry(
            dir.join("v.sock"),
            &state,
            Path::new("/bin/true"),
            1,
            0,
            7,
            false,
        )
        .unwrap();
        let now = crate::session::now_ms();
        let line = |v: serde_json::Value| format!("{v}\n");
        let log: String = [
            json!({"k": "start", "t": now - 60_000, "off": 0, "agent": "claude", "command": ["claude"]}),
            json!({"k": "hook", "t": now - 50_000, "off": 0, "agent": "claude", "ignored": false,
                "payload": {"cwd": dir.display().to_string(), "hook_event_name": "UserPromptSubmit",
                    "prompt": "tidy the readme"}}),
            json!({"k": "work", "t": now - 40_000, "off": 0, "state": "clean", "branch": "b",
                "pr": "https://github.com/o/r/pull/3"}),
        ]
        .into_iter()
        .map(line)
        .collect();
        std::fs::write(
            state.join(format!(
                "sessions/{}-1.events.jsonl",
                (now - 60_000) * 1_000_000
            )),
            log,
        )
        .unwrap();

        let Reply::Digest { digest } = registry.digest(None, DigestGroup::Repo).await.unwrap()
        else {
            panic!("not a digest");
        };
        assert!(!digest.away, "nothing typed yet: the last day");
        assert_eq!(digest.entries.len(), 1);
        let e = &digest.entries[0];
        assert_eq!(
            (e.outcome, e.detail.as_str()),
            (Outcome::Waiting, "PR #3 open")
        );
        assert_eq!(e.what, "tidy the readme");
        assert!(
            digest.markdown.contains("PR #3 open"),
            "{}",
            digest.markdown
        );

        // Typed long ago, then again now: the digest starts where you left.
        registry.presence.lock().unwrap().typed(now - 3 * 3_600_000);
        registry.you_typed();
        let saved = super::load(&super::file(&state));
        assert_eq!(
            saved.left_ms,
            Some(now - 3 * 3_600_000),
            "written down on coming back"
        );
        let Reply::Digest { digest } = registry.digest(None, DigestGroup::Day).await.unwrap()
        else {
            panic!("not a digest");
        };
        assert!(digest.away);
        assert_eq!(digest.since_ms, now - 3 * 3_600_000);
        let Reply::Digest { digest } = registry
            .digest(Some(now - 1_000), DigestGroup::Repo)
            .await
            .unwrap()
        else {
            panic!("not a digest");
        };
        assert!(digest.entries.is_empty(), "nothing in the last second");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

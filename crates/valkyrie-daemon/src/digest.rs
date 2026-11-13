//! The away digest (backlog #105): `ClientMsg::Digest` reads the sessions' event
//! logs (`valkyrie_context::digest`) and joins them with the backlog and the live
//! sessions. Also keeps `Presence`: when you last typed, so the digest starts
//! where you left. Search over session history (backlog #106) reads the same
//! logs the same way.

use crate::Registry;
use crate::session::now_ms;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use valkyrie_context::digest::{self, Known, Presence};
use valkyrie_context::project;
use valkyrie_context::search;
use valkyrie_proto::{Digest, DigestGroup, Item, Reply, SearchResults};

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
        let around = self.around();
        let dir = self.host.transcript_dir.clone();
        let mut digest = tokio::task::spawn_blocking(move || {
            let runs = read_runs(&dir, since, now, digest::read_run);
            let known = around.known();
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

/// What the daemon knows beside the logs, gathered to take off its thread.
struct Around {
    items: Vec<Item>,
    /// The conversations live sessions are in: one is never offered again.
    talking: HashSet<String>,
    /// Live sessions by log key. Exited ones stay listed until closed, but
    /// aren't running.
    live: BTreeMap<String, u32>,
    roots: Vec<PathBuf>,
}

impl Around {
    fn known(&self) -> Known<'_> {
        Known {
            items: &self.items,
            live: &self.live,
            roots: &self.roots,
        }
    }
}

impl Registry {
    fn around(&self) -> Around {
        let items: Vec<_> = self.items.borrow().iter().cloned().collect();
        let sessions = self.all();
        let live = sessions
            .iter()
            .filter(|s| !s.exited())
            .map(|s| (s.key(), s.id))
            .collect();
        let talking = sessions
            .iter()
            .filter(|s| !s.exited())
            .filter_map(|s| s.conversation())
            .collect();
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
        Around {
            items,
            talking,
            live,
            roots,
        }
    }

    /// The sessions whose history since `since_ms` (by default, as far back as a
    /// digest reaches) holds every word of `query`, newest first (backlog #106).
    pub(crate) async fn search(
        &self,
        query: String,
        since_ms: Option<u64>,
        repo: Option<String>,
        limit: Option<u32>,
    ) -> anyhow::Result<Reply> {
        let now = now_ms();
        let since = since_ms
            .unwrap_or(0)
            .min(now)
            .max(now.saturating_sub(digest::LONGEST_MS));
        let limit = limit.unwrap_or(search::DEFAULT_LIMIT).max(1) as usize;
        let around = self.around();
        let dir = self.host.transcript_dir.clone();
        let results = tokio::task::spawn_blocking(move || {
            let parsed = search::Query::parse(&query);
            let runs = read_runs(&dir, since, now, digest::read_said);
            let mut hits = search::hits(&runs, &around.known(), &parsed, repo.as_deref());
            let total = u32::try_from(hits.len()).unwrap_or(u32::MAX);
            hits.truncate(limit);
            let by_key: BTreeMap<&str, &digest::Run> =
                runs.iter().map(|r| (r.key.as_str(), r)).collect();
            for hit in hits.iter_mut().filter(|h| h.session.is_none()) {
                hit.resume = by_key
                    .get(hit.key.as_str())
                    .and_then(|run| resume(run, &around.talking));
            }
            SearchResults {
                query,
                since_ms: since,
                until_ms: now,
                hits,
                total,
            }
        })
        .await?;
        Ok(Reply::Search {
            results: Box::new(results),
        })
    }
}

/// The command that resumes a closed run's conversation, by its agent's own
/// rule for bringing a session back after a restart. An agent started from a
/// shell in it resumes as the agent alone, its options unknown. `None` while a
/// live session is in that conversation: two agents would write one transcript.
fn resume(run: &digest::Run, talking: &HashSet<String>) -> Option<Vec<String>> {
    let conversation = run
        .conversation
        .as_deref()
        .filter(|c| !talking.contains(*c))?;
    let agent = run.agent.as_deref()?;
    let command = if run.command.is_empty() || valkyrie_agents::is_shell(&run.command) {
        vec![agent.to_owned()]
    } else {
        run.command.clone()
    };
    let adapter = valkyrie_agents::adapter_for(&command);
    if adapter.name() != agent {
        return None;
    }
    adapter.restore(&command, Some(conversation))
}

/// The runs of every log written to since `since` that started by `until`, each
/// folded by `read`.
fn read_runs(
    dir: &Path,
    since: u64,
    until: u64,
    read: fn(&Path, u64, u64) -> anyhow::Result<digest::Run>,
) -> Vec<digest::Run> {
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
        match read(&path, since, until) {
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

    #[test]
    fn resume_follows_the_agent_and_skips_a_conversation_already_live() {
        let mut run = valkyrie_context::digest::Run {
            agent: Some("claude".into()),
            command: vec!["fish".into()],
            conversation: Some("c1".into()),
            ..Default::default()
        };
        let none = std::collections::HashSet::new();
        assert_eq!(
            super::resume(&run, &none),
            Some(vec!["claude".into(), "--resume".into(), "c1".into()]),
            "claude typed in a shell resumes as claude, not the shell"
        );
        let talking = std::collections::HashSet::from(["c1".to_owned()]);
        assert_eq!(super::resume(&run, &talking), None, "already resumed");
        run.agent = Some("codex".into());
        run.command = vec!["claude".into()];
        assert_eq!(
            super::resume(&run, &none),
            None,
            "command and agent disagree"
        );
        run.agent = None;
        run.command = vec!["fish".into()];
        assert_eq!(super::resume(&run, &none), None, "a plain shell");
    }

    /// Search reads the same logs, and offers a closed Claude session's resume.
    #[tokio::test]
    async fn search_finds_a_closed_session_and_how_to_resume_it() {
        let dir = std::env::temp_dir().join(format!("valkyrie-search-d-{}", std::process::id()));
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
        let cwd = dir.display().to_string();
        let log = |started: u64, prompt: &str, conv: &str| -> String {
            [
                json!({"k": "start", "t": started, "off": 0, "agent": "claude", "command": ["claude", "--model", "opus", "do it"]}),
                json!({"k": "hook", "t": started + 1, "off": 0, "agent": "claude", "ignored": false,
                    "payload": {"cwd": cwd, "hook_event_name": "SessionStart", "session_id": conv}}),
                json!({"k": "hook", "t": started + 2, "off": 0, "agent": "claude", "ignored": false,
                    "payload": {"cwd": cwd, "hook_event_name": "UserPromptSubmit", "prompt": prompt, "session_id": conv}}),
                json!({"k": "exit", "t": started + 3, "off": 0, "code": 0}),
            ]
            .into_iter()
            .map(|v| format!("{v}\n"))
            .collect()
        };
        for (i, (ago, prompt, conv)) in [
            (90_000, "rename the Zoom key", "c-old"),
            (60_000, "zoom out the docs", "c-new"),
            (40 * 86_400_000, "zoom from last month", "c-gone"),
        ]
        .into_iter()
        .enumerate()
        {
            let started = now - ago;
            std::fs::write(
                state.join(format!("sessions/{}-{i}.events.jsonl", started * 1_000_000)),
                log(started, prompt, conv),
            )
            .unwrap();
        }

        let Reply::Search { results } = registry
            .search("zoom".into(), None, None, Some(1))
            .await
            .unwrap()
        else {
            panic!("not search results");
        };
        assert_eq!(
            results.total, 2,
            "a log older than a digest reaches is left out"
        );
        assert_eq!(results.hits.len(), 1, "the limit");
        let hit = &results.hits[0];
        assert_eq!(hit.snippet, "zoom out the docs");
        assert_eq!(hit.session, None);
        assert_eq!(
            hit.resume.as_deref(),
            Some(&["claude", "--resume", "c-new", "--model", "opus"].map(String::from)[..]),
            "the first prompt isn't sent again"
        );

        let Reply::Search { results } = registry
            .search("ZOOM key".into(), None, Some("elsewhere".into()), None)
            .await
            .unwrap()
        else {
            panic!("not search results");
        };
        assert!(results.hits.is_empty(), "another repository");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

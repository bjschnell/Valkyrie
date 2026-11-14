//! What the supervisor watches (backlog #181): each open pull request the PR poll
//! read on its last pass, as it read it, for the supervisor page. The poll
//! (`watch_prs`) marks a pass's start and end; `ship_merged_prs` and
//! `watch_branches` say what they read in between. A PR that merged, closed, or
//! wasn't read on a pass (its item shipped, its session ended) drops off.

use crate::Registry;
use crate::backlog::pr_summary;
use crate::work::Pr;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use valkyrie_proto::{PrState, Reply, WatchedPr};

/// The poll's last reads, kept between passes.
#[derive(Debug, Default)]
pub(crate) struct Watching {
    prs: BTreeMap<String, (WatchedPr, u64)>,
    /// Which pass this is: a PR not read on the latest one is dropped at its end.
    pass: u64,
    polled_at: Option<u64>,
    every: Duration,
}

impl Watching {
    /// A pass begins.
    pub(crate) fn begin(&mut self) {
        self.pass += 1;
    }

    /// The pass that began last ended at `now`; the next is `every` after.
    pub(crate) fn end(&mut self, now: u64, every: Duration) {
        let pass = self.pass;
        self.prs.retain(|_, (_, seen)| *seen == pass);
        self.polled_at = Some(now);
        self.every = every;
    }

    /// What a pass read of `url`. A merged or closed PR isn't watched any more; one
    /// `gh` couldn't read keeps what was read before, marked unread.
    pub(crate) fn saw(
        &mut self,
        url: &str,
        project: PathBuf,
        item: Option<u32>,
        branch: Option<String>,
        pr: &Pr,
        now: u64,
    ) {
        let Some((state, detail)) = read(url, pr) else {
            self.prs.remove(url);
            return;
        };
        let pr = WatchedPr {
            url: url.to_owned(),
            project,
            item,
            branch,
            state,
            detail,
            read_at: now,
        };
        let pr = match (state, self.prs.remove(url)) {
            // Unread now: what it was last read as still says the most.
            (PrState::Unknown, Some((was, _))) => WatchedPr {
                state: PrState::Unknown,
                detail: format!("couldn't read it this time; last {}", was.state.label()),
                ..was
            },
            _ => pr,
        };
        self.prs.insert(url.to_owned(), (pr, self.pass));
    }

    /// `gh` couldn't say which PR `branch` in `project` has: the one last read for
    /// it, if any, stays on the page marked unread, as an item's would.
    pub(crate) fn unread_branch(&mut self, project: &Path, branch: &str, now: u64) {
        let known = self.prs.values().find_map(|(p, _)| {
            (p.item.is_none() && p.project == project && p.branch.as_deref() == Some(branch))
                .then(|| p.url.clone())
        });
        if let Some(url) = known {
            let branch = Some(branch.to_owned());
            self.saw(&url, project.to_owned(), None, branch, &Pr::Unknown, now);
        }
    }

    /// The reply: red and conflicting first, then the rest, each oldest read last.
    fn reply(&self, on: bool) -> Reply {
        let mut prs: Vec<WatchedPr> = self.prs.values().map(|(p, _)| p.clone()).collect();
        let rank = |s: PrState| match s {
            PrState::Conflicting => 0,
            PrState::Red => 1,
            PrState::Pending => 2,
            PrState::Unknown => 3,
            PrState::Green => 4,
        };
        prs.sort_by(|a, b| rank(a.state).cmp(&rank(b.state)).then(a.url.cmp(&b.url)));
        Reply::SupervisorWatch {
            prs,
            polled_at: self.polled_at,
            next_at: self.polled_at.map(|at| at + self.every.as_secs()),
            on,
        }
    }
}

/// What a read says of an open PR, in a word and a line; `None` for one that's
/// merged, closed or gone.
fn read(url: &str, pr: &Pr) -> Option<(PrState, String)> {
    match pr {
        Pr::Open {
            failed,
            conflicts,
            settled,
            ..
        } => Some(match pr_summary(url, failed, conflicts.as_deref()) {
            Some(summary) if conflicts.is_some() => (PrState::Conflicting, summary),
            Some(summary) => (PrState::Red, summary),
            None if *settled => (PrState::Green, "checks passed, merges cleanly".into()),
            None => (PrState::Pending, "checks still running".into()),
        }),
        Pr::Unknown => Some((PrState::Unknown, "couldn't read it".into())),
        Pr::None | Pr::Merged { .. } | Pr::Closed { .. } => None,
    }
}

impl Registry {
    /// `ClientMsg::SupervisorWatch`.
    pub(crate) fn supervisor_watch(&self) -> anyhow::Result<Reply> {
        let on = self.supervisor.lock().unwrap().on();
        Ok(self.watching.lock().unwrap().reply(on))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::Check;

    fn open(failed: &[&str], conflicts: Option<&str>, settled: bool) -> Pr {
        Pr::Open {
            url: String::new(),
            failed: failed
                .iter()
                .map(|n| Check {
                    name: (*n).into(),
                    url: String::new(),
                    run: None,
                })
                .collect(),
            conflicts: conflicts.map(Into::into),
            head: String::new(),
            settled,
            author: String::new(),
        }
    }

    const URL: &str = "https://github.com/o/r/pull/7";

    #[test]
    fn a_read_names_the_state_and_why() {
        assert_eq!(
            read(URL, &open(&["lint"], None, true)),
            Some((PrState::Red, "CI red on PR #7: lint".into()))
        );
        assert_eq!(
            read(URL, &open(&[], Some("main"), true)).map(|r| r.0),
            Some(PrState::Conflicting)
        );
        assert_eq!(
            read(URL, &open(&[], None, false)).map(|r| r.0),
            Some(PrState::Pending)
        );
        assert_eq!(
            read(URL, &open(&[], None, true)).map(|r| r.0),
            Some(PrState::Green)
        );
        assert_eq!(read(URL, &Pr::Closed { url: URL.into() }), None);
    }

    #[test]
    fn a_pass_keeps_what_it_read_and_drops_the_rest() {
        let mut w = Watching::default();
        let seen = |w: &Watching| match w.reply(true) {
            Reply::SupervisorWatch {
                prs,
                polled_at,
                next_at,
                ..
            } => (
                prs.into_iter()
                    .map(|p| (p.url, p.state, p.detail))
                    .collect::<Vec<_>>(),
                polled_at,
                next_at,
            ),
            _ => unreachable!(),
        };
        assert_eq!(seen(&w), (vec![], None, None));
        let other = "https://github.com/o/r/pull/8";
        w.begin();
        w.saw(URL, "/r".into(), Some(3), None, &open(&[], None, true), 100);
        w.saw(
            other,
            "/r".into(),
            None,
            None,
            &open(&["test"], None, true),
            100,
        );
        w.end(110, Duration::from_secs(300));
        let (prs, polled, next) = seen(&w);
        // Red first.
        assert_eq!(prs[0].1, PrState::Red);
        assert_eq!(prs[1].1, PrState::Green);
        assert_eq!((polled, next), (Some(110), Some(410)));
        // Next pass: #7 unreadable keeps what it was; #8 isn't read at all, so it goes.
        w.begin();
        w.saw(URL, "/r".into(), Some(3), None, &Pr::Unknown, 400);
        w.end(410, Duration::from_secs(300));
        let (prs, ..) = seen(&w);
        assert_eq!(
            prs,
            [(
                URL.to_owned(),
                PrState::Unknown,
                "couldn't read it this time; last green".to_owned()
            )]
        );
        // A branch PR `gh` can't find this pass stays, unread; an unknown branch adds nothing.
        let branch = "https://github.com/o/r/pull/9";
        w.begin();
        w.saw(
            branch,
            "/r".into(),
            None,
            Some("b".into()),
            &open(&[], None, true),
            420,
        );
        w.end(430, Duration::from_secs(300));
        w.begin();
        w.saw(URL, "/r".into(), Some(3), None, &Pr::Unknown, 440);
        w.unread_branch(Path::new("/r"), "b", 440);
        w.unread_branch(Path::new("/r"), "other", 440);
        w.end(450, Duration::from_secs(300));
        let (prs, ..) = seen(&w);
        assert_eq!(prs.len(), 2, "{prs:?}");
        assert!(prs.iter().any(|p| p.0 == branch && p.1 == PrState::Unknown));
        // Merged: no longer watched.
        w.begin();
        w.saw(
            URL,
            "/r".into(),
            Some(3),
            None,
            &Pr::Merged {
                url: URL.into(),
                base: "main".into(),
                author: String::new(),
            },
            500,
        );
        w.end(510, Duration::from_secs(300));
        assert!(seen(&w).0.is_empty());
    }
}

//! What the supervisor watches (backlog #181): each open pull request the PR poll
//! read on its last pass, as it read it, for the supervisor page. The poll
//! (`watch_prs`) marks a pass's start and end; `ship_merged_prs` and
//! `watch_branches` say what they read in between. A PR that merged, closed, or
//! wasn't read on a pass (its item shipped, its session ended) drops off.
//!
//! It also keeps the order to merge each repository's PRs in (backlog #215,
//! `merge_order`): which pairs collide, read with `git merge-tree` after each
//! pass. When a PR merges, each one predicted to collide with it is flagged as
//! conflicting on the next read (`foresee`), and the poll reads them again at
//! once, so its agent gets `merge-base` within seconds instead of after GitHub
//! works its mergeability out and the next pass reads that.

use crate::Registry;
use crate::backlog::pr_summary;
use crate::merge_order::{self, Open};
use crate::work::Pr;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use valkyrie_proto::{PrState, Reply, WatchedPr};

/// A prediction that a merge left a PR conflicting holds this long, unless
/// GitHub settles it first.
const FORESEE_SECS: u64 = 3600;
/// Pairs of heads `git merge-tree` reads per pass, at most.
const CLASHES_PER_PASS: usize = 40;

/// One PR as last read.
#[derive(Debug, Clone)]
struct Entry {
    pr: WatchedPr,
    /// The pass that read it.
    seen: u64,
    /// Its head commit; empty when `gh` didn't say.
    head: String,
}

/// A PR predicted to conflict with its base now that `after` merged into it.
#[derive(Debug, Clone)]
struct Doom {
    after: String,
    /// Its head when the prediction was made: a new one (the base merged in)
    /// ends it.
    head: String,
    base: String,
    files: Vec<String>,
    at: u64,
}

/// The poll's last reads, kept between passes.
#[derive(Debug, Default)]
pub(crate) struct Watching {
    prs: BTreeMap<String, Entry>,
    /// Which pass this is: a PR not read on the latest one is dropped at its end.
    pass: u64,
    polled_at: Option<u64>,
    every: Duration,
    /// The files each pair of heads conflicts on (empty: they merge cleanly), by
    /// the pair in sorted order.
    clashes: HashMap<(String, String), Vec<String>>,
    /// PRs a merge left conflicting, by URL, until GitHub says so itself.
    doomed: HashMap<String, Doom>,
    /// A merge doomed a PR: the poll reads them all again at once.
    rerun: bool,
}

fn pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_owned(), b.to_owned())
    } else {
        (b.to_owned(), a.to_owned())
    }
}

impl Watching {
    /// A pass begins.
    pub(crate) fn begin(&mut self) {
        self.pass += 1;
    }

    /// The pass that began last ended at `now`; the next is `every` after.
    pub(crate) fn end(&mut self, now: u64, every: Duration) {
        let pass = self.pass;
        self.prs.retain(|_, e| e.seen == pass);
        self.doomed.retain(|url, _| self.prs.contains_key(url));
        self.polled_at = Some(now);
        self.every = every;
        self.plan();
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
        if let Pr::Merged { base, .. } = pr {
            self.doom(url, base, now);
        }
        let Some((state, detail)) = read(url, pr) else {
            self.prs.remove(url);
            self.doomed.remove(url);
            return;
        };
        let head = match pr {
            Pr::Open { head, .. } => head.clone(),
            _ => String::new(),
        };
        let fresh = WatchedPr {
            url: url.to_owned(),
            project,
            item,
            branch,
            state,
            detail,
            read_at: now,
            order: None,
            merge_note: None,
        };
        let entry = match (state, self.prs.remove(url)) {
            // Unread now: what it was last read as still says the most.
            (PrState::Unknown, Some(was)) => Entry {
                pr: WatchedPr {
                    state: PrState::Unknown,
                    detail: format!("couldn't read it this time; last {}", was.pr.state.label()),
                    ..was.pr
                },
                head: was.head,
                seen: self.pass,
            },
            (_, was) => Entry {
                pr: WatchedPr {
                    // Kept until the pass replans.
                    order: was.as_ref().and_then(|w| w.pr.order),
                    merge_note: was.and_then(|w| w.pr.merge_note),
                    ..fresh
                },
                head,
                seen: self.pass,
            },
        };
        self.prs.insert(url.to_owned(), entry);
    }

    /// `gh` couldn't say which PR `branch` in `project` has: the one last read for
    /// it, if any, stays on the page marked unread, as an item's would.
    pub(crate) fn unread_branch(&mut self, project: &Path, branch: &str, now: u64) {
        let known = self.prs.values().find_map(|e| {
            let p = &e.pr;
            (p.item.is_none() && p.project == project && p.branch.as_deref() == Some(branch))
                .then(|| p.url.clone())
        });
        if let Some(url) = known {
            let branch = Some(branch.to_owned());
            self.saw(&url, project.to_owned(), None, branch, &Pr::Unknown, now);
        }
    }

    /// `url` merged into `base`: each PR of its repository known to collide with
    /// it will conflict with `base` now.
    fn doom(&mut self, url: &str, base: &str, now: u64) {
        let Some(merged) = self.prs.get(url).filter(|e| !e.head.is_empty()) else {
            return;
        };
        let base = if base.is_empty() {
            "its base branch"
        } else {
            base
        };
        let found: Vec<(String, Doom)> = self
            .prs
            .iter()
            .filter(|(other, e)| {
                *other != url && e.pr.project == merged.pr.project && !e.head.is_empty()
            })
            .filter_map(|(other, e)| {
                let files = self.clashes.get(&pair(&merged.head, &e.head))?;
                (!files.is_empty()).then(|| {
                    let doom = Doom {
                        after: url.to_owned(),
                        head: e.head.clone(),
                        base: base.to_owned(),
                        files: files.clone(),
                        at: now,
                    };
                    (other.clone(), doom)
                })
            })
            .collect();
        if !found.is_empty() {
            tracing::info!(
                pr = url,
                doomed = found.len(),
                "a merge left other PRs conflicting"
            );
            self.rerun = true;
        }
        self.doomed.extend(found);
    }

    /// What `gh` read of `url`, with a merge's prediction laid over it: an open PR
    /// a merge left conflicting reads as conflicting until GitHub settles its
    /// mergeability, its head moves (the base merged in), or an hour passes.
    pub(crate) fn foresee(&mut self, url: &str, pr: Pr, now: u64) -> Pr {
        let Some(doom) = self.doomed.get(url) else {
            return pr;
        };
        match pr {
            Pr::Open {
                url: u,
                failed,
                conflicts: None,
                head,
                settled: false,
                author,
            } if head == doom.head && now.saturating_sub(doom.at) < FORESEE_SECS => Pr::Open {
                url: u,
                failed,
                conflicts: Some(doom.base.clone()),
                head,
                settled: false,
                author,
            },
            // Unread: the prediction waits for a read.
            Pr::Unknown => Pr::Unknown,
            pr => {
                self.doomed.remove(url);
                pr
            }
        }
    }

    /// Whether a merge doomed a PR since this was last asked.
    pub(crate) fn take_rerun(&mut self) -> bool {
        std::mem::take(&mut self.rerun)
    }

    /// Pairs of heads, in one repository each, not yet read with `git merge-tree`.
    fn unchecked(&self) -> Vec<(PathBuf, String, String)> {
        let mut out = Vec::new();
        let mut asked = HashSet::new();
        for (project, heads) in self.by_project() {
            for (i, a) in heads.iter().enumerate() {
                for b in &heads[i + 1..] {
                    let key = pair(a, b);
                    if a != b && !self.clashes.contains_key(&key) && asked.insert(key) {
                        out.push((project.clone(), a.clone(), b.clone()));
                    }
                }
            }
        }
        out.truncate(CLASHES_PER_PASS);
        out
    }

    /// Each repository with more than one PR, and their heads.
    fn by_project(&self) -> BTreeMap<PathBuf, Vec<String>> {
        let mut all: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
        for e in self.prs.values().filter(|e| !e.head.is_empty()) {
            all.entry(e.pr.project.clone())
                .or_default()
                .push(e.head.clone());
        }
        all.retain(|p, heads| heads.len() > 1 && !p.as_os_str().is_empty());
        all
    }

    /// What `git merge-tree` read, then the order again.
    fn learn(&mut self, found: Vec<((String, String), Vec<String>)>) {
        for ((a, b), files) in found {
            self.clashes.insert(pair(&a, &b), files);
        }
        self.plan();
    }

    /// Each repository's merge order, and each PR's note.
    fn plan(&mut self) {
        let heads: HashSet<String> = self.prs.values().map(|e| e.head.clone()).collect();
        self.clashes
            .retain(|(a, b), _| heads.contains(a) && heads.contains(b));
        let mut groups: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
        for (url, e) in &self.prs {
            groups
                .entry(e.pr.project.clone())
                .or_default()
                .push(url.clone());
        }
        for urls in groups.values() {
            let opens: Vec<Open> = urls
                .iter()
                .map(|u| Open {
                    number: number(u),
                    state: self.prs[u].pr.state,
                })
                .collect();
            let head = |i: usize| self.prs[&urls[i]].head.as_str();
            let planned = merge_order::plan(&opens, |i, j| {
                if head(i).is_empty() || head(j).is_empty() {
                    return Vec::new();
                }
                self.clashes
                    .get(&pair(head(i), head(j)))
                    .cloned()
                    .unwrap_or_default()
            });
            let alone = urls.len() < 2;
            for (url, (place, note)) in urls.iter().zip(planned) {
                let note = match self.doomed.get(url) {
                    Some(d) => Some(format!(
                        "{} merged and conflicts with it on {}: merge {} in",
                        number(&d.after).map_or("a PR".to_owned(), |n| format!("#{n}")),
                        merge_order::shown(&d.files.iter().map(String::as_str).collect::<Vec<_>>()),
                        d.base
                    )),
                    None => note,
                };
                let e = self.prs.get_mut(url).unwrap();
                e.pr.order = (!alone).then_some(place);
                e.pr.merge_note = note;
            }
        }
    }

    /// The reply: each repository's PRs in the order to merge them.
    fn reply(&self, on: bool) -> Reply {
        let mut prs: Vec<WatchedPr> = self.prs.values().map(|e| e.pr.clone()).collect();
        let rank = |s: PrState| match s {
            PrState::Conflicting => 0,
            PrState::Red => 1,
            PrState::Pending => 2,
            PrState::Unknown => 3,
            PrState::Green => 4,
        };
        prs.sort_by(|a, b| {
            (&a.project, a.order, rank(a.state), &a.url).cmp(&(
                &b.project,
                b.order,
                rank(b.state),
                &b.url,
            ))
        });
        Reply::SupervisorWatch {
            prs,
            polled_at: self.polled_at,
            next_at: self.polled_at.map(|at| at + self.every.as_secs()),
            on,
        }
    }
}

/// A PR's number, from its URL.
fn number(url: &str) -> Option<u32> {
    url.rsplit_once("/pull/")?
        .1
        .split(['/', '#', '?'])
        .next()?
        .parse()
        .ok()
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
    /// Reads which of each repository's open PRs collide, for the merge order
    /// (backlog #215): local git only, after a pass's reads.
    pub(crate) async fn plan_merges(&self) {
        let jobs = self.watching.lock().unwrap().unchecked();
        let found = tokio::task::spawn_blocking(move || {
            jobs.into_iter()
                .filter(|(dir, ..)| dir.is_dir())
                .filter_map(|(dir, a, b)| {
                    Some(((a.clone(), b.clone()), merge_order::clash(&dir, &a, &b)?))
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        self.watching.lock().unwrap().learn(found);
    }

    /// `Watching::foresee`, for a read of `url` just made.
    pub(crate) fn foresee(&self, url: &str, pr: Pr) -> Pr {
        let now = crate::session::now_ms() / 1000;
        self.watching.lock().unwrap().foresee(url, pr, now)
    }

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

    /// An open PR on `head`.
    fn at(head: &str, settled: bool) -> Pr {
        match open(&[], None, settled) {
            Pr::Open {
                url,
                failed,
                conflicts,
                settled,
                author,
                ..
            } => Pr::Open {
                url,
                failed,
                conflicts,
                head: head.into(),
                settled,
                author,
            },
            _ => unreachable!(),
        }
    }

    /// `at`, read from `url`.
    fn reads(url: &str, head: &str, settled: bool) -> Pr {
        match at(head, settled) {
            Pr::Open {
                failed,
                conflicts,
                head,
                settled,
                author,
                ..
            } => Pr::Open {
                url: url.to_owned(),
                failed,
                conflicts,
                head,
                settled,
                author,
            },
            _ => unreachable!(),
        }
    }

    fn pr(n: u32) -> String {
        format!("https://github.com/o/r/pull/{n}")
    }

    fn listed(w: &Watching) -> Vec<(String, Option<u32>, Option<String>)> {
        match w.reply(true) {
            Reply::SupervisorWatch { prs, .. } => prs
                .into_iter()
                .map(|p| (p.url, p.order, p.merge_note))
                .collect(),
            _ => unreachable!(),
        }
    }

    /// Three PRs in a repository, two of which collide: the pairs are asked about
    /// once, the order puts the clean one first, and when one of the colliding
    /// pair merges the other reads as conflicting until GitHub settles it or its
    /// head moves.
    #[test]
    fn a_merge_order_and_what_a_merge_leaves_conflicting() {
        let mut w = Watching::default();
        w.begin();
        w.saw(&pr(3), "/r".into(), Some(1), None, &at("c3", true), 100);
        w.saw(&pr(4), "/r".into(), Some(2), None, &at("c4", true), 100);
        w.saw(
            &pr(5),
            "/r".into(),
            None,
            Some("b".into()),
            &at("c5", false),
            100,
        );
        // Alone in its repository: no order.
        w.saw(&pr(9), "/s".into(), Some(8), None, &at("c9", true), 100);
        w.end(110, Duration::from_secs(300));
        let mut asked: Vec<(String, String)> = w
            .unchecked()
            .into_iter()
            .map(|(_, a, b)| pair(&a, &b))
            .collect();
        asked.sort();
        assert_eq!(
            asked,
            [pair("c3", "c4"), pair("c3", "c5"), pair("c4", "c5")]
        );
        w.learn(vec![
            (pair("c3", "c4"), vec!["src/a.rs".into()]),
            (pair("c3", "c5"), vec![]),
            (pair("c4", "c5"), vec![]),
        ]);
        assert!(w.unchecked().is_empty(), "asked once");
        assert_eq!(
            listed(&w),
            [
                (
                    pr(3),
                    Some(1),
                    Some("merge before #4: conflicts on src/a.rs".into())
                ),
                (
                    pr(4),
                    Some(2),
                    Some(
                        "merge after #3: conflicts on src/a.rs; then it needs the base merged in"
                            .into()
                    )
                ),
                (pr(5), Some(3), None),
                (pr(9), None, None),
            ]
        );
        // #3 merges: #4 is doomed, and the poll reads again.
        assert!(!w.take_rerun());
        w.begin();
        w.saw(&pr(3), "/r".into(), Some(1), None, &Pr::merged(pr(3)), 200);
        assert!(w.take_rerun());
        // GitHub hasn't worked #4 out yet: it reads as conflicting with main.
        let read = w.foresee(&pr(4), at("c4", false), 201);
        assert!(matches!(&read, Pr::Open { conflicts: Some(b), .. } if b == "main"));
        w.saw(&pr(4), "/r".into(), Some(2), None, &read, 201);
        w.saw(
            &pr(5),
            "/r".into(),
            None,
            Some("b".into()),
            &at("c5", false),
            201,
        );
        w.end(210, Duration::from_secs(300));
        // #4 needs main merged in now, so it goes after #5.
        let now = listed(&w);
        assert_eq!(now[1].0, pr(4));
        assert_eq!(now[1].1, Some(2));
        assert_eq!(
            now[1].2.as_deref(),
            Some("#3 merged and conflicts with it on src/a.rs: merge main in")
        );
        assert_eq!(w.prs[&pr(4)].pr.state, PrState::Conflicting);
        // An unread read keeps the prediction; a new head (main merged in) ends it.
        assert!(matches!(w.foresee(&pr(4), Pr::Unknown, 220), Pr::Unknown));
        assert!(matches!(
            w.foresee(&pr(4), at("c4", false), 230),
            Pr::Open {
                conflicts: Some(_),
                ..
            }
        ));
        assert!(matches!(
            w.foresee(&pr(4), at("c4b", false), 240),
            Pr::Open {
                conflicts: None,
                ..
            }
        ));
        assert!(matches!(
            w.foresee(&pr(4), at("c4", false), 250),
            Pr::Open {
                conflicts: None,
                ..
            }
        ));
    }

    /// GitHub's settled read wins over the prediction, and it lapses after an hour.
    #[test]
    fn github_settling_or_an_hour_ends_a_prediction() {
        let doomed = |w: &mut Watching| {
            w.begin();
            w.saw(&pr(3), "/r".into(), None, None, &at("c3", true), 100);
            w.saw(&pr(4), "/r".into(), None, None, &at("c4", true), 100);
            w.end(110, Duration::from_secs(300));
            w.learn(vec![(pair("c3", "c4"), vec!["a".into()])]);
            w.begin();
            w.saw(&pr(3), "/r".into(), None, None, &Pr::merged(pr(3)), 200);
        };
        let mut w = Watching::default();
        doomed(&mut w);
        assert!(matches!(
            w.foresee(&pr(4), at("c4", true), 201),
            Pr::Open {
                conflicts: None,
                ..
            }
        ));
        let mut w = Watching::default();
        doomed(&mut w);
        assert!(matches!(
            w.foresee(&pr(4), at("c4", false), 200 + FORESEE_SECS),
            Pr::Open {
                conflicts: None,
                ..
            }
        ));
    }

    /// Through the poll: two items' PRs collide, one merges, and the other's idle
    /// agent is flagged as conflicting with main on that pass, with `merge-base`
    /// offered, while GitHub still hasn't worked its mergeability out.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_merge_flags_the_pr_it_collides_with_at_once() {
        use crate::work::PrLookup;
        use std::sync::Arc;
        use valkyrie_proto::{
            AgentState, ItemChange, ItemStatus, NewItem, Size, SpawnSpec, StepAction,
        };
        let dir = std::env::temp_dir().join(format!("valkyrie-foresee-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (registry, _) = crate::new_registry(
            dir.join("v.sock"),
            &dir.join("state"),
            Path::new("/bin/true"),
            1,
            0,
            7,
            false,
        )
        .unwrap();
        let spawn = || {
            let spec = SpawnSpec {
                command: vec!["sleep".into(), "30".into()],
                cwd: Some(dir.clone()),
                name: None,
                size: Size { cols: 40, rows: 10 },
                env: Vec::new(),
                worktree: None,
                hidden: false,
            };
            let Reply::Session { info } = crate::spawn(&registry, spec).unwrap() else {
                panic!("no session");
            };
            registry.get(info.id).unwrap()
        };
        let (first, second) = (spawn(), spawn());
        for (session, n) in [(&first, 1), (&second, 2)] {
            let b = registry.backlog.lock().unwrap();
            let new = NewItem {
                title: format!("item {n}"),
                cwd: Some(dir.clone()),
                ..Default::default()
            };
            let item = b.add(&new, "human", 1).unwrap();
            let open = ItemChange::Status {
                status: ItemStatus::Open,
            };
            b.change(item.id, &open, 1).unwrap();
            b.taken(
                item.id,
                session.info().id,
                registry.boot,
                &[],
                None,
                Some(&pr(n)),
                None,
                1,
            )
            .unwrap();
        }
        let pass = |lookup: PrLookup| {
            let registry = registry.clone();
            async move {
                registry.watching.lock().unwrap().begin();
                registry.ship_merged_prs(lookup).await;
                registry
                    .watching
                    .lock()
                    .unwrap()
                    .end(0, Duration::from_secs(300));
            }
        };
        let both_open: PrLookup = Arc::new(|_, url| {
            let head = if url.ends_with("/1") { "h1" } else { "h2" };
            reads(url, head, true)
        });
        pass(both_open).await;
        // What `plan_merges` would read with git: the two collide.
        registry
            .watching
            .lock()
            .unwrap()
            .learn(vec![(pair("h1", "h2"), vec!["src/a.rs".into()])]);
        assert_eq!(second.info().status.state, AgentState::Idle);
        // #1 merges; GitHub says nothing of #2's mergeability yet.
        let merged: PrLookup = Arc::new(|_, url| {
            if url.ends_with("/1") {
                Pr::merged(url)
            } else {
                reads(url, "h2", false)
            }
        });
        pass(merged.clone()).await;
        if registry.watching.lock().unwrap().take_rerun() {
            pass(merged).await;
        }
        let status = second.info().status;
        assert_eq!(status.state, AgentState::Blocked);
        assert_eq!(status.summary.as_deref(), Some("PR #2 conflicts with main"));
        let flag = registry.flag_of(&second, second.info().id).unwrap();
        assert_eq!(flag.step.unwrap().action, StepAction::MergeBase);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_number_comes_from_the_url() {
        assert_eq!(number("https://github.com/o/r/pull/236"), Some(236));
        assert_eq!(number("https://github.com/o/r/pull/236/files"), Some(236));
        assert_eq!(number("nope"), None);
    }

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
        // In the order to merge them: green first.
        assert_eq!(prs[0].1, PrState::Green);
        assert_eq!(prs[1].1, PrState::Red);
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

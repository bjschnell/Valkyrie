//! The supervisor page (backlog #180): `s` on the home screen. What the
//! supervisor waits on you for, each with its keys, and what it did: the steps
//! you answered, the ones policies took without asking, the build orders you
//! answered. Opened on purpose and closed again, never a pane that stays up.

use crate::backlog::fit_hints;
use crate::theme::Theme;
use crate::{fit, next_key};
use ratatui::layout::Rect;
use ratatui::style::{self, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use std::path::PathBuf;
use valkyrie_proto::PrState;
use valkyrie_proto::supervisor::Watch;
use valkyrie_proto::{
    Decision, DecisionKind, DecisionStatus, LaunchOffer, OrderProposal, QueueItem, SessionId, Step,
    StepAction, SupervisorEntry,
};

/// Something the supervisor waits on you for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Wait {
    /// A flagged session's pull request, and the step offered on it.
    Flag {
        session: SessionId,
        name: String,
        reason: String,
        seq: u64,
        step: Step,
    },
    /// A ready item it offers to start.
    Launch(LaunchOffer),
    /// A build order to review.
    Order {
        project: PathBuf,
        id: u64,
        steps: usize,
    },
    /// A step policy it learned from your answers, to accept or reject.
    Policy(Decision),
}

/// Everything waiting on you, in the order the page lists it: flags first, since
/// a red or conflicting PR is the most urgent, then ready items, build orders and
/// policies.
pub(crate) fn waiting(
    queue: &[QueueItem],
    launches: &[LaunchOffer],
    orders: &[OrderProposal],
    proposals: &[Decision],
) -> Vec<Wait> {
    let flags = queue.iter().filter_map(|q| {
        let flag = q.flag.as_ref()?;
        Some(Wait::Flag {
            session: q.session,
            name: q.name.clone(),
            reason: flag.reason.clone(),
            seq: flag.seq,
            step: flag.step.clone()?,
        })
    });
    let launches = launches.iter().cloned().map(Wait::Launch);
    let orders = orders.iter().map(|o| Wait::Order {
        project: o.project.clone(),
        id: o.id,
        steps: o.steps.len(),
    });
    let policies = proposals
        .iter()
        .filter(|d| d.kind == DecisionKind::Policy && d.status == DecisionStatus::Proposed)
        .cloned()
        .map(Wait::Policy);
    flags
        .chain(launches)
        .chain(orders)
        .chain(policies)
        .collect()
}

/// What a key on the page asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Do {
    Close,
    /// Fetch the log again.
    Load,
    TakeStep {
        session: SessionId,
        seq: u64,
        action: StepAction,
    },
    AnswerLaunch {
        project: PathBuf,
        id: u64,
        launch: bool,
    },
    /// Open the backlog with this repository's build order over it.
    OpenOrder(PathBuf),
    /// Accept or reject a proposed policy.
    Review {
        project: PathBuf,
        id: u32,
        seen: u64,
        accept: bool,
    },
    Say(String),
}

/// The open page: the log as the daemon last said it, and which waiting row the
/// cursor is on.
#[derive(Debug, Default)]
pub(crate) struct Page {
    log: Option<(Vec<SupervisorEntry>, bool)>,
    failed: Option<String>,
    /// The pull requests it watches, as the poll last read them (backlog #181).
    watch: Option<Result<Watch, String>>,
    cursor: usize,
}

impl Page {
    pub(crate) fn watched(&mut self, got: Result<Watch, String>) {
        self.watch = Some(got);
    }

    pub(crate) fn loaded(&mut self, got: Result<(Vec<SupervisorEntry>, bool), String>) {
        match got {
            Ok(log) => {
                self.log = Some(log);
                self.failed = None;
            }
            Err(e) => self.failed = Some(e),
        }
    }

    /// Keys while the page is open: ↑/↓ (j/k) pick what waits; `a` (or ↩) takes
    /// what's offered, `o` the other step, `d` leaves or rejects it; ↩ on a build
    /// order opens it; `r` loads again; Esc or `q` closes.
    pub(crate) fn keys(&mut self, mut bytes: &[u8], waits: &[Wait]) -> Vec<Do> {
        let mut out = Vec::new();
        let n = waits.len();
        self.cursor = self.cursor.min(n.saturating_sub(1));
        while !bytes.is_empty() {
            let key = next_key(bytes);
            let whole = key.len() == bytes.len();
            bytes = &bytes[key.len()..];
            let here = waits.get(self.cursor);
            match key {
                b"k" | b"\x1b[A" | b"\x1bOA" => self.cursor = self.cursor.saturating_sub(1),
                b"j" | b"\x1b[B" | b"\x1bOB" => {
                    self.cursor = (self.cursor + 1).min(n.saturating_sub(1))
                }
                b"a" | b"\r" | b"\n" => out.extend(here.map(|w| take(w, true))),
                b"d" => out.extend(here.map(|w| take(w, false))),
                b"o" => match here {
                    Some(Wait::Flag {
                        session,
                        seq,
                        step: Step { alt: Some(alt), .. },
                        ..
                    }) => out.push(Do::TakeStep {
                        session: *session,
                        seq: *seq,
                        action: *alt,
                    }),
                    Some(_) => out.push(Do::Say("no other step offered there".into())),
                    None => {}
                },
                b"r" => out.push(Do::Load),
                b"\x1b" if whole => out.push(Do::Close),
                b"q" | [0x03] => out.push(Do::Close),
                _ => {}
            }
        }
        out
    }

    pub(crate) fn draw(&self, frame: &mut ratatui::Frame, waits: &[Wait], now_ms: u64, t: &Theme) {
        let area = frame.area();
        let rect = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        )
        .intersection(area);
        frame.render_widget(Clear, rect);
        let inner = rect.width.saturating_sub(4) as usize;
        let cursor = self.cursor.min(waits.len().saturating_sub(1));

        let mut lines: Vec<Line> = vec![Line::default()];
        lines.push(Line::from(vec![
            " Waiting on you ".fg(t.accent2).bold(),
            format!("· {}", waits.len()).fg(t.muted),
        ]));
        if waits.is_empty() {
            lines.push(
                "   Nothing: no flag, ready item, build order or policy waits."
                    .fg(t.muted)
                    .into(),
            );
        }
        let mut at_cursor = 0;
        for (i, w) in waits.iter().enumerate() {
            let here = i == cursor;
            if here {
                at_cursor = lines.len();
            }
            let (tag, what, keys) = describe(w);
            let mark = if here { "▌".fg(t.accent) } else { " ".into() };
            let room = inner.saturating_sub(8 + keys.chars().count() + 6);
            let mut line = Line::from(vec![
                mark,
                // One width for every tag, so the titles line up.
                format!(" {tag:^6} ").fg(t.bg).bg(t.accent2).bold(),
                " ".into(),
                fit(&what, room).fg(t.fg),
                format!("  {keys}").fg(t.accent),
            ]);
            if here {
                line = line.style(style::Style::new().bg(t.selection));
            }
            lines.push(line);
        }
        lines.push(Line::default());
        lines.extend(watch_lines(self.watch.as_ref(), inner, now_ms / 1000, t));
        lines.push(Line::default());
        let on = self.log.as_ref().is_none_or(|(_, on)| *on);
        lines.push(Line::from(vec![
            " What it did ".fg(t.accent2).bold(),
            if on {
                "· last 90 days".fg(t.muted)
            } else {
                "· the supervisor is off: `valk supervise on`".fg(t.needs)
            },
        ]));
        match (&self.log, &self.failed) {
            (_, Some(e)) => lines.push(
                format!("   Couldn't load it: {}", fit(e, inner.saturating_sub(20)))
                    .fg(t.needs)
                    .into(),
            ),
            (None, None) => lines.push("   Loading…".fg(t.muted).into()),
            (Some((entries, _)), None) if entries.is_empty() => {
                lines.push("   Nothing yet.".fg(t.muted).into())
            }
            (Some((entries, _)), None) => {
                for e in entries {
                    let when = crate::age(e.at * 1000, now_ms);
                    let repo = e
                        .project
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let head = format!("   {when:>4}  ");
                    let room = inner.saturating_sub(head.len() + repo.len() + 3);
                    let color = if e.policy.is_some() { t.accent } else { t.fg };
                    lines.push(Line::from(vec![
                        head.fg(t.muted),
                        fit(&e.said, room).fg(color),
                        format!("  {repo}").fg(t.muted),
                    ]));
                }
            }
        }
        // Keep the waiting row under the cursor in view; the log scrolls under it.
        let room = (rect.height as usize).saturating_sub(2);
        let skip = (at_cursor + 1).saturating_sub(room);
        let lines: Vec<Line> = lines.into_iter().skip(skip).collect();

        let hint = |k: &'static str, w: &'static str| [k.fg(t.accent).bold(), w.fg(t.muted)];
        let hints = [
            hint(" a ", "take "),
            hint(" o ", "other step "),
            hint(" d ", "leave/reject "),
            hint(" r ", "reload "),
            hint(" esc ", "close "),
        ]
        .concat();
        let hints = fit_hints(hints, rect.width.saturating_sub(4) as usize);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(style::Style::new().fg(t.accent))
            .title(" Supervisor ".fg(t.fg).bold())
            .title_bottom(Line::from(hints).centered())
            .style(style::Style::new().bg(t.panel).fg(t.fg));
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }
}

/// The "Watching" section: each open PR the poll read, its state and why, and
/// when the poll reads them next.
fn watch_lines(
    watch: Option<&Result<Watch, String>>,
    inner: usize,
    now: u64,
    t: &Theme,
) -> Vec<Line<'static>> {
    let ago = |at: u64| crate::age(at * 1000, now * 1000);
    let mut lines = Vec::new();
    let (head, note) = match watch {
        Some(Ok(w)) if !w.on => (" Watching ", "· off: nothing is read".to_owned()),
        Some(Ok(w)) => (
            " Watching ",
            match (w.polled_at, w.next_at) {
                (Some(at), Some(next)) if next > now => format!(
                    "· {} PRs, read {} ago, next in {}",
                    w.prs.len(),
                    ago(at),
                    crate::age(now * 1000, next * 1000)
                ),
                (Some(at), _) => format!("· {} PRs, read {} ago", w.prs.len(), ago(at)),
                (None, _) => "· the poll hasn't run yet".to_owned(),
            },
        ),
        _ => (" Watching ", String::new()),
    };
    lines.push(Line::from(vec![
        head.fg(t.accent2).bold(),
        note.fg(t.muted),
    ]));
    match watch {
        None => lines.push("   Loading…".fg(t.muted).into()),
        Some(Err(e)) => lines.push(
            format!("   Couldn't load it: {}", fit(e, inner.saturating_sub(20)))
                .fg(t.needs)
                .into(),
        ),
        Some(Ok(w)) if w.prs.is_empty() && w.on => {
            lines.push("   No open pull request of yours.".fg(t.muted).into())
        }
        Some(Ok(w)) => {
            for pr in &w.prs {
                let color = match pr.state {
                    PrState::Red | PrState::Conflicting => t.needs,
                    PrState::Green => t.done,
                    PrState::Pending | PrState::Unknown => t.muted,
                };
                let whose = match (pr.item, &pr.branch) {
                    (Some(i), _) => format!("#{i}"),
                    (None, Some(b)) => b.clone(),
                    (None, None) => String::new(),
                };
                let state = format!(" {:<11} ", pr.state.label());
                let room = inner.saturating_sub(state.len() + whose.len() + 8);
                lines.push(Line::from(vec![
                    "   ".into(),
                    state.fg(color).bold(),
                    fit(&pr.detail, room).fg(t.fg),
                    format!("  {whose}").fg(t.muted),
                ]));
            }
        }
    }
    lines
}

/// What `a` (`yes`) or `d` does on `w`.
fn take(w: &Wait, yes: bool) -> Do {
    match w {
        Wait::Flag {
            session, seq, step, ..
        } => Do::TakeStep {
            session: *session,
            seq: *seq,
            action: if yes { step.action } else { StepAction::Leave },
        },
        Wait::Launch(o) => Do::AnswerLaunch {
            project: o.project.clone(),
            id: o.id,
            launch: yes,
        },
        // Accepting one is a page of ticks, not a key: it opens where they are.
        Wait::Order { project, .. } if yes => Do::OpenOrder(project.clone()),
        Wait::Order { .. } => Do::Say("↩ opens the build order: d declines it there".into()),
        Wait::Policy(d) => Do::Review {
            project: d.project.clone(),
            id: d.id,
            seen: d.updated,
            accept: yes,
        },
    }
}

/// A waiting row's tag, one line on it, and its keys.
fn describe(w: &Wait) -> (&'static str, String, String) {
    match w {
        Wait::Flag {
            name, reason, step, ..
        } => (
            "PR",
            format!("{name}: {reason}"),
            match step.alt {
                Some(alt) => format!("a {} · o {} · d leave", step.action.label(), alt.label()),
                None => format!("a {} · d leave", step.action.label()),
            },
        ),
        Wait::Launch(o) => (
            "ready",
            format!("#{} {} ({})", o.item, o.title, o.why),
            "a start · d leave".into(),
        ),
        Wait::Order { project, steps, .. } => (
            "order",
            format!(
                "build order for {}: {steps} items",
                project
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
            "↩ review".into(),
        ),
        Wait::Policy(d) => (
            "policy",
            format!("#{} {}", d.id, d.title),
            "a accept · d reject".into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> LaunchOffer {
        serde_json::from_value(serde_json::json!({
            "id": 5, "project": "/r/app", "item": 45, "title": "fix it",
            "why": "its blockers shipped", "agent": "claude", "made": 0
        }))
        .unwrap()
    }

    fn policy(status: DecisionStatus, kind: DecisionKind) -> Decision {
        let mut d: Decision = serde_json::from_value(serde_json::json!({
            "id": 12, "project": "/r/app", "title": "merge-base on conflicting",
            "body": "", "kind": "policy", "status": "proposed", "created": 0, "updated": 3
        }))
        .unwrap();
        d.status = status;
        d.kind = kind;
        d
    }

    #[test]
    fn waiting_lists_ready_items_orders_and_only_proposed_policies() {
        let order: OrderProposal = serde_json::from_value(serde_json::json!({
            "id": 9, "project": "/r/app", "made": 0, "trigger": "you asked",
            "steps": [{"item": 1, "reason": ""}, {"item": 2, "reason": ""}]
        }))
        .unwrap();
        let waits = waiting(
            &[],
            &[launch()],
            &[order],
            &[
                policy(DecisionStatus::Proposed, DecisionKind::Policy),
                policy(DecisionStatus::Active, DecisionKind::Policy),
                policy(DecisionStatus::Proposed, DecisionKind::Gotcha),
            ],
        );
        assert_eq!(waits.len(), 3);
        assert!(matches!(waits[0], Wait::Launch(_)));
        assert!(matches!(waits[1], Wait::Order { steps: 2, .. }));
        assert!(matches!(&waits[2], Wait::Policy(d) if d.id == 12));
    }

    #[test]
    fn keys_take_leave_and_open_what_waits() {
        let order = Wait::Order {
            project: "/r/app".into(),
            id: 9,
            steps: 2,
        };
        let flag = Wait::Flag {
            session: 4,
            name: "app".into(),
            reason: "PR #98 conflicts with main".into(),
            seq: 7,
            step: Step {
                action: StepAction::MergeBase,
                why: "conflicting".into(),
                alt: None,
            },
        };
        let waits = [
            flag,
            Wait::Launch(launch()),
            order,
            Wait::Policy(policy(DecisionStatus::Proposed, DecisionKind::Policy)),
        ];
        let mut page = Page::default();
        assert_eq!(
            page.keys(b"a", &waits),
            [Do::TakeStep {
                session: 4,
                seq: 7,
                action: StepAction::MergeBase
            }]
        );
        assert!(matches!(&page.keys(b"o", &waits)[..], [Do::Say(_)]));
        assert_eq!(
            page.keys(b"d", &waits),
            [Do::TakeStep {
                session: 4,
                seq: 7,
                action: StepAction::Leave
            }]
        );
        assert_eq!(
            page.keys(b"jd", &waits),
            [Do::AnswerLaunch {
                project: "/r/app".into(),
                id: 5,
                launch: false
            }]
        );
        assert_eq!(page.keys(b"j\r", &waits), [Do::OpenOrder("/r/app".into())]);
        assert!(matches!(&page.keys(b"d", &waits)[..], [Do::Say(_)]));
        assert_eq!(
            page.keys(b"ja", &waits),
            [Do::Review {
                project: "/r/app".into(),
                id: 12,
                seen: 3,
                accept: true
            }]
        );
        // Past the end stays on the last; Esc closes.
        assert!(page.keys(b"jjj", &waits).is_empty());
        assert_eq!(page.keys(b"\x1b", &waits), [Do::Close]);
    }

    #[test]
    fn watching_says_each_pr_and_when_it_reads_them_next() {
        let text = |w: Option<&Result<Watch, String>>| {
            watch_lines(w, 80, 1_000, &crate::theme::VALKYRIE)
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
        };
        let pr: valkyrie_proto::WatchedPr = serde_json::from_value(serde_json::json!({
            "url": "u", "project": "/r/app", "item": 12, "state": "red",
            "detail": "CI red on PR #101: lint", "read_at": 880
        }))
        .unwrap();
        let w = Ok(Watch {
            prs: vec![pr],
            polled_at: Some(880),
            next_at: Some(1_180),
            on: true,
        });
        let lines = text(Some(&w));
        assert!(
            lines[0].contains("1 PRs, read 2m ago, next in 3m"),
            "{lines:?}"
        );
        assert!(lines[1].contains("red") && lines[1].contains("lint") && lines[1].contains("#12"));
        let off = Ok(Watch::default());
        assert!(text(Some(&off))[0].contains("off"));
        assert!(text(None)[1].contains("Loading"));
    }
}

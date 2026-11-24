//! Start agents (backlog #280): the composer's draft of several lines, one
//! session per line. A dialog over a dimmed home (rule 8): each line with the
//! repository and agent it starts with, to check and change before anything
//! runs. ↩ starts them all; Esc goes back to the draft as it was.

use crate::compose::{AGENTS, Defaults};
use crate::next_key;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::path::{Path, PathBuf};

/// One line of the draft: a session to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) task: String,
    pub(crate) repo: Option<PathBuf>,
    /// An index into `AGENTS`.
    pub(crate) agent: usize,
    /// Why its start failed, the last time ↩ tried.
    pub(crate) failed: Option<String>,
}

impl Row {
    /// What `compose::command` starts it with.
    pub(crate) fn defaults(&self) -> Defaults {
        let (agent, model) = AGENTS[self.agent % AGENTS.len()];
        Defaults {
            repo: self.repo.clone(),
            agent,
            model,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Batch {
    pub(crate) rows: Vec<Row>,
    pub(crate) cursor: usize,
}

/// What a key asked of the dialog's owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Did {
    /// ↩: start every row.
    Start,
    /// Esc, or the last row dropped: back to the draft.
    Back,
}

impl Batch {
    /// A row per non-empty line of `draft`, each with `d`'s repository unless
    /// the line names another of `repos`, and `d`'s agent.
    pub(crate) fn new(draft: &str, d: &Defaults, repos: &[PathBuf]) -> Self {
        let agent = AGENTS
            .iter()
            .position(|&(a, m)| a == d.agent && m == d.model)
            .unwrap_or(0);
        let rows = draft
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|task| Row {
                task: task.to_owned(),
                repo: named_repo(task, repos).or_else(|| d.repo.clone()),
                agent,
                failed: None,
            })
            .collect();
        Self { rows, cursor: 0 }
    }

    /// The keys of one read.
    pub(crate) fn keys(&mut self, bytes: &[u8], repos: &[PathBuf]) -> Vec<Did> {
        let mut out = Vec::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            let key = next_key(rest);
            let whole = rest.len() == key.len();
            rest = &rest[key.len()..];
            let last = self.rows.len().saturating_sub(1);
            match key {
                b"j" | b"\x1b[B" | b"\x1bOB" => self.cursor = (self.cursor + 1).min(last),
                b"k" | b"\x1b[A" | b"\x1bOA" => self.cursor = self.cursor.saturating_sub(1),
                b"\t" => {
                    if let Some(row) = self.rows.get_mut(self.cursor) {
                        row.repo = next_repo(row.repo.as_deref(), repos);
                        row.failed = None;
                    }
                }
                b"\x1b[Z" => {
                    if let Some(row) = self.rows.get_mut(self.cursor) {
                        row.agent = (row.agent + 1) % AGENTS.len();
                        row.failed = None;
                    }
                }
                b"d" => {
                    if self.cursor < self.rows.len() {
                        self.rows.remove(self.cursor);
                    }
                    self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
                    if self.rows.is_empty() {
                        out.push(Did::Back);
                        break;
                    }
                }
                b"\r" | b"\n" => {
                    out.push(Did::Start);
                    // What follows in the read would act on rows already started.
                    break;
                }
                b"\x1b" if whole => {
                    out.push(Did::Back);
                    break;
                }
                [0x03] => {
                    out.push(Did::Back);
                    break;
                }
                // Alt and a key: neither is the dialog's.
                b"\x1b" if !rest.is_empty() => rest = &rest[next_key(rest).len()..],
                _ => {}
            }
        }
        out
    }

    /// The draft the rows came from, as they stand now: Esc gives it back.
    pub(crate) fn draft(&self) -> String {
        self.rows
            .iter()
            .map(|r| r.task.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `3 tasks · api, web`: how many, and where.
    pub(crate) fn summary(&self) -> String {
        let mut repos: Vec<String> = Vec::new();
        for r in &self.rows {
            let name = repo_name(r.repo.as_deref());
            if !repos.contains(&name) {
                repos.push(name);
            }
        }
        let n = self.rows.len();
        format!(
            "{n} task{} · {}",
            if n == 1 { "" } else { "s" },
            repos.join(", ")
        )
    }

    /// The dialog, centered over what `dim` has already stepped back.
    pub(crate) fn draw(&self, frame: &mut ratatui::Frame, t: &Theme) {
        let area = frame.area();
        let width = area.width.saturating_sub(4).clamp(20, 100);
        let failed = self.rows.iter().filter(|r| r.failed.is_some()).count() as u16;
        let height = (self.rows.len() as u16 + failed + 6).min(area.height);
        let rect = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        )
        .intersection(area);
        let block = crate::panel(Line::from(" Start agents ".fg(t.fg).bold()), t, true);
        let inner = block.inner(rect);
        frame.render_widget(Clear, rect);
        frame.render_widget(block, rect);
        let room = inner.width as usize;
        let mut lines = vec![Line::default()];
        // Rows slide to keep the cursor in sight when there are more than fit.
        let fits = inner.height.saturating_sub(3) as usize;
        let top = self.cursor.saturating_sub(fits.saturating_sub(1));
        for (i, row) in self.rows.iter().enumerate().skip(top).take(fits) {
            lines.push(row_line(row, i == self.cursor, room, t));
            if let Some(why) = &row.failed {
                lines.push(Line::from(
                    crate::fit(&format!("     couldn't start: {why}"), room).fg(t.blocked),
                ));
            }
        }
        lines.push(Line::default());
        lines.push(Line::from(self.summary().fg(t.muted)));
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

/// `› task …   repo · agent · model`, the cursor's on the selection color.
fn row_line(row: &Row, here: bool, room: usize, t: &Theme) -> Line<'static> {
    let d = row.defaults();
    let mut right = vec![
        repo_name(d.repo.as_deref()).fg(t.secondary()),
        " · ".fg(t.muted),
        d.agent.fg(t.secondary()),
    ];
    if !d.model.is_empty() {
        right.extend([" · ".fg(t.muted), d.model.fg(t.secondary())]);
    }
    let right_width: usize = right.iter().map(Span::width).sum();
    let task_room = room.saturating_sub(3 + 2 + right_width).max(8);
    let task = crate::pad(&crate::fit(&row.task, task_room), task_room);
    let mut spans = vec![
        if here {
            " › ".fg(t.accent).bold()
        } else {
            "   ".into()
        },
        task.fg(t.fg),
        "  ".into(),
    ];
    spans.extend(right);
    let used: usize = spans.iter().map(Span::width).sum();
    spans.push(" ".repeat(room.saturating_sub(used)).into());
    let line = Line::from(spans);
    if here {
        line.style(Style::new().bg(t.selection))
    } else {
        line
    }
}

/// The keys the dialog's footer shows (rule 8: a dialog's footer is its own).
pub(crate) fn hints(b: &Batch) -> Vec<(String, &'static str)> {
    vec![
        ("↩".into(), start_label(b.rows.len())),
        ("tab".into(), "repo"),
        ("shift-tab".into(), "agent"),
        ("d".into(), "drop"),
        ("esc".into(), "back"),
    ]
}

fn start_label(n: usize) -> &'static str {
    if n == 1 { "start it" } else { "start them all" }
}

/// A repository whose folder name is a word of `task`: "the web badge" is for
/// `web`. The first named wins.
pub(crate) fn named_repo(task: &str, repos: &[PathBuf]) -> Option<PathBuf> {
    let words: Vec<String> = task
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    repos
        .iter()
        .find(|r| {
            r.file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .is_some_and(|n| words.contains(&n))
        })
        .cloned()
}

/// The repository after `now`: none comes after the last (backlog #292).
fn next_repo(now: Option<&Path>, repos: &[PathBuf]) -> Option<PathBuf> {
    let at = now.and_then(|r| repos.iter().position(|p| p == r));
    repos.get(at.map_or(0, |i| i + 1)).cloned()
}

fn repo_name(repo: Option<&Path>) -> String {
    repo.and_then(Path::file_name)
        .map_or("no repository".to_owned(), |n| {
            n.to_string_lossy().into_owned()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repos() -> Vec<PathBuf> {
        ["/r/api", "/r/web", "/r/infra"].map(PathBuf::from).to_vec()
    }

    fn defaults() -> Defaults {
        Defaults {
            repo: Some("/r/api".into()),
            agent: "claude",
            model: "opus",
        }
    }

    #[test]
    fn a_row_per_line_each_where_it_names_or_the_default() {
        let b = Batch::new(
            "Add token expiry\n\n  Service worker: Allow and Deny on push in web\nCache the infra build\n",
            &defaults(),
            &repos(),
        );
        let got: Vec<(&str, Option<&Path>, &str)> = b
            .rows
            .iter()
            .map(|r| (r.task.as_str(), r.repo.as_deref(), r.defaults().model))
            .collect();
        assert_eq!(
            got,
            [
                ("Add token expiry", Some(Path::new("/r/api")), "opus"),
                (
                    "Service worker: Allow and Deny on push in web",
                    Some(Path::new("/r/web")),
                    "opus"
                ),
                ("Cache the infra build", Some(Path::new("/r/infra")), "opus"),
            ]
        );
        assert_eq!(b.summary(), "3 tasks · api, web, infra");
    }

    #[test]
    fn keys_move_change_and_drop_rows() {
        let mut b = Batch::new("one\ntwo\nthree", &defaults(), &repos());
        assert!(b.keys(b"jj j", &repos()).is_empty());
        assert_eq!(b.cursor, 2, "stops at the last row");
        b.keys(b"k\t", &repos());
        assert_eq!(b.rows[1].repo.as_deref(), Some(Path::new("/r/web")));
        b.keys(b"\t\t", &repos());
        assert_eq!(b.rows[1].repo, None, "no repository after the last");
        b.keys(b"\t\t", &repos());
        assert_eq!(b.rows[1].repo.as_deref(), Some(Path::new("/r/web")));
        b.keys(b"\x1b[Z", &repos());
        assert_eq!(b.rows[1].defaults().agent, "claude");
        assert_eq!(b.rows[1].defaults().model, "sonnet");
        b.keys(b"d", &repos());
        assert_eq!(b.draft(), "one\nthree");
        assert_eq!(b.keys(b"dd", &repos()), [Did::Back], "nothing left: back");
    }

    #[test]
    fn enter_starts_and_esc_goes_back_ending_the_read() {
        let mut b = Batch::new("one\ntwo", &defaults(), &repos());
        assert_eq!(b.keys(b"\rd", &repos()), [Did::Start]);
        assert_eq!(b.rows.len(), 2, "nothing after ↩ acts on the rows");
        assert_eq!(b.keys(b"\x1b", &repos()), [Did::Back]);
        assert!(b.keys(b"\x1bd", &repos()).is_empty(), "alt-d is no drop");
        assert_eq!(b.rows.len(), 2);
    }

    #[test]
    fn a_repository_is_named_by_a_whole_word() {
        assert_eq!(named_repo("fix the webhook", &repos()), None);
        assert_eq!(
            named_repo("API: rate limits", &repos()),
            Some(PathBuf::from("/r/api"))
        );
    }
}

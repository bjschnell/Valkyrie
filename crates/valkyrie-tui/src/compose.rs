//! The home composer (backlog #276): "+ New session" is the Inbox's top row,
//! and on it you describe a task and start an agent on it, file it to the
//! backlog, or `#` find a backlog item to start. Never focused until asked
//! (`n`, `/`, `#` or ↩ on the row), so the single-key grammar stays yours.

use crate::backlog::{self, Lane};
use crate::next_key;
use crate::theme::Theme;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use std::path::{Path, PathBuf};
use valkyrie_proto::{AgentState, Item, ItemStatus, SessionId, SessionInfo, Size, SpawnSpec};

/// What `shift-tab` steps through: an agent, and the model it runs (empty for
/// the agent's own default).
pub(crate) const AGENTS: &[(&str, &str)] = &[
    ("claude", ""),
    ("claude", "opus"),
    ("claude", "sonnet"),
    ("codex", ""),
    ("shell", ""),
];

/// How many `#` matches show under the band.
pub(crate) const MATCHES: usize = 5;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Composer {
    /// Keys go to the draft, not the Inbox.
    pub(crate) typing: bool,
    /// The draft, kept when Esc hands the keys back. Lines are `\n`.
    pub(crate) text: String,
    /// The repository tab picked, `Some(None)` for none; `None` is the guess
    /// (`Defaults`).
    pub(crate) repo: Option<Option<PathBuf>>,
    /// The agent shift-tab picked, an index into `AGENTS`; `None` is the guess.
    pub(crate) agent: Option<usize>,
    /// The `#` match under the cursor.
    pub(crate) pick: usize,
    /// The start of a character a read cut off.
    partial: Vec<u8>,
    /// Inside a bracketed paste: its line breaks are lines of the task, never ↩.
    pasting: bool,
}

/// What a key asked of the composer's owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Composed {
    /// The draft changed, or nothing to do.
    Kept,
    /// ↩: start the task, or the picked `#` item.
    Start,
    /// ^f: file the draft to the backlog.
    File,
    /// tab: the next repository.
    Repo,
    /// shift-tab: the next agent.
    Agent,
    /// Esc: the keys go back to the Inbox, the draft kept.
    Done,
}

/// ^f: files the draft to the backlog. Plain ^F passes through every terminal
/// and multiplexer untouched (unlike ^S/^Q, flow control, or ^B, tmux's prefix).
pub(crate) const FILE_KEY: u8 = 0x06;

impl Composer {
    /// Starts typing, `with` added to the draft (`#` from the Inbox).
    pub(crate) fn focus(&mut self, with: &str) {
        self.typing = true;
        if !with.is_empty() {
            self.text = with.to_owned();
            self.pick = 0;
        }
    }

    /// The keys of one read while typing.
    pub(crate) fn keys(&mut self, bytes: &[u8]) -> Vec<Composed> {
        let mut out = Vec::new();
        let mut rest = bytes;
        while !rest.is_empty() && self.typing {
            // Alt-↩: a new line. Read as one key: `next_key` would split it.
            if let [0x1b, b'\r' | b'\n', ..] = rest {
                self.newline();
                rest = &rest[2..];
                continue;
            }
            let key = next_key(rest);
            let whole = rest.len() == key.len();
            rest = &rest[key.len()..];
            if let Some(did) = self.key(key, whole) {
                out.push(did);
            }
            // Alt and another key: neither is the draft's.
            if key == b"\x1b" && !whole && !rest.is_empty() {
                rest = &rest[next_key(rest).len()..];
            }
        }
        out
    }

    fn key(&mut self, key: &[u8], whole: bool) -> Option<Composed> {
        match key {
            b"\x1b[200~" => self.pasting = true,
            b"\x1b[201~" => self.pasting = false,
            b"\r" | b"\n" if self.pasting => self.newline(),
            b"\t" if self.pasting => self.push(" "),
            b"\r" | b"\n" => return Some(Composed::Start),
            [FILE_KEY] => return Some(Composed::File),
            b"\t" => return Some(Composed::Repo),
            b"\x1b[Z" => return Some(Composed::Agent),
            b"\x1b[A" | b"\x1bOA" => self.pick = self.pick.saturating_sub(1),
            b"\x1b[B" | b"\x1bOB" => self.pick += 1,
            // Esc in a `#` search goes back to typing text; else the keys go back.
            b"\x1b" if whole && self.item_query().is_some() => self.clear(),
            b"\x1b" if whole => {
                self.typing = false;
                self.pasting = false;
                return Some(Composed::Done);
            }
            // ^C drops the draft too.
            [0x03] => {
                self.clear();
                self.typing = false;
                return Some(Composed::Done);
            }
            [0x7f] | [0x08] => {
                self.text.pop();
                self.pick = 0;
            }
            // ^U: the whole draft; ^W: the last word.
            [0x15] => self.clear(),
            [0x17] => {
                let kept = self.text.trim_end().len();
                let cut = self.text[..kept]
                    .rfind(char::is_whitespace)
                    .map_or(0, |i| i + 1);
                self.text.truncate(cut);
                self.pick = 0;
            }
            [0x1b, ..] => {}
            _ => {
                self.partial.extend_from_slice(key);
                match std::str::from_utf8(&self.partial) {
                    Ok(text) => {
                        let text: String = text.chars().filter(|c| !c.is_control()).collect();
                        self.partial.clear();
                        self.push(&text);
                    }
                    // Waiting on the rest of the character.
                    Err(e) if e.error_len().is_none() && self.partial.len() < 4 => {}
                    Err(_) => self.partial.clear(),
                }
            }
        }
        Some(Composed::Kept)
    }

    fn push(&mut self, text: &str) {
        if !text.is_empty() {
            self.text.push_str(text);
            self.pick = 0;
        }
    }

    fn newline(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
    }

    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.partial.clear();
        self.pick = 0;
    }

    /// What's typed after a leading `#`: a backlog search, not a task.
    pub(crate) fn item_query(&self) -> Option<&str> {
        self.text.strip_prefix('#')
    }

    /// The task as typed, without a trailing line break.
    pub(crate) fn task(&self) -> &str {
        self.text.trim()
    }
}

/// The repository, agent and model ↩ starts with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Defaults {
    pub(crate) repo: Option<PathBuf>,
    pub(crate) agent: &'static str,
    pub(crate) model: &'static str,
}

/// The repositories tab steps through: the daemon's list, most recently worked
/// in first, then the backlog's own.
pub(crate) fn repos(known: &[PathBuf], items: &[Item]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for p in known
        .iter()
        .cloned()
        .chain(items.iter().filter_map(|i| i.project.clone()))
    {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// What ↩ starts with: the repository picked, else the one a one-line task
/// names (several lines are named a line each in Start agents), else none
/// (backlog #292); the agent picked, else what that repository's last launch
/// ran (decision #46), else Claude with its own default model.
pub(crate) fn defaults(c: &Composer, repos: &[PathBuf], items: &[Item]) -> Defaults {
    let task = c.task();
    let repo = match &c.repo {
        Some(picked) => picked.clone().filter(|r| repos.contains(r)),
        None if task.contains('\n') => None,
        None => crate::batch::named_repo(task, repos),
    };
    let (agent, model) = match c.agent {
        Some(i) => AGENTS[i % AGENTS.len()],
        None => last_launch(repo.as_deref(), items),
    };
    Defaults { repo, agent, model }
}

/// The newest launch in `repo`, as one of `AGENTS`.
fn last_launch(repo: Option<&Path>, items: &[Item]) -> (&'static str, &'static str) {
    let launch = items
        .iter()
        .filter(|i| repo.is_some() && i.project.as_deref() == repo)
        .filter_map(|i| i.launch.as_ref().map(|l| (i.updated, l)))
        .max_by_key(|(at, _)| *at)
        .map(|(_, l)| l);
    launch
        .and_then(|l| {
            AGENTS
                .iter()
                .find(|(a, m)| *a == l.agent && *m == l.model)
                .or_else(|| AGENTS.iter().find(|(a, _)| *a == l.agent))
        })
        .copied()
        .unwrap_or(AGENTS[0])
}

/// The agent after (or, `back`, before) the one in use.
pub(crate) fn step_agent(d: &Defaults) -> usize {
    let at = AGENTS
        .iter()
        .position(|&(a, m)| a == d.agent && m == d.model)
        .unwrap_or(0);
    (at + 1) % AGENTS.len()
}

/// The repository after the one in use: none comes after the last.
pub(crate) fn step_repo(d: &Defaults, repos: &[PathBuf]) -> Option<PathBuf> {
    let at = d
        .repo
        .as_ref()
        .and_then(|r| repos.iter().position(|p| p == r));
    repos.get(at.map_or(0, |i| i + 1)).cloned()
}

/// The session ↩ starts: the agent in a worktree of its own of the repository,
/// a shell in the repository itself, either in your home folder with no
/// repository (backlog #292).
pub(crate) fn spawn_spec(
    d: &Defaults,
    task: &str,
    size: Size,
    env: Vec<(String, Option<String>)>,
) -> SpawnSpec {
    SpawnSpec {
        command: command(d, task),
        worktree: (d.repo.is_some() && d.agent != "shell").then(String::new),
        cwd: Some(d.repo.clone().unwrap_or_else(valkyrie_proto::home_dir)),
        name: None,
        size,
        env,
        hidden: false,
    }
}

/// The command ↩ runs: the agent with its model and the task as its first
/// message (as a launched item's brief), or the user's shell.
pub(crate) fn command(d: &Defaults, task: &str) -> Vec<String> {
    if d.agent == "shell" {
        return vec![valkyrie_proto::default_shell()];
    }
    let mut command = vec![d.agent.to_owned()];
    if !d.model.is_empty() {
        let flag = if d.agent == "codex" { "-m" } else { "--model" };
        command.extend([flag.to_owned(), d.model.to_owned()]);
    }
    if !task.is_empty() {
        command.push(task.to_owned());
    }
    command
}

/// The draft as a backlog item's title and body: the first line, the rest.
pub(crate) fn as_item(task: &str) -> (String, String) {
    let mut lines = task.lines();
    let title = lines.next().unwrap_or_default().trim().to_owned();
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_owned();
    (title, body)
}

/// A backlog item `#` found, and what ↩ does with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) id: u32,
    pub(crate) label: String,
    pub(crate) title: String,
    pub(crate) what: Can,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Can {
    /// Ready: ↩ starts it.
    Start,
    /// A session has it already: ↩ opens that one rather than a second.
    Open(SessionId, AgentState),
    /// Waits on another item, or is done or in review: shown, greyed out.
    No(String),
}

/// The open backlog items the `#` query names, by number (`12`, `api#12`) or by
/// every word in the title: ready ones first, then the ones with a session, then
/// the greyed ones.
pub(crate) fn find(query: &str, items: &[Item], sessions: &[SessionInfo], boot: u64) -> Vec<Found> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<Found> = items
        .iter()
        .filter(|i| i.status == ItemStatus::Open)
        .filter(|i| {
            let label = label(i).to_lowercase();
            let title = i.title.to_lowercase();
            words.iter().all(|w| {
                let n = w.trim_start_matches('#');
                title.contains(w.as_str())
                    || label == *w
                    || i.number.to_string() == n
                    || label.ends_with(&format!("#{n}"))
            })
        })
        .map(|i| Found {
            id: i.id,
            label: label(i),
            title: i.title.clone(),
            what: can(i, items, sessions, boot),
        })
        .collect();
    found.sort_by_key(|f| match f.what {
        Can::Start => 0,
        Can::Open(..) => 1,
        Can::No(_) => 2,
    });
    found
}

/// What ↩ can do with `item`.
fn can(item: &Item, items: &[Item], sessions: &[SessionInfo], boot: u64) -> Can {
    if let Some(s) = backlog::live(item, sessions, boot) {
        return Can::Open(s.id, s.status.state);
    }
    let waits: Vec<String> = item
        .blocked_by
        .iter()
        .filter_map(|b| items.iter().find(|i| i.id == *b))
        .filter(|b| !matches!(b.status, ItemStatus::Shipped | ItemStatus::Dropped))
        .map(label)
        .collect();
    if let Some(first) = waits.first() {
        return Can::No(format!("after {first}"));
    }
    match backlog::lane(item, sessions, boot) {
        Lane::Backlog if item.project.is_some() => Can::Start,
        Lane::Backlog => Can::No("no folder yet".into()),
        Lane::Review => Can::No("in review".into()),
        _ => Can::No(item.status.as_str().into()),
    }
}

/// The ready items, in the order to work them, for the pane's list and `s`.
pub(crate) fn ready<'a>(items: &'a [Item], sessions: &[SessionInfo], boot: u64) -> Vec<&'a Item> {
    backlog::rows_of(items, items, sessions, boot)
        .into_iter()
        .filter(|(lane, item)| *lane == Lane::Backlog && item.blocked_by.is_empty())
        .filter(|(_, item)| item.project.is_some())
        .filter_map(|(_, row)| items.iter().find(|i| i.id == row.id))
        .collect()
}

/// `api#4`: the repository's name and the item's number in it (decision #73).
pub(crate) fn label(item: &Item) -> String {
    let repo = item
        .project
        .as_deref()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{repo}#{}", item.number)
}

/// `valkyrie · claude · opus`, faint words (rule 4), the one tab changes first.
pub(crate) fn defaults_line(d: &Defaults, t: &Theme) -> Vec<Span<'static>> {
    let repo = d
        .repo
        .as_deref()
        .and_then(Path::file_name)
        .map_or("no repository".to_owned(), |n| {
            n.to_string_lossy().into_owned()
        });
    let mut spans = vec![
        repo.fg(t.secondary()),
        " · ".fg(t.muted),
        d.agent.fg(t.secondary()),
    ];
    if !d.model.is_empty() {
        spans.extend([" · ".fg(t.muted), d.model.fg(t.secondary())]);
    }
    spans
}

/// The band: `› text▏` on the selection color while typing, the placeholder
/// when there's no draft; one line per line of the draft, the last `rows`.
pub(crate) fn band(
    c: &Composer,
    width: usize,
    rows: usize,
    selected: bool,
    t: &Theme,
) -> Vec<Line<'static>> {
    let bg = ratatui::style::Style::new().bg(t.selection);
    let pad = |spans: Vec<Span<'static>>| {
        let used: usize = spans.iter().map(Span::width).sum();
        let mut spans = spans;
        spans.push(" ".repeat(width.saturating_sub(used)).into());
        let line = Line::from(spans);
        if selected || c.typing {
            line.style(bg)
        } else {
            line
        }
    };
    if c.text.is_empty() {
        let mut spans = vec![" › ".fg(t.accent).bold()];
        if c.typing {
            spans.push("▏".fg(t.accent));
        }
        spans.push("describe a task, or # for a backlog item".fg(t.muted));
        return vec![pad(spans)];
    }
    let lines: Vec<&str> = c.text.split('\n').collect();
    let skip = lines.len().saturating_sub(rows.max(1));
    let last = lines.len() - 1;
    lines
        .iter()
        .enumerate()
        .skip(skip)
        .map(|(i, line)| {
            let lead = if i == skip { " › " } else { "   " };
            let room = width.saturating_sub(4);
            let text = tail(line, room);
            let mut spans = vec![lead.fg(t.accent).bold()];
            let hash = i == 0 && line.starts_with('#');
            spans.push(if hash {
                text.fg(t.accent)
            } else {
                text.fg(t.fg)
            });
            if i == last && c.typing {
                spans.push("▏".fg(t.accent));
            }
            pad(spans)
        })
        .collect()
}

/// The end of `s` that fits `room` cells: the cursor end of a long line.
fn tail(s: &str, room: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut width = 0;
    let mut from = chars.len();
    while from > 0 {
        let w = unicode_width::UnicodeWidthChar::width(chars[from - 1]).unwrap_or(0);
        if width + w > room {
            break;
        }
        width += w;
        from -= 1;
    }
    chars[from..].iter().collect()
}

/// The `#` matches as lines under the band, the picked one on the selection
/// color with its key.
pub(crate) fn match_lines(
    found: &[Found],
    pick: usize,
    width: usize,
    t: &Theme,
) -> Vec<Line<'static>> {
    if found.is_empty() {
        return vec![Line::from("     no open item matches".fg(t.muted))];
    }
    let pick = pick.min(found.len() - 1);
    let from = pick.saturating_sub(MATCHES - 1);
    found
        .iter()
        .enumerate()
        .skip(from)
        .take(MATCHES)
        .map(|(i, f)| {
            let (state, key) = match &f.what {
                Can::Start => ("ready".fg(t.muted), "↩ start"),
                Can::Open(_, state) => (
                    format!("● {}", crate::theme::state_word(*state)).fg(t.state(*state)),
                    "↩ opens it",
                ),
                Can::No(why) => (why.clone().fg(t.muted), ""),
            };
            let greyed = matches!(f.what, Can::No(_));
            let title_room = width.saturating_sub(5 + 9 + 2 + 24).max(10);
            let title = crate::pad(&crate::fit(&f.title, title_room), title_room);
            let mut spans = vec![
                "     ".into(),
                crate::pad(&crate::fit(&f.label, 8), 9).fg(t.muted),
                if greyed {
                    title.fg(t.muted)
                } else {
                    title.fg(t.fg)
                },
                "  ".into(),
                state,
            ];
            if i == pick && !key.is_empty() {
                spans.extend(["  ".into(), key.fg(t.accent).bold()]);
            }
            let used: usize = spans.iter().map(Span::width).sum();
            spans.push(" ".repeat(width.saturating_sub(used)).into());
            let line = Line::from(spans);
            if i == pick {
                line.style(ratatui::style::Style::new().bg(t.selection))
            } else {
                line
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typing() -> Composer {
        let mut c = Composer::default();
        c.focus("");
        c
    }

    #[test]
    fn typing_keeps_a_draft_that_esc_hands_back_and_ctrl_c_drops() {
        let mut c = Composer::default();
        assert!(c.keys(b"abc").is_empty(), "not focused: nothing types");
        c.focus("");
        assert_eq!(c.keys(b"fix it\x7f"), vec![Composed::Kept; 7]);
        assert_eq!(c.text, "fix i");
        assert_eq!(c.keys(b"\x1b"), vec![Composed::Done]);
        assert!(!c.typing);
        assert_eq!(c.text, "fix i", "Esc keeps the draft");
        c.focus("");
        assert_eq!(c.keys(b"\x03"), vec![Composed::Done]);
        assert!(!c.typing && c.text.is_empty(), "^C drops it");
    }

    #[test]
    fn enter_starts_ctrl_f_files_and_tab_steps_the_defaults() {
        let mut c = typing();
        assert_eq!(
            c.keys(b"go\r"),
            vec![Composed::Kept, Composed::Kept, Composed::Start]
        );
        assert_eq!(c.keys(&[FILE_KEY]), vec![Composed::File]);
        assert_eq!(c.keys(b"\t\x1b[Z"), vec![Composed::Repo, Composed::Agent]);
        assert_eq!(c.text, "go");
    }

    #[test]
    fn alt_enter_and_a_pasted_list_are_lines_of_one_task() {
        let mut c = typing();
        c.keys(b"one\x1b\rtwo");
        assert_eq!(c.text, "one\ntwo");
        let mut c = typing();
        let did = c.keys(b"\x1b[200~a\r\nb\rc\x1b[201~");
        assert!(
            !did.contains(&Composed::Start),
            "a pasted line never starts it"
        );
        assert_eq!(c.text, "a\nb\nc");
        assert_eq!(c.task(), "a\nb\nc");
        assert_eq!(as_item(c.task()), ("a".into(), "b\nc".into()));
    }

    #[test]
    fn alt_and_a_key_types_nothing() {
        let mut c = typing();
        c.keys(b"\x1bjx");
        assert_eq!(c.text, "x");
    }

    #[test]
    fn esc_in_a_hash_search_goes_back_to_the_text() {
        let mut c = typing();
        c.keys(b"#rate");
        assert_eq!(c.item_query(), Some("rate"));
        c.keys(b"\x1b[B\x1b[B\x1b[A");
        assert_eq!(c.pick, 1);
        assert_eq!(c.keys(b"\x1b"), vec![Composed::Kept]);
        assert!(c.typing && c.text.is_empty());
    }

    #[test]
    fn ctrl_w_drops_the_last_word() {
        let mut c = typing();
        c.keys(b"add a flag \x17");
        assert_eq!(c.text, "add a ");
    }

    #[test]
    fn a_character_split_over_reads_is_joined() {
        let mut c = typing();
        for b in "日本".bytes() {
            c.keys(&[b]);
        }
        assert_eq!(c.text, "日本");
    }

    #[test]
    fn the_command_is_the_agent_its_model_and_the_task() {
        let d = |agent, model| Defaults {
            repo: None,
            agent,
            model,
        };
        assert_eq!(command(&d("claude", ""), "fix"), ["claude", "fix"]);
        assert_eq!(
            command(&d("claude", "opus"), "fix"),
            ["claude", "--model", "opus", "fix"]
        );
        assert_eq!(command(&d("codex", "o3"), ""), ["codex", "-m", "o3"]);
        assert_eq!(command(&d("shell", ""), "x").len(), 1);
    }

    #[test]
    fn tab_and_shift_tab_go_round() {
        let repos = vec![PathBuf::from("/r/a"), PathBuf::from("/r/b")];
        let mut c = Composer::default();
        let d = defaults(&c, &repos, &[]);
        assert_eq!((d.repo.as_deref(), d.agent, d.model), (None, "claude", ""));
        c.repo = Some(step_repo(&d, &repos));
        c.agent = Some(step_agent(&d));
        let d = defaults(&c, &repos, &[]);
        assert_eq!(
            (d.repo.as_deref(), d.agent, d.model),
            (Some(Path::new("/r/a")), "claude", "opus")
        );
        c.repo = Some(step_repo(&d, &repos));
        let d = defaults(&c, &repos, &[]);
        assert_eq!(d.repo.as_deref(), Some(Path::new("/r/b")));
        assert_eq!(step_repo(&d, &repos), None, "then no repository again");
    }

    // Backlog #292: a task that names no repository starts in none.
    #[test]
    fn a_task_starts_in_a_repository_only_when_named_or_picked() {
        let repos = vec![PathBuf::from("/r/api"), PathBuf::from("/r/web")];
        let mut c = typing();
        c.text = "write a haiku".into();
        assert_eq!(defaults(&c, &repos, &[]).repo, None, "nothing named");
        c.text = "fix the web badge".into();
        assert_eq!(
            defaults(&c, &repos, &[]).repo.as_deref(),
            Some(Path::new("/r/web")),
            "named in the task"
        );
        c.repo = Some(None);
        assert_eq!(defaults(&c, &repos, &[]).repo, None, "none picked");
        c.repo = Some(Some("/r/api".into()));
        assert_eq!(
            defaults(&c, &repos, &[]).repo.as_deref(),
            Some(Path::new("/r/api")),
            "picked"
        );
    }

    #[test]
    fn no_repository_starts_at_home_without_a_worktree() {
        let d = |repo: Option<&str>, agent| Defaults {
            repo: repo.map(PathBuf::from),
            agent,
            model: "",
        };
        let size = Size { cols: 80, rows: 24 };
        let s = spawn_spec(&d(None, "claude"), "hi", size, Vec::new());
        assert_eq!(
            (s.cwd, s.worktree),
            (Some(valkyrie_proto::home_dir()), None)
        );
        let s = spawn_spec(&d(Some("/r/api"), "claude"), "hi", size, Vec::new());
        assert_eq!(
            (s.cwd, s.worktree),
            (Some(PathBuf::from("/r/api")), Some(String::new()))
        );
        let s = spawn_spec(&d(Some("/r/api"), "shell"), "", size, Vec::new());
        assert_eq!(s.worktree, None, "a shell starts in the repository itself");
    }
}

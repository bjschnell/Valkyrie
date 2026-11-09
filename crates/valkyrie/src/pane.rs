//! `valk pane`: an agent drives the panes of its tab (DESIGN §8.9), as herdr's
//! `pane` commands do. Each pane is a session, so this is the daemon's `Split`,
//! `Ratio`, `Kill`, `Input`, `Dump` and `Scrollback` behind names an agent can
//! guess. A pane started here never takes the keyboard: focus is the TUI's.

use anyhow::{Context, Result, bail};
use clap::{Subcommand, ValueEnum};
use serde_json::json;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use valkyrie_proto::client::Client;
use valkyrie_proto::layout::{Area, RATIO_SCALE};
use valkyrie_proto::{
    AgentState, Pane, ScrollAnchor, SessionId, SessionInfo, Side, Size, SpawnSpec,
};

#[derive(Subcommand)]
pub enum PaneCmd {
    /// The panes of your tab, or of the tab SESSION is in: id, where it sits, name,
    /// agent, state. `*` marks yours.
    List {
        session: Option<SessionId>,
        #[arg(long)]
        json: bool,
    },
    /// Start a pane beside SESSION (default: yours) running COMMAND, or a shell in
    /// SESSION's directory. Prints the new pane's id. The keyboard stays where it is.
    Split {
        session: Option<SessionId>,
        #[arg(short, long, value_enum, default_value_t = Direction::Right)]
        direction: Direction,
        /// Start in this directory instead of SESSION's.
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Type TEXT into a pane and press Enter. `valk send` sends keys without Enter.
    Run { session: SessionId, text: String },
    /// Print a pane's screen as plain text, or its last N lines, scrollback included.
    Read {
        session: SessionId,
        #[arg(short = 'n', long)]
        lines: Option<u32>,
    },
    /// Wait until TEXT is on a pane's screen, or the pane reaches STATE. Exits 124
    /// when the timeout runs out first. A state that already holds counts, so right
    /// after `run` wait for its output, not for `stopped`.
    #[command(group(clap::ArgGroup::new("until").required(true).args(["text", "state"])))]
    Wait {
        session: SessionId,
        /// Text to look for on the screen.
        #[arg(short, long = "match", value_name = "TEXT")]
        text: Option<String>,
        #[arg(short, long, value_enum)]
        state: Option<WaitState>,
        /// Seconds.
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },
    /// Close a pane, ending what runs in it; its room goes to its neighbour.
    Close { session: SessionId },
    /// Give a pane PERCENT of the split it shares with its neighbour.
    Resize {
        session: SessionId,
        #[arg(value_parser = clap::value_parser!(u16).range(1..=99))]
        percent: u16,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Direction {
    Right,
    Down,
    Left,
    Up,
}

impl From<Direction> for Side {
    fn from(d: Direction) -> Side {
        match d {
            Direction::Right => Side::Right,
            Direction::Down => Side::Down,
            Direction::Left => Side::Left,
            Direction::Up => Side::Up,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum WaitState {
    Working,
    NeedsInput,
    Blocked,
    /// Finished a turn.
    Done,
    Idle,
    Exited,
    /// Anything but working: its turn is over, however it ended.
    Stopped,
}

impl WaitState {
    fn holds(self, s: &SessionInfo) -> bool {
        let state = match s.exited {
            Some(_) => AgentState::Exited,
            None => s.status.state,
        };
        match self {
            WaitState::Working => state == AgentState::Working,
            WaitState::NeedsInput => state == AgentState::NeedsInput,
            WaitState::Blocked => state == AgentState::Blocked,
            WaitState::Done => state == AgentState::ReviewReady,
            WaitState::Idle => state == AgentState::Idle,
            WaitState::Exited => state == AgentState::Exited,
            WaitState::Stopped => !matches!(state, AgentState::Working | AgentState::Stale),
        }
    }
}

/// The session this process runs in, if it runs in one.
fn me() -> Option<SessionId> {
    std::env::var("VALK_SESSION").ok()?.trim().parse().ok()
}

fn mine(session: Option<SessionId>) -> Result<SessionId> {
    session
        .or_else(me)
        .context("not inside a Valkyrie session: name the pane (see `valk pane list ID`)")
}

pub async fn run(client: &Client, cmd: PaneCmd) -> Result<()> {
    match cmd {
        PaneCmd::List { session, json } => {
            let at = mine(session)?;
            let (sessions, layouts) = client.list_tabs().await?;
            let panes = tab(at, &sessions, &layouts)?;
            let me = me();
            if json {
                let out: Vec<_> = panes
                    .iter()
                    .map(|(s, area)| {
                        json!({
                            "id": s.id,
                            "me": Some(s.id) == me,
                            "where": place(*area),
                            "area": {"x": area.x, "y": area.y, "cols": area.cols, "rows": area.rows},
                            "name": s.name,
                            "agent": s.status.agent,
                            "state": state_label(s),
                            "cwd": s.cwd,
                            "command": s.command,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                for (s, area) in &panes {
                    let mark = if Some(s.id) == me { '*' } else { ' ' };
                    println!(
                        "{mark}{:>3}  {:<12} {:<14} {:<7} {}",
                        s.id,
                        place(*area),
                        s.name,
                        s.status.agent,
                        state_label(s)
                    );
                }
            }
            Ok(())
        }
        PaneCmd::Split {
            session,
            direction,
            cwd,
            name,
            command,
        } => {
            let at = mine(session)?;
            let sessions = client.list().await?;
            let target = sessions
                .iter()
                .find(|s| s.id == at)
                .with_context(|| format!("no session {at}"))?;
            let command = match command.is_empty() {
                true => vec![valkyrie_proto::default_shell()],
                false => command,
            };
            let spec = SpawnSpec {
                command,
                cwd: Some(cwd.unwrap_or_else(|| target.cwd.clone())),
                name,
                // Until a TUI attaches it at its real size.
                size: Size { cols: 80, rows: 24 },
                env: valkyrie_proto::login_env(),
                worktree: None,
                hidden: false,
            };
            let info = client.split(at, direction.into(), spec).await?;
            println!("{}", info.id);
            Ok(())
        }
        PaneCmd::Run { session, text } => {
            let mut data = crate::unescape(&text)?;
            data.push(b'\r');
            send(client, session, data).await
        }
        PaneCmd::Read { session, lines } => {
            let text = match lines {
                None => client.dump(session).await?,
                Some(n) => tail(client, session, n).await?,
            };
            print!("{text}");
            Ok(())
        }
        PaneCmd::Wait {
            session,
            text,
            state,
            timeout,
        } => {
            let deadline = Instant::now() + Duration::from_secs(timeout);
            loop {
                let hit = match (&text, state) {
                    (Some(text), _) => client.dump(session).await?.contains(text.as_str()),
                    (None, Some(state)) => {
                        let sessions = client.list().await?;
                        match sessions.iter().find(|s| s.id == session) {
                            Some(s) => state.holds(s),
                            // A pane that ended cleanly (any shell, or exit 0)
                            // leaves the list at once: it has exited.
                            None if matches!(state, WaitState::Exited | WaitState::Stopped) => true,
                            None => bail!("pane {session} has ended (or never was)"),
                        }
                    }
                    (None, None) => unreachable!("clap requires one"),
                };
                if hit {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    eprintln!("valk: timed out after {timeout}s waiting on pane {session}");
                    // As timeout(1) does, so a timeout reads apart from an error.
                    std::process::exit(124);
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        PaneCmd::Close { session } => {
            if me() == Some(session) {
                bail!("pane {session} is the one you run in; close it from the TUI");
            }
            client.kill(session).await
        }
        PaneCmd::Resize { session, percent } => {
            let (_, layouts) = client.list_tabs().await?;
            let (other, first) = layouts
                .iter()
                .find_map(|p| p.neighbour(session))
                .with_context(|| {
                    format!("pane {session} has no neighbour to share a split with")
                })?;
            let share = percent * (RATIO_SCALE / 100);
            match first {
                true => client.ratio(session, other, share).await,
                false => client.ratio(other, session, RATIO_SCALE - share).await,
            }
        }
    }
}

async fn send(client: &Client, session: SessionId, data: Vec<u8>) -> Result<()> {
    // `Input` has no reply, so an unknown pane would pass in silence.
    if !client.list().await?.iter().any(|s| s.id == session) {
        bail!("no session {session}");
    }
    client.input(session, data)?;
    // A round trip, so the input is flushed before exit.
    client.list().await.map(drop)
}

/// The panes of `at`'s tab with where each sits, in thousandths of the tab.
fn tab<'a>(
    at: SessionId,
    sessions: &'a [SessionInfo],
    layouts: &[Pane],
) -> Result<Vec<(&'a SessionInfo, Area)>> {
    let whole = Area {
        x: 0,
        y: 0,
        cols: RATIO_SCALE,
        rows: RATIO_SCALE,
    };
    let placed = match layouts.iter().find(|p| p.contains(at)) {
        Some(p) => p.layout(whole).0,
        None => vec![(at, whole)],
    };
    let panes: Vec<_> = placed
        .into_iter()
        .filter_map(|(id, area)| Some((sessions.iter().find(|s| s.id == id)?, area)))
        .collect();
    if panes.is_empty() {
        bail!("no session {at}");
    }
    Ok(panes)
}

/// Where a pane sits in its tab, in words: `left`, `top-right`, `whole tab`.
fn place(a: Area) -> String {
    fn along(start: u16, len: u16, lo: &'static str, hi: &'static str) -> Option<&'static str> {
        match (start, start + len) {
            (0, RATIO_SCALE) => None,
            (0, _) => Some(lo),
            (_, RATIO_SCALE) => Some(hi),
            _ => Some("middle"),
        }
    }
    match (
        along(a.y, a.rows, "top", "bottom"),
        along(a.x, a.cols, "left", "right"),
    ) {
        (None, None) => "whole tab".into(),
        (Some(v), None) => v.into(),
        (None, Some(h)) => h.into(),
        (Some("middle"), Some("middle")) => "middle".into(),
        (Some(v), Some(h)) => format!("{v}-{h}"),
    }
}

fn state_label(s: &SessionInfo) -> String {
    match s.exited {
        None => s.status.state.label().to_string(),
        Some(Some(code)) => format!("exited {code}"),
        Some(None) => "exited".into(),
    }
}

/// The last `n` lines of a pane, history then screen, trailing blank rows left out.
async fn tail(client: &Client, session: SessionId, n: u32) -> Result<String> {
    let (from_top, history, rows) = client.scrollback(session, ScrollAnchor::Up(0)).await?;
    let height = rows.len() as u32;
    let end = from_top + height;
    // The screen's blank rows below its last line don't count towards `n`.
    let blank = rows
        .iter()
        .rev()
        .take_while(|r| row_text(r).is_empty())
        .count() as u32;
    let mut at = (end - blank).saturating_sub(n).min(history);
    let mut lines: Vec<String> = Vec::new();
    // Pages from the oldest line wanted; the last page is the screen itself.
    while at < end {
        let (top, _, page) = client
            .scrollback(session, ScrollAnchor::FromTop(at))
            .await?;
        let page_lines: Vec<String> = page.iter().map(row_text).collect();
        let skip = (at - top.min(at)) as usize;
        let take = (end - at) as usize;
        let got: Vec<String> = page_lines.into_iter().skip(skip).take(take).collect();
        if got.is_empty() {
            break;
        }
        at += got.len() as u32;
        lines.extend(got);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let start = lines.len().saturating_sub(n as usize);
    Ok(lines[start..].iter().map(|l| format!("{l}\n")).collect())
}

fn row_text(row: &valkyrie_proto::Row) -> String {
    let mut out = String::new();
    let mut col = 0usize;
    for span in &row.spans {
        let x = span.x as usize;
        if x > col {
            out.extend(std::iter::repeat_n(' ', x - col));
            col = x;
        }
        out.push_str(&span.text);
        col += span.text.chars().count();
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(x: u16, y: u16, cols: u16, rows: u16) -> Area {
        Area { x, y, cols, rows }
    }

    #[test]
    fn place_names_where_a_pane_sits() {
        assert_eq!(place(area(0, 0, 1000, 1000)), "whole tab");
        assert_eq!(place(area(0, 0, 499, 1000)), "left");
        assert_eq!(place(area(500, 0, 500, 1000)), "right");
        assert_eq!(place(area(500, 500, 500, 500)), "bottom-right");
        assert_eq!(place(area(0, 0, 1000, 499)), "top");
        assert_eq!(place(area(300, 300, 300, 300)), "middle");
    }

    #[test]
    fn a_tab_of_splits_lists_each_pane_in_layout_order() {
        let mut layout = Pane::leaf(1);
        layout.split(1, 2, Side::Right);
        layout.split(2, 3, Side::Down);
        let sessions: Vec<SessionInfo> = [1, 2, 3, 4]
            .into_iter()
            .map(|id| {
                serde_json::from_value(json!({
                    "id": id, "name": format!("s{id}"), "command": ["sh"], "cwd": "/",
                    "pid": null, "created_unix": 0, "title": null, "clients": 0, "exited": null,
                }))
                .unwrap()
            })
            .collect();
        let layouts = vec![layout];
        let places: Vec<_> = tab(3, &sessions, &layouts)
            .unwrap()
            .into_iter()
            .map(|(s, a)| (s.id, place(a)))
            .collect();
        assert_eq!(
            places,
            [
                (1, "left".into()),
                (2, "top-right".into()),
                (3, "bottom-right".into())
            ]
        );
        // A tab of one pane has no tree.
        let one: Vec<_> = tab(4, &sessions, &layouts).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(place(one[0].1), "whole tab");
        assert!(tab(9, &sessions, &layouts).is_err());
    }

    #[test]
    fn row_text_puts_spans_at_their_columns() {
        let row: valkyrie_proto::Row = serde_json::from_value(json!({
            "y": 0,
            "spans": [{"x": 0, "text": "$ ls"}, {"x": 8, "text": "done  "}],
        }))
        .unwrap();
        assert_eq!(row_text(&row), "$ ls    done");
    }
}

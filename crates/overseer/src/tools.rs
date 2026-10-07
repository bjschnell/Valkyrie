//! Offline tools over recorded sessions (DESIGN §14.6).

use anyhow::{Context, Result};
use overseer_agents::{AgentEvent, Tracker};
use overseer_proto::{AgentState, AgentStatus, Size};
use overseer_term::VtScreen;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// The daemon's tick (screen scans, timers); replay steps time the same way.
const TICK_MS: u64 = 250;

/// Plain text of the screen after feeding the first `upto` bytes of a transcript.
pub fn render(file: &Path, size: Size, upto: Option<usize>) -> Result<String> {
    let data = std::fs::read(file).with_context(|| format!("read {}", file.display()))?;
    let end = upto.unwrap_or(data.len()).min(data.len());
    let mut screen = VtScreen::new(size);
    screen.feed(&data[..end]);
    Ok(screen.text())
}

/// One recorded session re-run through the current adapters and tracker.
pub struct Replay {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Every transition the replay reached.
    pub timeline: Vec<(u64, AgentStatus)>,
    /// Points where the recording says one state and the replay another.
    pub mismatches: Vec<(u64, AgentState, AgentState)>,
}

/// Replays `<stem>.events.jsonl` against `<stem>.raw`. Hooks are re-normalized and
/// screens re-scanned, so adapter changes show up as differences from the recording.
pub fn replay(events: &Path) -> Result<Replay> {
    let name = events.to_string_lossy();
    let stem = name
        .strip_suffix(".events.jsonl")
        .context("expected a <stem>.events.jsonl file")?;
    let raw = std::fs::read(format!("{stem}.raw")).with_context(|| format!("read {stem}.raw"))?;
    let text = std::fs::read_to_string(events)?;

    let mut screen = VtScreen::new(Size { cols: 80, rows: 24 });
    let mut adapter = overseer_agents::by_name("generic");
    let mut tracker = Tracker::new("generic", false, 0);
    let mut fed = 0usize;
    let mut watching = false;
    let mut next_tick = u64::MAX;
    let mut out = Replay {
        start_ms: 0,
        end_ms: 0,
        timeline: Vec::new(),
        mismatches: Vec::new(),
    };
    let record = |tracker: &Tracker, t: u64, out: &mut Replay| {
        let status = tracker.status();
        if out.timeline.last().is_none_or(|(_, s)| s != status) {
            out.timeline.push((t, status.clone()));
        }
    };

    for (n, line) in text.lines().enumerate() {
        let line: Value = serde_json::from_str(line).with_context(|| format!("line {}", n + 1))?;
        let t = line["t"].as_u64().context("missing t")?;
        out.end_ms = t;
        // Strictly before `t`: at the same moment the daemon handles the event first.
        while next_tick < t {
            tracker.tick(next_tick, watching);
            record(&tracker, next_tick, &mut out);
            next_tick += TICK_MS;
        }
        let off = (line["off"].as_u64().unwrap_or(0) as usize).min(raw.len());
        if off > fed {
            screen.feed(&raw[fed..off]);
            fed = off;
        }
        let watch = watching;
        match line["k"].as_str().unwrap_or("") {
            "start" => {
                let size: Size = serde_json::from_value(line["size"].clone())?;
                screen = VtScreen::new(size);
                adapter = overseer_agents::by_name(line["agent"].as_str().unwrap_or(""));
                tracker = Tracker::new(adapter.name(), adapter.hook_gaps(), t);
                out.start_ms = t;
                next_tick = t + TICK_MS;
            }
            "out" => {
                tracker.output(t);
            }
            "bell" => {
                tracker.apply(&AgentEvent::Bell, t, watch);
            }
            "hook" if line["ignored"].as_bool() == Some(true) => {}
            "hook" => {
                let agent = line["agent"].as_str().unwrap_or("");
                let events = overseer_agents::by_name(agent).normalize(&line["payload"]);
                let sent_us = line["sent_us"].as_u64().unwrap_or(0);
                tracker.hook(&events, sent_us, t, watch);
            }
            "scan" => {
                let verdict = adapter.scan(&screen.unwrapped_text());
                tracker.screen(verdict, t, watch);
            }
            "attach" => {
                watching = line["clients"].as_u64().unwrap_or(1) > 0;
                tracker.seen();
            }
            "detach" => watching = line["clients"].as_u64().unwrap_or(0) > 0,
            "seen" => {
                tracker.seen();
            }
            "resize" => screen.resize(serde_json::from_value(line["size"].clone())?),
            "exit" => {
                let code = line["code"].as_i64().map(|c| c as i32);
                tracker.apply(&AgentEvent::Exited { code }, t, watch);
            }
            "state" => {
                let recorded: AgentState = serde_json::from_value(line["status"]["state"].clone())?;
                // Timer transitions happen on the daemon's own tick; catch up first.
                if tracker.status().state != recorded {
                    tracker.tick(t, watching);
                }
                let replayed = tracker.status().state;
                if replayed != recorded {
                    out.mismatches.push((t, recorded, replayed));
                }
            }
            _ => {}
        }
        record(&tracker, t, &mut out);
    }
    Ok(out)
}

/// `(labeled, replayed) -> ms` spent wrong.
pub type Confusion = BTreeMap<(String, String), u64>;

/// The replayed state at each moment.
fn state_at(timeline: &[(u64, AgentStatus)], t: u64) -> Option<AgentState> {
    let i = timeline.partition_point(|(at, _)| *at <= t);
    i.checked_sub(1).map(|i| timeline[i].1.state)
}

/// Hand labels: JSON lines `{"at": <seconds from start>, "state": "needs_input"}`, each
/// holding until the next. A replayed `interrupted` counts as a correct `idle`. Returns the time-weighted share the replay got right, and
/// `(label, replayed) -> ms` for the time it got wrong.
pub fn accuracy(replay: &Replay, labels: &str) -> Result<(f64, Confusion)> {
    let mut points: Vec<(u64, AgentState)> = Vec::new();
    for (n, line) in labels
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let v: Value =
            serde_json::from_str(line).with_context(|| format!("label line {}", n + 1))?;
        let at = v["at"].as_f64().context("label needs \"at\" (seconds)")?;
        let state = serde_json::from_value(v["state"].clone())
            .with_context(|| format!("label line {}: unknown state", n + 1))?;
        points.push((replay.start_ms + (at * 1000.0) as u64, state));
    }
    points.sort_by_key(|p| p.0);
    let (Some(first), Some(_)) = (points.first(), points.last()) else {
        anyhow::bail!("no labels");
    };
    let (mut right, mut total) = (0u64, 0u64);
    let mut wrong = BTreeMap::new();
    let mut t = first.0;
    while t < replay.end_ms {
        let i = points.partition_point(|p| p.0 <= t) - 1;
        let label = points[i].1;
        let got = state_at(&replay.timeline, t);
        total += TICK_MS;
        // `interrupted?` is the tracker's call that the agent went idle without a hook.
        let idle_call = label == AgentState::Idle && got == Some(AgentState::Interrupted);
        if got == Some(label) || idle_call {
            right += TICK_MS;
        } else {
            let got = got.map_or("none", |s| s.label());
            *wrong
                .entry((label.label().to_string(), got.to_string()))
                .or_insert(0) += TICK_MS;
        }
        t += TICK_MS;
    }
    if total == 0 {
        anyhow::bail!("labels start after the recording ends");
    }
    Ok((right as f64 / total as f64, wrong))
}

/// Prints the replayed timeline, drift from the recording and, given labels, accuracy.
pub fn replay_report(events: &Path, labels: Option<&Path>) -> Result<String> {
    use std::fmt::Write;
    let r = replay(events)?;
    let mut s = String::new();
    for (t, status) in &r.timeline {
        let at = t.saturating_sub(r.start_ms) as f64 / 1000.0;
        let ask = status.ask.map(|a| format!(" ({a:?})")).unwrap_or_default();
        let summary = status.summary.as_deref().unwrap_or("");
        let seen = if status.seen { " seen" } else { "" };
        writeln!(
            s,
            "{at:>9.2}s  {:<13}{ask}{seen}  {summary}",
            status.state.label()
        )?;
    }
    writeln!(s, "{} mismatches with the recording", r.mismatches.len())?;
    for (t, recorded, replayed) in &r.mismatches {
        let at = t.saturating_sub(r.start_ms) as f64 / 1000.0;
        writeln!(
            s,
            "  {at:.2}s: recorded {}, replayed {}",
            recorded.label(),
            replayed.label()
        )?;
    }
    if let Some(labels) = labels {
        let (acc, wrong) = accuracy(&r, &std::fs::read_to_string(labels)?)?;
        writeln!(s, "accuracy vs labels: {:.1}%", acc * 100.0)?;
        for ((label, got), ms) in wrong {
            writeln!(
                s,
                "  labeled {label}, replayed {got}: {:.1}s",
                ms as f64 / 1000.0
            )?;
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_session(dir: &Path, raw: &[u8], events: &[Value]) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("1-1.raw"), raw).unwrap();
        let log: String = events.iter().map(|e| format!("{e}\n")).collect();
        let path = dir.join("1-1.events.jsonl");
        std::fs::write(&path, log).unwrap();
        path
    }

    #[test]
    fn replays_hooks_and_screens_and_scores_labels() {
        use serde_json::json;
        let dir = std::env::temp_dir().join(format!("overseer-replay-{}", std::process::id()));
        let trust = b"Quick safety check: Is this a project you created or one you trust?";
        let start = 1_000_000;
        let path = write_session(
            &dir,
            trust,
            &[
                json!({"t": start, "k": "start", "off": 0, "agent": "claude",
                       "size": {"cols": 100, "rows": 10}}),
                json!({"t": start + 200, "k": "scan", "off": trust.len()}),
                json!({"t": start + 200, "k": "state", "off": trust.len(),
                       "status": {"state": "needs_input"}}),
                json!({"t": start + 2000, "k": "hook", "off": trust.len(), "agent": "claude",
                       "sent_us": 1, "payload": {"hook_event_name": "UserPromptSubmit"}}),
                json!({"t": start + 2000, "k": "state", "off": trust.len(),
                       "status": {"state": "working"}}),
                json!({"t": start + 4000, "k": "hook", "off": trust.len(), "agent": "claude",
                       "sent_us": 2, "payload": {"hook_event_name": "Stop"}}),
                // Deliberately wrong, to show up as drift.
                json!({"t": start + 4000, "k": "state", "off": trust.len(),
                       "status": {"state": "idle"}}),
            ],
        );
        let r = replay(&path).unwrap();
        let states: Vec<AgentState> = r.timeline.iter().map(|(_, s)| s.state).collect();
        assert_eq!(
            states,
            [
                AgentState::Idle,
                AgentState::NeedsInput,
                AgentState::Working,
                AgentState::ReviewReady
            ]
        );
        assert_eq!(
            r.mismatches,
            [(start + 4000, AgentState::Idle, AgentState::ReviewReady)]
        );

        // Labels say working from 1s, but the replay shows needs_input until 2s.
        let labels = r#"{"at": 0, "state": "idle"}
{"at": 0.25, "state": "needs_input"}
{"at": 1, "state": "working"}"#;
        let (acc, wrong) = accuracy(&r, labels).unwrap();
        assert!((acc - 0.75).abs() < 0.01, "{acc}");
        assert_eq!(wrong[&("working".into(), "needs input".into())], 1000);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

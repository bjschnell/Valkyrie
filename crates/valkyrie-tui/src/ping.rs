//! Pings: a short sound and a toast when a session you aren't looking at starts
//! needing you (`request`) or finishes (`done`), like herdr's. They mirror the queue
//! (DESIGN §5.4): only a new transition pings, never what was queued before this
//! client connected.
//!
//! The sound plays where the TUI runs. Over SSH that is the remote machine, so there
//! the outer terminal gets a bell instead, which reaches whichever machine you sit at.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use valkyrie_proto::{AgentState, QueueItem, SessionId};

/// Back-to-back transitions make one sound; the toast still names the last one.
const SOUND_GAP: Duration = Duration::from_secs(1);
/// A player still running after this is stuck (herdr's hung for 15 s on a dead
/// audio server) and gets killed.
const PLAYER_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Request,
    Done,
}

impl Kind {
    fn of(state: AgentState) -> Option<Kind> {
        match state {
            AgentState::NeedsInput | AgentState::Blocked => Some(Kind::Request),
            AgentState::ReviewReady => Some(Kind::Done),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Request => "request",
            Kind::Done => "done",
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Ping {
    pub kind: Kind,
    pub session: SessionId,
    /// `api needs input`, `api finished`.
    pub text: String,
    /// Whether to make a sound (false inside the gap after the last one).
    pub sound: bool,
}

#[derive(Default)]
pub struct Pinger {
    /// The transition (`seq`) and state each queued session was last seen at.
    known: HashMap<SessionId, (u64, AgentState)>,
    /// Transitions waiting out `SETTLE` before they ping.
    pending: Vec<Pending>,
    primed: bool,
    last_sound: Option<(Instant, Kind)>,
}

struct Pending {
    item: QueueItem,
    kind: Kind,
    at: Instant,
}

/// A transition must hold this long to ping. Codex reports a permission request
/// before its auto-reviewer takes it (DESIGN §14); the screen moves the session
/// back to working within a scan or two, and that must not ring.
pub const SETTLE: Duration = Duration::from_millis(1500);

impl Pinger {
    /// Forget everything, so the next queue only primes. For a daemon restart, where
    /// ids start over. An upgrade handoff keeps ids and seqs, so it needs no reset.
    pub fn reset(&mut self) {
        self.known.clear();
        self.pending.clear();
        self.primed = false;
    }

    /// Takes a new queue: notes its new transitions, and drops waiting ones that no
    /// longer hold. `viewing` is the attached session, which never pings: you're
    /// looking at it.
    pub fn on_queue(&mut self, items: &[QueueItem], viewing: Option<SessionId>, now: Instant) {
        let primed = std::mem::replace(&mut self.primed, true);
        // Still waiting while the session stays in the state it pinged for; a summary
        // update on the way bumps seq but changes nothing.
        self.pending.retain(|p| {
            items
                .iter()
                .any(|i| i.session == p.item.session && i.status.state == p.item.status.state)
        });
        for item in items {
            let Some(kind) = Kind::of(item.status.state) else {
                continue;
            };
            // A new seq in the same state is a summary update, not a new ask.
            let new = match self.known.get(&item.session) {
                Some(&(seq, state)) => seq != item.status.seq && state != item.status.state,
                None => true,
            };
            if primed && new && !item.status.seen {
                self.pending.retain(|p| p.item.session != item.session);
                self.pending.push(Pending {
                    item: item.clone(),
                    kind,
                    at: now,
                });
            }
        }
        // A pending transition someone has looked at since is old news.
        self.pending
            .retain(|p| Some(p.item.session) != viewing && !Self::seen(items, p));
        self.known = items
            .iter()
            .map(|i| (i.session, (i.status.seq, i.status.state)))
            .collect();
    }

    fn seen(items: &[QueueItem], p: &Pending) -> bool {
        items
            .iter()
            .any(|i| i.session == p.item.session && i.status.seen)
    }

    /// When the next waiting transition is due.
    pub fn next_due(&self) -> Option<Instant> {
        self.pending.iter().map(|p| p.at + SETTLE).min()
    }

    /// The ping for the transitions that held through `SETTLE`: one, a request before
    /// a finish, the queue's order breaking ties.
    pub fn due(&mut self, viewing: Option<SessionId>, now: Instant) -> Option<Ping> {
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|p| now >= p.at + SETTLE);
        self.pending = waiting;
        let best = ready
            .into_iter()
            .filter(|p| Some(p.item.session) != viewing)
            .reduce(|a, b| {
                if b.kind == Kind::Request && a.kind == Kind::Done {
                    b
                } else {
                    a
                }
            })?;
        // A request always sounds unless one just did; a finish waits out any sound.
        let sound = match self.last_sound {
            Some((at, last)) => {
                now.duration_since(at) >= SOUND_GAP
                    || (best.kind == Kind::Request && last == Kind::Done)
            }
            None => true,
        };
        if sound {
            self.last_sound = Some((now, best.kind));
        }
        let what = match best.item.status.state {
            AgentState::NeedsInput => "needs input",
            AgentState::Blocked => "is blocked",
            _ => "finished",
        };
        Some(Ping {
            kind: best.kind,
            session: best.item.session,
            text: format!("{} {what}", best.item.name),
            sound,
        })
    }
}

/// Sounds on or off: `$VALK_SOUND` (`on`/`off`), else the last `m` toggle, else on.
pub fn load_enabled() -> bool {
    let parse = |s: &str| match s.trim() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    };
    std::env::var("VALK_SOUND")
        .ok()
        .and_then(|v| parse(&v))
        .or_else(|| parse(&std::fs::read_to_string(saved_path()).ok()?))
        .unwrap_or(true)
}

/// Remembers the toggle for the next start; failing to is not worth an error.
pub fn save_enabled(on: bool) {
    let path = saved_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, if on { "on" } else { "off" });
}

fn saved_path() -> PathBuf {
    valkyrie_proto::state_dir().join("tui-sound")
}

/// Plays `kind` without blocking: a bell over SSH, else the sound file through the
/// first audio player that works. Failures are silent; a ping is never worth an error.
pub fn play(kind: Kind) {
    if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some() {
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x07").and_then(|()| out.flush());
        return;
    }
    let Some(file) = sound_file(&valkyrie_proto::state_dir().join("sounds"), kind) else {
        return;
    };
    std::thread::spawn(move || {
        // macOS ships afplay; Linux has one of the others.
        for player in ["pw-play", "paplay", "aplay", "afplay"] {
            let mut cmd = Command::new(player);
            if player == "aplay" {
                cmd.arg("-q");
            }
            let Ok(mut child) = cmd
                .arg(&file)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            else {
                continue;
            };
            let deadline = Instant::now() + PLAYER_TIMEOUT;
            let status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(50))
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break None;
                    }
                }
            };
            if status.is_some_and(|s| s.success()) {
                return;
            }
        }
    });
}

/// `<dir>/<kind>.wav`, written with the built-in chime when missing. Replace the
/// file to use your own sound.
fn sound_file(dir: &Path, kind: Kind) -> Option<PathBuf> {
    let path = dir.join(format!("{}.wav", kind.name()));
    if !path.exists() {
        std::fs::create_dir_all(dir).ok()?;
        let tmp = dir.join(format!(".{}.wav.tmp-{}", kind.name(), std::process::id()));
        if std::fs::write(&tmp, chime(kind))
            .and_then(|()| std::fs::rename(&tmp, &path))
            .is_err()
        {
            let _ = std::fs::remove_file(&tmp);
            return None;
        }
    }
    Some(path)
}

const RATE: u32 = 44_100;

/// A soft two-note chime as a 16-bit mono WAV: rising for a request, falling for done.
fn chime(kind: Kind) -> Vec<u8> {
    // E5 and A5.
    let notes: [f32; 2] = match kind {
        Kind::Request => [659.25, 880.0],
        Kind::Done => [880.0, 659.25],
    };
    let note_len = (RATE as f32 * 0.14) as usize;
    let tail = (RATE as f32 * 0.22) as usize;
    let mut samples = Vec::with_capacity(note_len + tail + note_len);
    for (n, freq) in notes.iter().enumerate() {
        let len = if n == 0 { note_len } else { note_len + tail };
        for i in 0..len {
            let t = i as f32 / RATE as f32;
            // 5 ms attack, then an exponential decay; a quiet octave adds a bell tone.
            let env = (t / 0.005).min(1.0) * (-t * 9.0).exp();
            let wave = (std::f32::consts::TAU * freq * t).sin()
                + 0.25 * (std::f32::consts::TAU * 2.0 * freq * t).sin();
            samples.push((wave * env * 0.22 * i16::MAX as f32) as i16);
        }
    }
    wav(&samples)
}

fn wav(samples: &[i16]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use valkyrie_proto::AgentStatus;

    fn item(id: SessionId, state: AgentState, seq: u64) -> QueueItem {
        let mut status = AgentStatus::new("claude", 0);
        status.state = state;
        status.seq = seq;
        status.seen = false;
        QueueItem {
            session: id,
            name: format!("s{id}"),
            cwd: "/".into(),
            status,
        }
    }

    /// Feeds `items`, then reports what is due once `SETTLE` has passed.
    fn settle(
        p: &mut Pinger,
        items: &[QueueItem],
        viewing: Option<SessionId>,
        now: Instant,
    ) -> Option<Ping> {
        p.on_queue(items, viewing, now);
        p.due(viewing, now + SETTLE)
    }

    #[test]
    fn only_new_transitions_ping() {
        let mut p = Pinger::default();
        let t0 = Instant::now();
        // What is queued on connect primes, silently.
        assert_eq!(
            settle(&mut p, &[item(1, AgentState::NeedsInput, 3)], None, t0),
            None
        );
        // The same transition pushed again, or its summary rewritten, stays quiet.
        assert_eq!(
            settle(&mut p, &[item(1, AgentState::NeedsInput, 3)], None, t0),
            None
        );
        assert_eq!(
            settle(&mut p, &[item(1, AgentState::NeedsInput, 4)], None, t0),
            None
        );
        let ping = settle(
            &mut p,
            &[
                item(1, AgentState::NeedsInput, 4),
                item(2, AgentState::ReviewReady, 7),
            ],
            None,
            t0,
        )
        .unwrap();
        assert_eq!((ping.kind, ping.session, ping.sound), (Kind::Done, 2, true));
        assert_eq!(ping.text, "s2 finished");
        // Session 1 escalates to blocked right after: a request still sounds.
        let ping = settle(&mut p, &[item(1, AgentState::Blocked, 5)], None, t0).unwrap();
        assert_eq!((ping.kind, ping.sound), (Kind::Request, true));
        assert_eq!(ping.text, "s1 is blocked");
        // Another finish inside the gap: toast only.
        let ping = settle(&mut p, &[item(9, AgentState::ReviewReady, 1)], None, t0).unwrap();
        assert!(!ping.sound);
        // Interrupted? is a guess, not worth a ping.
        assert_eq!(
            settle(
                &mut p,
                &[item(3, AgentState::Interrupted, 1)],
                None,
                t0 + SOUND_GAP * 2
            ),
            None
        );
    }

    #[test]
    fn a_transition_that_does_not_hold_never_pings() {
        let mut p = Pinger::default();
        let t0 = Instant::now();
        p.on_queue(&[], None, t0);
        // Codex: PermissionRequest queues it, the auto-reviewer takes it at once.
        p.on_queue(&[item(1, AgentState::NeedsInput, 2)], None, t0);
        assert_eq!(p.next_due(), Some(t0 + SETTLE));
        assert_eq!(p.due(None, t0 + SETTLE / 2), None);
        p.on_queue(&[], None, t0 + SETTLE / 2);
        assert_eq!(p.due(None, t0 + SETTLE * 2), None);
        assert_eq!(p.next_due(), None);
        // A summary update while settling keeps the ping.
        p.on_queue(&[item(2, AgentState::ReviewReady, 1)], None, t0);
        p.on_queue(
            &[item(2, AgentState::ReviewReady, 2)],
            None,
            t0 + SETTLE / 2,
        );
        assert_eq!(p.due(None, t0 + SETTLE).unwrap().session, 2);
        // Then the reviewer hands it to the human: that one holds.
        p.on_queue(&[item(1, AgentState::NeedsInput, 4)], None, t0 + SETTLE * 2);
        assert_eq!(p.due(None, t0 + SETTLE * 2), None);
        assert_eq!(p.due(None, t0 + SETTLE * 3).unwrap().session, 1);
    }

    #[test]
    fn the_viewed_and_seen_stay_quiet_and_requests_win() {
        let mut p = Pinger::default();
        let t0 = Instant::now();
        p.on_queue(&[], None, t0);
        assert_eq!(
            settle(&mut p, &[item(1, AgentState::NeedsInput, 1)], Some(1), t0),
            None
        );
        let mut seen = item(2, AgentState::ReviewReady, 1);
        seen.status.seen = true;
        assert_eq!(settle(&mut p, &[seen], None, t0), None);
        // Looked at while settling (attached elsewhere: the daemon marks it seen).
        p.on_queue(&[item(7, AgentState::NeedsInput, 1)], None, t0);
        let mut looked = item(7, AgentState::NeedsInput, 1);
        looked.status.seen = true;
        assert_eq!(settle(&mut p, &[looked], None, t0), None);
        let ping = settle(
            &mut p,
            &[
                item(4, AgentState::ReviewReady, 1),
                item(5, AgentState::NeedsInput, 1),
            ],
            None,
            t0,
        )
        .unwrap();
        assert_eq!((ping.kind, ping.session), (Kind::Request, 5));
        // After a daemon restart the queue primes again instead of re-pinging.
        p.reset();
        assert_eq!(
            settle(
                &mut p,
                &[item(6, AgentState::NeedsInput, 1)],
                None,
                t0 + SOUND_GAP * 2
            ),
            None
        );
    }

    #[test]
    fn chimes_are_valid_wavs() {
        for kind in [Kind::Request, Kind::Done] {
            let bytes = chime(kind);
            assert_eq!(&bytes[..4], b"RIFF");
            assert_eq!(&bytes[8..16], b"WAVEfmt ");
            let data = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
            assert_eq!(bytes.len(), 44 + data);
            assert_eq!(
                u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
                bytes.len() - 8
            );
            let peak = bytes[44..]
                .chunks(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs())
                .max()
                .unwrap();
            assert!(peak > 3000 && peak < i16::MAX as u16 / 2, "peak {peak}");
        }
        assert_ne!(chime(Kind::Request), chime(Kind::Done));
    }

    #[test]
    fn sound_files_are_written_once_and_can_be_replaced() {
        let dir = std::env::temp_dir().join(format!("valkyrie-ping-{}", std::process::id()));
        let path = sound_file(&dir, Kind::Done).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), chime(Kind::Done));
        std::fs::write(&path, b"mine").unwrap();
        assert_eq!(sound_file(&dir, Kind::Done).unwrap(), path);
        assert_eq!(std::fs::read(&path).unwrap(), b"mine");
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}

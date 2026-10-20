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
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use valkyrie_proto::{AgentState, QueueItem, SessionId};

/// Back-to-back transitions make one sound; the toast still names the last one.
const SOUND_GAP: Duration = Duration::from_secs(1);
/// A player still running after this is stuck (herdr's hung for 15 s on a dead
/// audio server) and gets killed.
#[cfg(unix)]
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
    /// longer hold. `viewing` is the attached tab's panes, which never ping: you're
    /// looking at it.
    pub fn on_queue(&mut self, items: &[QueueItem], viewing: &[SessionId], now: Instant) {
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
            .retain(|p| !viewing.contains(&p.item.session) && !Self::seen(items, p));
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
    pub fn due(&mut self, viewing: &[SessionId], now: Instant) -> Option<Ping> {
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|p| now >= p.at + SETTLE);
        self.pending = waiting;
        let best = ready
            .into_iter()
            .filter(|p| !viewing.contains(&p.item.session))
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

/// What a ping sounds like. Each is generated, in two forms (rising and falling,
/// or double and single) so a request and a finish tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Noise {
    /// A soft two-note chime, herdr's.
    Chime,
    /// Bubble pops.
    Pop,
    /// Two knocks on a marimba.
    Wood,
    /// A soft electronic beep.
    Beep,
    /// The rest are 8-bit, square waves like a NES's: a coin picked up.
    Coin,
    /// A power-up's quick arpeggio.
    Powerup,
    /// A laser's falling sweep.
    Zap,
    /// An extra life's jingle.
    OneUp,
}

impl Noise {
    pub const ALL: &[Noise] = &[
        Noise::Chime,
        Noise::Pop,
        Noise::Wood,
        Noise::Beep,
        Noise::Coin,
        Noise::Powerup,
        Noise::Zap,
        Noise::OneUp,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Noise::Chime => "chime",
            Noise::Pop => "pop",
            Noise::Wood => "wood",
            Noise::Beep => "beep",
            Noise::Coin => "coin",
            Noise::Powerup => "powerup",
            Noise::Zap => "zap",
            Noise::OneUp => "1up",
        }
    }
}

/// Plays `kind` in `noise` without blocking: a bell over SSH, else the sound file through the
/// first audio player that works. Failures are silent; a ping is never worth an error.
pub fn play(noise: Noise, kind: Kind) {
    if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some() {
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x07").and_then(|()| out.flush());
        return;
    }
    let Some(file) = sound_file(&valkyrie_proto::state_dir().join("sounds"), noise, kind) else {
        return;
    };
    // Windows plays a WAV itself.
    #[cfg(windows)]
    std::thread::spawn(move || {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_FILENAME, SND_NODEFAULT};
        let wide: Vec<u16> = file.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: a NUL-terminated path; synchronous, on this thread of its own.
        unsafe {
            PlaySoundW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                SND_FILENAME | SND_NODEFAULT,
            )
        };
    });
    #[cfg(unix)]
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

/// `<dir>/<noise>-<kind>.wav`, written with the generated sound when missing; the
/// chime keeps the `<kind>.wav` earlier versions wrote. Replace a file to use your
/// own sound.
fn sound_file(dir: &Path, noise: Noise, kind: Kind) -> Option<PathBuf> {
    let name = match noise {
        Noise::Chime => kind.name().to_string(),
        _ => format!("{}-{}", noise.name(), kind.name()),
    };
    let path = dir.join(format!("{name}.wav"));
    if !path.exists() {
        std::fs::create_dir_all(dir).ok()?;
        let tmp = dir.join(format!(".{name}.wav.tmp-{}", std::process::id()));
        if std::fs::write(&tmp, sound(noise, kind))
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

/// `noise` for `kind` as a 16-bit mono WAV.
fn sound(noise: Noise, kind: Kind) -> Vec<u8> {
    let request = kind == Kind::Request;
    let mut out = Vec::new();
    // Peak level: a beep's square wave sounds louder than a sine at the same peak.
    let level = match noise {
        Noise::Chime => {
            // E5 and A5, rising for a request, falling for done; a quiet octave adds
            // a bell tone.
            let (a, b) = if request {
                (659.25, 880.0)
            } else {
                (880.0, 659.25)
            };
            let partials = [(1.0, 1.0, 9.0), (2.0, 0.25, 9.0)];
            note(&mut out, a, 0.14, &partials);
            note(&mut out, b, 0.36, &partials);
            0.27
        }
        Noise::Pop => {
            // Pitch that rises as it pops; done is one lower pop that falls.
            if request {
                pop(&mut out, 400.0, 900.0, 0.07);
                silence(&mut out, 0.05);
                pop(&mut out, 500.0, 1100.0, 0.09);
            } else {
                pop(&mut out, 900.0, 350.0, 0.12);
            }
            0.3
        }
        Noise::Wood => {
            // C5 and G5; a marimba bar's overtone sits near four times the note.
            let (a, b) = if request {
                (523.25, 783.99)
            } else {
                (783.99, 523.25)
            };
            let partials = [(1.0, 1.0, 22.0), (3.93, 0.3, 40.0)];
            note(&mut out, a, 0.12, &partials);
            note(&mut out, b, 0.3, &partials);
            0.3
        }
        Noise::Beep => {
            // A soft square wave: two short beeps for a request, one longer for done.
            let partials = [(1.0, 1.0, 0.0), (3.0, 0.33, 0.0), (5.0, 0.2, 0.0)];
            if request {
                note(&mut out, 1046.5, 0.08, &partials);
                silence(&mut out, 0.06);
                note(&mut out, 1046.5, 0.08, &partials);
            } else {
                note(&mut out, 784.0, 0.18, &partials);
            }
            0.15
        }
        Noise::Coin => {
            // B5 then a held E6, the game's; done is the same a fifth down.
            let o = if request { 1.0 } else { 2.0 / 3.0 };
            pulse(&mut out, 987.77 * o, 987.77 * o, 0.07, 0.5);
            pulse(&mut out, 1318.51 * o, 1318.51 * o, 0.3, 0.5);
            0.12
        }
        Noise::Powerup => {
            // C E G C, up for a request, down for done.
            let mut notes = [523.25, 659.25, 783.99, 1046.5];
            if !request {
                notes.reverse();
            }
            for freq in notes {
                pulse(&mut out, freq, freq, 0.055, 0.5);
            }
            0.12
        }
        Noise::Zap => {
            // Two quick falls for a request, one long fall for done.
            if request {
                pulse(&mut out, 1000.0, 250.0, 0.1, 0.5);
                pulse(&mut out, 1000.0, 250.0, 0.1, 0.5);
            } else {
                pulse(&mut out, 800.0, 120.0, 0.25, 0.5);
            }
            0.12
        }
        Noise::OneUp => {
            // E G E' C' D' G' for a request, an octave under the game's; done is a
            // short C major fanfare.
            let notes: &[(f32, f32)] = if request {
                &[
                    (659.25, 0.07),
                    (783.99, 0.07),
                    (1318.51, 0.07),
                    (1046.5, 0.07),
                    (1174.66, 0.07),
                    (1567.98, 0.12),
                ]
            } else {
                &[
                    (523.25, 0.07),
                    (659.25, 0.07),
                    (783.99, 0.07),
                    (1046.5, 0.25),
                ]
            };
            for &(freq, secs) in notes {
                pulse(&mut out, freq, freq, secs, 0.5);
            }
            0.1
        }
    };
    let peak = out
        .iter()
        .fold(0.0f32, |m, s| m.max(s.abs()))
        .max(f32::EPSILON);
    let samples: Vec<i16> = out
        .iter()
        .map(|s| (s / peak * level * i16::MAX as f32) as i16)
        .collect();
    wav(&samples)
}

/// How loud a note is `t` seconds into `secs`: a 5 ms attack and release, so it
/// starts and stops without a click.
fn edges(t: f32, secs: f32) -> f32 {
    (t / 0.005).min(1.0) * ((secs - t) / 0.005).clamp(0.0, 1.0)
}

/// A struck note `secs` long: `partials` as (multiple of `freq`, level, decay per
/// second).
fn note(out: &mut Vec<f32>, freq: f32, secs: f32, partials: &[(f32, f32, f32)]) {
    let len = (RATE as f32 * secs) as usize;
    for i in 0..len {
        let t = i as f32 / RATE as f32;
        let wave: f32 = partials
            .iter()
            .map(|&(ratio, level, decay)| {
                level * (-t * decay).exp() * (std::f32::consts::TAU * ratio * freq * t).sin()
            })
            .sum();
        out.push(wave * edges(t, secs));
    }
}

/// A sine gliding from `from` to `to` Hz, quickly at first, as it dies away.
fn pop(out: &mut Vec<f32>, from: f32, to: f32, secs: f32) {
    let len = (RATE as f32 * secs) as usize;
    let mut phase = 0.0f32;
    for i in 0..len {
        let t = i as f32 / RATE as f32;
        let freq = to + (from - to) * (-t * 30.0).exp();
        phase += std::f32::consts::TAU * freq / RATE as f32;
        out.push(phase.sin() * (-t * 40.0).exp() * edges(t, secs));
    }
}

/// A pulse wave sliding from `from` to `to` Hz, high for `duty` of each cycle. A
/// raw one pierces, so a low-pass takes the edge off its top harmonics and the note
/// fades a little as it holds.
fn pulse(out: &mut Vec<f32>, from: f32, to: f32, secs: f32, duty: f32) {
    // One pole at 2.2 kHz.
    let smooth = 1.0 - (-std::f32::consts::TAU * 2200.0 / RATE as f32).exp();
    let len = (RATE as f32 * secs) as usize;
    let (mut phase, mut level) = (0.0f32, 0.0f32);
    for i in 0..len {
        let t = i as f32 / RATE as f32;
        phase = (phase + (from + (to - from) * t / secs) / RATE as f32).fract();
        let wave = if phase < duty { 1.0 } else { -1.0 };
        level += smooth * (wave - level);
        out.push(level * (-t * 3.0).exp() * edges(t, secs));
    }
}

fn silence(out: &mut Vec<f32>, secs: f32) {
    out.extend(std::iter::repeat_n(0.0, (RATE as f32 * secs) as usize));
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
        p.on_queue(items, viewing.as_slice(), now);
        p.due(viewing.as_slice(), now + SETTLE)
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
        p.on_queue(&[], &[], t0);
        // Codex: PermissionRequest queues it, the auto-reviewer takes it at once.
        p.on_queue(&[item(1, AgentState::NeedsInput, 2)], &[], t0);
        assert_eq!(p.next_due(), Some(t0 + SETTLE));
        assert_eq!(p.due(&[], t0 + SETTLE / 2), None);
        p.on_queue(&[], &[], t0 + SETTLE / 2);
        assert_eq!(p.due(&[], t0 + SETTLE * 2), None);
        assert_eq!(p.next_due(), None);
        // A summary update while settling keeps the ping.
        p.on_queue(&[item(2, AgentState::ReviewReady, 1)], &[], t0);
        p.on_queue(&[item(2, AgentState::ReviewReady, 2)], &[], t0 + SETTLE / 2);
        assert_eq!(p.due(&[], t0 + SETTLE).unwrap().session, 2);
        // Then the reviewer hands it to the human: that one holds.
        p.on_queue(&[item(1, AgentState::NeedsInput, 4)], &[], t0 + SETTLE * 2);
        assert_eq!(p.due(&[], t0 + SETTLE * 2), None);
        assert_eq!(p.due(&[], t0 + SETTLE * 3).unwrap().session, 1);
    }

    #[test]
    fn the_viewed_and_seen_stay_quiet_and_requests_win() {
        let mut p = Pinger::default();
        let t0 = Instant::now();
        p.on_queue(&[], &[], t0);
        assert_eq!(
            settle(&mut p, &[item(1, AgentState::NeedsInput, 1)], Some(1), t0),
            None
        );
        let mut seen = item(2, AgentState::ReviewReady, 1);
        seen.status.seen = true;
        assert_eq!(settle(&mut p, &[seen], None, t0), None);
        // Looked at while settling (attached elsewhere: the daemon marks it seen).
        p.on_queue(&[item(7, AgentState::NeedsInput, 1)], &[], t0);
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
    fn sounds_are_valid_wavs() {
        let mut all = Vec::new();
        for &noise in Noise::ALL {
            for kind in [Kind::Request, Kind::Done] {
                let bytes = sound(noise, kind);
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
                assert!(
                    peak > 3000 && peak < i16::MAX as u16 / 2,
                    "{noise:?} peak {peak}"
                );
                // Under a second, so pings don't pile up.
                assert!(data < RATE as usize * 2, "{noise:?} {kind:?}");
                all.push(bytes);
            }
        }
        // Every noise and kind sounds different.
        for (i, a) in all.iter().enumerate() {
            assert!(all[i + 1..].iter().all(|b| a != b));
        }
    }

    #[test]
    fn sound_files_are_written_once_and_can_be_replaced() {
        let dir = std::env::temp_dir().join(format!("valkyrie-ping-{}", std::process::id()));
        let path = sound_file(&dir, Noise::Chime, Kind::Done).unwrap();
        // The chime keeps the name earlier versions wrote, so a replaced one stays.
        assert_eq!(path, dir.join("done.wav"));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            sound(Noise::Chime, Kind::Done)
        );
        std::fs::write(&path, b"mine").unwrap();
        assert_eq!(sound_file(&dir, Noise::Chime, Kind::Done).unwrap(), path);
        assert_eq!(std::fs::read(&path).unwrap(), b"mine");
        let coin = sound_file(&dir, Noise::Coin, Kind::Request).unwrap();
        assert_eq!(coin, dir.join("coin-request.wav"));
        assert_eq!(
            std::fs::read(&coin).unwrap(),
            sound(Noise::Coin, Kind::Request)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

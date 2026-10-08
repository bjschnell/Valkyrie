//! Images (DESIGN §8.4). Programs draw them with the Kitty graphics protocol: APC
//! commands (`ESC _ G <control> ; <payload> ESC \`) that `alacritty_terminal` drops.
//! [`Scanner`] takes them out of the output stream before the VT parser sees it;
//! [`Graphics`] answers the program and decides what clients get, whose own terminal
//! draws the images; [`Log`] keeps what a client attaching later must replay.
//!
//! What reaches a client's terminal is checked first: only well-formed commands, with
//! direct (in-band) data, asking for no reply. Anything else could make the user's
//! real terminal do what the program chose (run an escape sequence, read a file).
//!
//! Placement by Unicode placeholders (`U=1`) needs nothing more: the placeholder
//! cells are ordinary text on the screen, and the client's terminal draws the image
//! into them.

use std::collections::VecDeque;
use std::sync::Arc;

/// The most image data in one transmission. Larger ones are refused: they must fit
/// one protocol frame (16 MiB) to reach a client.
const MAX_TRANSMISSION: usize = 12 << 20;

/// Fed to the VT where an APC is cut out. A real terminal's parser is reset by the
/// APC's ESC; `ESC \` (a bare ST, which it ignores) does the same for ours, so a
/// sequence the program left half-written before the APC cannot swallow what follows.
const RESET: &[u8] = b"\x1b\\";

/// One piece of program output, in order.
#[derive(Debug, PartialEq)]
pub enum Piece<'a> {
    /// Bytes for the VT parser.
    Text(&'a [u8]),
    /// A well-formed graphics command, `ESC _ G ... ESC \` included.
    Graphics(Vec<u8>),
    /// `CSI 16 t`, a cell size query the VT parser does not answer.
    CellSizeQuery,
}

/// Splits output into text and graphics commands, across reads.
///
/// An APC ends where the VT parser would end it: at `ESC \`, or at any other ESC
/// (which then starts a new sequence), and CAN or SUB abort it. So a stray `ESC _`
/// in binary output costs one sequence, not the rest of the session.
#[derive(Default)]
pub struct Scanner {
    state: State,
    /// The APC so far (without `ESC _`), when it is a graphics command.
    apc: Vec<u8>,
    /// Keeping this APC: it is a graphics command of sane length.
    keep: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum State {
    #[default]
    Text,
    /// An ESC ended the last read: an APC may start with the next byte.
    Esc,
    /// Just after `ESC _`: the next byte says whether it is a graphics command.
    ApcStart,
    Apc,
    /// ESC inside an APC: `\` ends it, anything else aborts it.
    ApcEsc,
}

impl Scanner {
    pub fn split<'a>(&mut self, bytes: &'a [u8]) -> Vec<Piece<'a>> {
        let mut out = Vec::new();
        let mut i = 0;
        if self.state == State::Esc {
            match bytes.first() {
                None => return out,
                Some(b'_') => {
                    out.push(Piece::Text(RESET));
                    self.state = State::ApcStart;
                    i = 1;
                }
                Some(_) if bytes.starts_with(b"[16t") => {
                    self.state = State::Text;
                    out.push(Piece::Text(b"\x1b"));
                    out.push(Piece::Text(&bytes[..4]));
                    out.push(Piece::CellSizeQuery);
                    i = 4;
                }
                Some(_) => {
                    self.state = State::Text;
                    out.push(Piece::Text(b"\x1b"));
                }
            }
        }
        let mut text_from = i;
        while i < bytes.len() {
            match self.state {
                State::Text | State::Esc => {
                    let Some(at) = memchr::memchr(0x1b, &bytes[i..]) else {
                        break;
                    };
                    i += at;
                    match bytes.get(i + 1) {
                        // Hold the ESC: the next read says whether an APC starts.
                        None => {
                            if text_from < i {
                                out.push(Piece::Text(&bytes[text_from..i]));
                            }
                            self.state = State::Esc;
                            return out;
                        }
                        Some(b'_') => {
                            if text_from < i {
                                out.push(Piece::Text(&bytes[text_from..i]));
                            }
                            out.push(Piece::Text(RESET));
                            self.state = State::ApcStart;
                            i += 2;
                            text_from = i;
                        }
                        Some(_) if bytes[i..].starts_with(b"\x1b[16t") => {
                            out.push(Piece::Text(&bytes[text_from..i + 5]));
                            out.push(Piece::CellSizeQuery);
                            i += 5;
                            text_from = i;
                        }
                        Some(_) => i += 1,
                    }
                }
                State::ApcStart => {
                    self.apc.clear();
                    self.keep = bytes[i] == b'G';
                    self.state = State::Apc;
                }
                State::Apc => {
                    let rest = &bytes[i..];
                    let end = memchr::memchr3(0x1b, 0x18, 0x1a, rest).unwrap_or(rest.len());
                    if self.keep {
                        if self.apc.len() + end <= MAX_TRANSMISSION {
                            self.apc.extend_from_slice(&rest[..end]);
                        } else {
                            self.keep = false;
                        }
                    }
                    i += end;
                    match bytes.get(i) {
                        Some(0x1b) => {
                            self.state = State::ApcEsc;
                            i += 1;
                        }
                        // CAN or SUB: the sequence is abandoned (the byte with it).
                        Some(_) => {
                            self.state = State::Text;
                            i += 1;
                        }
                        None => {}
                    }
                    text_from = i;
                }
                State::ApcEsc => {
                    self.state = State::Text;
                    if bytes[i] == b'\\' {
                        i += 1;
                        text_from = i;
                        let apc = std::mem::take(&mut self.apc);
                        if self.keep && well_formed(&apc) {
                            let mut cmd = Vec::with_capacity(apc.len() + 4);
                            cmd.extend_from_slice(b"\x1b_");
                            cmd.extend_from_slice(&apc);
                            cmd.extend_from_slice(b"\x1b\\");
                            out.push(Piece::Graphics(cmd));
                        }
                    } else {
                        // The APC is dropped and the ESC starts whatever comes next
                        // (another APC, `CSI 16 t`…): back up to it when it is in
                        // this read.
                        self.apc.clear();
                        if i > 0 {
                            i -= 1;
                        } else {
                            out.push(Piece::Text(b"\x1b"));
                        }
                        text_from = i;
                    }
                }
            }
        }
        if matches!(self.state, State::Text) && text_from < bytes.len() {
            out.push(Piece::Text(&bytes[text_from..]));
        }
        out
    }
}

/// `G`, control keys (`a=T,i=3`), then optionally `;` and base64. Nothing else may
/// reach a client's terminal inside the command.
fn well_formed(apc: &[u8]) -> bool {
    let Some(body) = apc.strip_prefix(b"G") else {
        return false;
    };
    let (control, payload) = match memchr::memchr(b';', body) {
        Some(at) => (&body[..at], &body[at + 1..]),
        None => (body, &b""[..]),
    };
    control
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'=' | b',' | b'-'))
        && payload
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}

/// A graphics command's control keys (`a=T,i=3,...`), parsed for routing only.
#[derive(Debug, Default, PartialEq)]
pub struct Control {
    /// `a`; 0 for a continuation chunk, which has no action of its own.
    pub action: u8,
    pub id: Option<u32>,
    pub number: Option<u32>,
    pub placement: Option<u32>,
    pub quiet: u8,
    pub medium: u8,
    pub more: bool,
    pub delete: u8,
}

impl Control {
    pub fn parse(cmd: &[u8]) -> Control {
        let body = cmd.strip_prefix(b"\x1b_G").unwrap_or(cmd);
        let end = body
            .iter()
            .position(|&b| b == b';' || b == 0x1b)
            .unwrap_or(body.len());
        // Kitty's defaults: transmit, direct, and a bare delete deletes all.
        let mut c = Control {
            action: b't',
            medium: b'd',
            delete: b'a',
            ..Control::default()
        };
        let mut has_action = false;
        for pair in body[..end].split(|&b| b == b',') {
            let Some((&key, value)) = pair.split_first() else {
                continue;
            };
            let value = value.strip_prefix(b"=").unwrap_or_default();
            let num = || std::str::from_utf8(value).ok()?.parse::<u32>().ok();
            match key {
                b'a' => {
                    has_action = true;
                    c.action = value.first().copied().unwrap_or(b't');
                }
                b'i' => c.id = num(),
                b'I' => c.number = num(),
                b'p' => c.placement = num(),
                b'q' => c.quiet = num().unwrap_or(0) as u8,
                b't' => c.medium = value.first().copied().unwrap_or(b'd'),
                b'm' => c.more = num() == Some(1),
                b'd' => c.delete = value.first().copied().unwrap_or(b'a'),
                _ => {}
            }
        }
        if !has_action && c.id.is_none() && c.number.is_none() {
            c.action = 0;
        }
        c
    }

    /// The reply a terminal gives this command, if the program asked for one: only
    /// when it named the image by `i` (an `I` number needs the terminal's own id).
    fn reply(&self, ok: bool, message: &str) -> Option<Vec<u8>> {
        let id = self.id?;
        if self.quiet >= 2 || (ok && self.quiet == 1) {
            return None;
        }
        let placement = self
            .placement
            .map(|p| format!(",p={p}"))
            .unwrap_or_default();
        Some(format!("\x1b_Gi={id}{placement};{message}\x1b\\").into_bytes())
    }
}

/// A copy of `cmd` (one APC) that asks the terminal for no response: the daemon
/// answers the program itself, once, however many clients are attached.
pub fn quiet(cmd: &[u8]) -> Vec<u8> {
    let Some(body) = cmd.strip_suffix(b"\x1b\\") else {
        return cmd.to_vec();
    };
    let end = body.iter().position(|&b| b == b';').unwrap_or(body.len());
    // The last key wins, so append rather than prepend.
    let mut out = Vec::with_capacity(cmd.len() + 4);
    out.extend_from_slice(&body[..end]);
    if end > 3 {
        out.push(b',');
    }
    out.extend_from_slice(b"q=2");
    out.extend_from_slice(&body[end..]);
    out.extend_from_slice(b"\x1b\\");
    out
}

/// A graphics command for clients (a whole transmission's chunks joined) and where
/// the cursor was when the program sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub data: Vec<u8>,
    pub x: u16,
    pub y: u16,
}

/// What a graphics command from the program leads to.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Bytes for the program, as its terminal would have answered.
    Reply(Vec<u8>),
    /// For clients' terminals.
    Forward(Command),
}

/// Joins chunked transmissions, answers the program, and filters what clients get.
#[derive(Default)]
pub struct Graphics {
    open: Option<Open>,
}

struct Open {
    first: Control,
    /// The quieted chunks so far.
    data: Vec<u8>,
    too_big: bool,
    x: u16,
    y: u16,
}

impl Graphics {
    /// `cmd` is one well-formed APC; (`x`, `y`) the cursor cell when it came.
    pub fn handle(&mut self, cmd: &[u8], x: u16, y: u16) -> Vec<Outcome> {
        let c = Control::parse(cmd);
        if c.action == 0 {
            // A continuation chunk; without a transmission open, an orphan.
            let Some(open) = &mut self.open else {
                return Vec::new();
            };
            if open.data.len() + cmd.len() > MAX_TRANSMISSION {
                open.too_big = true;
            } else if !open.too_big {
                open.data.extend_from_slice(&quiet(cmd));
            }
            if c.more {
                return Vec::new();
            }
            let open = self.open.take().expect("open transmission");
            return finish(open);
        }
        // A new command abandons a transmission left unfinished.
        self.open = None;
        let open = Open {
            data: quiet(cmd),
            first: c,
            too_big: false,
            x,
            y,
        };
        if open.first.more {
            self.open = Some(open);
            return Vec::new();
        }
        finish(open)
    }
}

fn finish(open: Open) -> Vec<Outcome> {
    let c = &open.first;
    let mut out = Vec::new();
    // Only in-band data: a file, temp file or shared memory would be read on the
    // client's machine, by a path the program chose.
    let refusal = if c.medium != b'd' {
        Some("EINVAL:only direct transmission is supported")
    } else if open.too_big {
        Some("EFBIG:image too large")
    } else {
        None
    };
    if let Some(why) = refusal {
        out.extend(c.reply(false, why).map(Outcome::Reply));
        return out;
    }
    if c.action == b'q' {
        out.extend(c.reply(true, "OK").map(Outcome::Reply));
        return out;
    }
    if matches!(c.action, b't' | b'T' | b'p') {
        out.extend(c.reply(true, "OK").map(Outcome::Reply));
    }
    out.push(Outcome::Forward(Command {
        data: open.data,
        x: open.x,
        y: open.y,
    }));
    out
}

/// One kept command: shared, so replaying it under the session lock costs nothing.
#[derive(Debug, Clone)]
pub struct Kept {
    pub data: Arc<str>,
    pub x: u16,
    pub y: u16,
}

struct Entry {
    image: Option<u32>,
    action: u8,
    kept: Kept,
}

/// The commands a client attaching now must replay to show what the program drew:
/// transmissions and placements, minus what was deleted since. Bounded in size;
/// whole commands are evicted, oldest first.
pub struct Log {
    entries: VecDeque<Entry>,
    bytes: usize,
    limit: usize,
}

impl Log {
    pub fn new(limit: usize) -> Log {
        Log {
            entries: VecDeque::new(),
            bytes: 0,
            limit,
        }
    }

    /// Records a command `Graphics` forwarded (already quiet), returning it shared.
    pub fn record(&mut self, cmd: Command) -> Kept {
        let c = Control::parse(&cmd.data);
        let image = c.id.or(c.number);
        let kept = Kept {
            data: String::from_utf8_lossy(&cmd.data).into(),
            x: cmd.x,
            y: cmd.y,
        };
        match c.action {
            b'd' => {
                match c.delete {
                    // Uppercase frees the image data too; lowercase only placements.
                    b'A' => self.retain(|_| false),
                    b'a' => self.unplace(|_| true),
                    b'I' | b'N' if image.is_some() => self.retain(|e| e.image != image),
                    b'i' | b'n' if image.is_some() => self.unplace(|e| e.image == image),
                    // By position and the like: replay it in order.
                    _ => self.push(None, c.action, kept.clone()),
                }
                return kept;
            }
            // A new transmission replaces the image's data and placements.
            b't' | b'T' if image.is_some() => self.retain(|e| e.image != image),
            _ => {}
        }
        self.push(image, c.action, kept.clone());
        kept
    }

    fn push(&mut self, image: Option<u32>, action: u8, kept: Kept) {
        self.bytes += kept.data.len();
        self.entries.push_back(Entry {
            image,
            action,
            kept,
        });
        while self.bytes > self.limit
            && let Some(old) = self.entries.pop_front()
        {
            self.bytes -= old.kept.data.len();
        }
    }

    fn retain(&mut self, keep: impl Fn(&Entry) -> bool) {
        let bytes = &mut self.bytes;
        self.entries.retain(|e| {
            let kept = keep(e);
            if !kept {
                *bytes -= e.kept.data.len();
            }
            kept
        });
    }

    /// Takes the placements of the images `which` matches, keeping their data: a
    /// transmit-and-place (`a=T`) becomes a plain transmit (`a=t`).
    fn unplace(&mut self, which: impl Fn(&Entry) -> bool) {
        self.retain(|e| !(e.action == b'p' && which(e)));
        for e in self.entries.iter_mut() {
            if e.action == b'T' && which(e) {
                e.action = b't';
                e.kept.data = e.kept.data.replacen("a=T", "a=t", 1).into();
            }
        }
    }

    /// What to replay, in order.
    pub fn replay(&self) -> Vec<Kept> {
        self.entries.iter().map(|e| e.kept.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(s: &mut Scanner, input: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut text = Vec::new();
        let mut graphics = Vec::new();
        for p in s.split(input) {
            match p {
                // The parser reset at each cut is checked in `VtScreen`'s tests.
                Piece::Text(t) if t == RESET => {}
                Piece::Text(t) => text.extend_from_slice(t),
                Piece::Graphics(g) => graphics.push(g),
                Piece::CellSizeQuery => {}
            }
        }
        (text, graphics)
    }

    #[test]
    fn graphics_are_cut_out_of_the_text() {
        let mut s = Scanner::default();
        let (text, graphics) = pieces(
            &mut s,
            b"ab\x1b_Ga=T,i=1;AAAA\x1b\\cd\x1b[1mx\x1b_Xother\x1b\\y",
        );
        assert_eq!(text, b"abcd\x1b[1mxy");
        assert_eq!(graphics, [b"\x1b_Ga=T,i=1;AAAA\x1b\\".to_vec()]);
    }

    #[test]
    fn a_command_split_across_reads_comes_out_whole() {
        let mut s = Scanner::default();
        let input = b"hi\x1b_Ga=T,i=7;QUJD\x1b\\there\x1b";
        let mut text = Vec::new();
        let mut graphics = Vec::new();
        // Every split point, including right after the ESC of `ESC _` and `ESC \`.
        for chunk in input.chunks(1) {
            let (t, g) = pieces(&mut s, chunk);
            text.extend(t);
            graphics.extend(g);
        }
        assert_eq!(graphics, [b"\x1b_Ga=T,i=7;QUJD\x1b\\".to_vec()]);
        // The trailing ESC is held until the next read decides what it is.
        assert_eq!(text, b"hithere");
        assert_eq!(pieces(&mut s, b"[0m").0, b"\x1b[0m");
    }

    #[test]
    fn nothing_but_a_well_formed_command_gets_through() {
        let mut s = Scanner::default();
        // An escape sequence smuggled inside: the APC ends at its ESC, which goes to
        // the VT as text; nothing reaches a client.
        let (text, graphics) = pieces(&mut s, b"\x1b_Ga=p;\x1b]52;c;aGk=\x07\x1b\\ok");
        assert!(graphics.is_empty());
        assert_eq!(text, b"\x1b]52;c;aGk=\x07\x1b\\ok");
        // Bytes outside the base64 alphabet, and CAN aborting one.
        let (_, graphics) = pieces(&mut s, b"\x1b_Ga=T;AA\x07AA\x1b\\");
        assert!(graphics.is_empty());
        let (text, graphics) = pieces(&mut s, b"\x1b_Ga=T;AA\x18after");
        assert!(graphics.is_empty());
        assert_eq!(text, b"after");
        // A good command right after an aborted one still counts.
        let (_, graphics) = pieces(&mut s, b"\x1b_Gbad\x1b_Ga=t,i=3;AAAA\x1b\\");
        assert_eq!(graphics, [b"\x1b_Ga=t,i=3;AAAA\x1b\\".to_vec()]);
    }

    #[test]
    fn a_stray_apc_start_does_not_swallow_the_session() {
        let mut s = Scanner::default();
        // Binary output with an `ESC _` and no terminator: the next ESC ends it.
        let (text, _) = pieces(&mut s, b"\x01\x1b_\xff\xfe junk");
        assert_eq!(text, b"\x01");
        let (text, _) = pieces(&mut s, b"more\x1b[0mprompt$ ");
        assert_eq!(text, b"\x1b[0mprompt$ ");
    }

    #[test]
    fn finds_cell_size_queries() {
        let mut s = Scanner::default();
        // After an ESC held over from the last read, too.
        assert_eq!(s.split(b"a\x1b"), [Piece::Text(b"a")]);
        assert_eq!(
            s.split(b"[16tb"),
            [
                Piece::Text(b"\x1b"),
                Piece::Text(b"[16t"),
                Piece::CellSizeQuery,
                Piece::Text(b"b")
            ]
        );
        assert_eq!(
            s.split(b"x\x1b[16ty"),
            [
                Piece::Text(b"x\x1b[16t"),
                Piece::CellSizeQuery,
                Piece::Text(b"y")
            ]
        );
    }

    fn handle(g: &mut Graphics, cmd: &str) -> Vec<Outcome> {
        g.handle(format!("\x1b_G{cmd}\x1b\\").as_bytes(), 1, 2)
    }

    fn reply(s: &str) -> Outcome {
        Outcome::Reply(s.as_bytes().to_vec())
    }

    #[test]
    fn the_daemon_answers_and_clients_stay_quiet() {
        let mut g = Graphics::default();
        // Queries: answered here, never forwarded.
        assert_eq!(
            handle(&mut g, "i=31,s=1,v=1,a=q,t=d,f=24;AAAA"),
            [reply("\x1b_Gi=31;OK\x1b\\")]
        );
        let shm = handle(&mut g, "i=9,a=q,t=s;AAAA");
        assert!(matches!(&shm[..], [Outcome::Reply(r)] if r.starts_with(b"\x1b_Gi=9;EINVAL")));
        assert!(handle(&mut g, "i=1,a=q,q=2;AAAA").is_empty());
        // A file transmission is refused, not forwarded.
        let file = handle(&mut g, "a=T,t=f,i=5;L2V0Yy9wYXNzd2Q=");
        assert!(matches!(&file[..], [Outcome::Reply(r)] if r.starts_with(b"\x1b_Gi=5;EINVAL")));
        // A transmission: the program gets OK, clients a quiet copy.
        assert_eq!(
            handle(&mut g, "a=T,i=4;AAAA"),
            [
                reply("\x1b_Gi=4;OK\x1b\\"),
                Outcome::Forward(Command {
                    data: b"\x1b_Ga=T,i=4,q=2;AAAA\x1b\\".to_vec(),
                    x: 1,
                    y: 2
                })
            ]
        );
    }

    #[test]
    fn chunks_travel_as_one_command() {
        let mut g = Graphics::default();
        assert!(handle(&mut g, "a=T,i=1,q=2,m=1;AAAA").is_empty());
        assert!(handle(&mut g, "m=1;BBBB").is_empty());
        assert_eq!(
            handle(&mut g, "m=0;CCCC"),
            [Outcome::Forward(Command {
                data: b"\x1b_Ga=T,i=1,q=2,m=1,q=2;AAAA\x1b\\\x1b_Gm=1,q=2;BBBB\x1b\\\x1b_Gm=0,q=2;CCCC\x1b\\"
                    .to_vec(),
                x: 1,
                y: 2
            })]
        );
        // A chunked query is answered once, at its end; an orphan chunk is dropped.
        assert!(handle(&mut g, "a=q,i=2,m=1;AAAA").is_empty());
        assert_eq!(handle(&mut g, "m=0;BBBB"), [reply("\x1b_Gi=2;OK\x1b\\")]);
        assert!(handle(&mut g, "m=0;CCCC").is_empty());
    }

    #[test]
    fn replays_ask_for_no_response() {
        assert_eq!(
            quiet(b"\x1b_Ga=T,i=1;AAAA\x1b\\"),
            b"\x1b_Ga=T,i=1,q=2;AAAA\x1b\\"
        );
        assert_eq!(quiet(b"\x1b_Gm=0;AAAA\x1b\\"), b"\x1b_Gm=0,q=2;AAAA\x1b\\");
        assert_eq!(quiet(b"\x1b_Ga=p,i=2\x1b\\"), b"\x1b_Ga=p,i=2,q=2\x1b\\");
    }

    fn cmd(s: &str) -> Command {
        Command {
            data: format!("\x1b_G{s}\x1b\\").into_bytes(),
            x: 0,
            y: 0,
        }
    }

    fn replayed(log: &Log) -> Vec<String> {
        log.replay()
            .into_iter()
            .map(|k| {
                k.data
                    .trim_start_matches("\x1b_G")
                    .trim_end_matches("\x1b\\")
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn the_log_keeps_what_is_still_shown() {
        let mut log = Log::new(1 << 20);
        log.record(cmd("a=T,i=1;AAAA"));
        log.record(cmd("a=t,i=2;BBBB"));
        log.record(cmd("a=p,i=2,p=1"));
        assert_eq!(
            replayed(&log),
            ["a=T,i=1;AAAA", "a=t,i=2;BBBB", "a=p,i=2,p=1"]
        );
        // Lowercase deletes take placements but keep the data for later ones.
        log.record(cmd("a=d,d=i,i=2"));
        log.record(cmd("a=d,d=a"));
        assert_eq!(replayed(&log), ["a=t,i=1;AAAA", "a=t,i=2;BBBB"]);
        // Uppercase frees the data; retransmitting replaces.
        log.record(cmd("a=d,d=I,i=1"));
        log.record(cmd("a=T,i=2;CCCC"));
        assert_eq!(replayed(&log), ["a=T,i=2;CCCC"]);
        log.record(cmd("a=d,d=A"));
        assert!(log.replay().is_empty());
    }

    #[test]
    fn the_log_is_bounded() {
        let mut log = Log::new(64);
        for i in 0..10 {
            log.record(cmd(&format!("a=T,i={i};AAAAAAAAAAAA")));
        }
        let kept = replayed(&log);
        assert!(
            kept.len() < 10 && kept.last().unwrap().contains("i=9"),
            "{kept:?}"
        );
    }
}

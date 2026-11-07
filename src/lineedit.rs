//! Interactive line editor for gash (POSIX terminal).
//!
//! Emacs-style editing, history, reverse incremental search and tab
//! completion on top of termios and ANSI escape sequences.  Zero
//! dependencies: the terminal calls live in `sys`.
//!
//! Rendering model: the prompt is printed once.  On every redraw only the
//! last prompt line plus the edit buffer are repainted.  We remember the
//! cursor's row offset relative to the row where the last prompt line starts,
//! so a redraw is: move up that many rows, `\r`, skip the start column, print
//! prompt line + buffer, `ESC[J`, then move the cursor to its logical spot.
//! All layout math (wrapping, wide chars, pending-wrap at the last column) is
//! done by the pure `layout` function.
//!
//! If stdin is not a terminal, `read_line` falls back to reading one line
//! byte-wise so it never over-reads input meant for children.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::ops::Range;

use crate::sys::{self, HANDLE as Handle};

// ===========================================================================
// Public API
// ===========================================================================

/// Result of [`LineEditor::read_line`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadResult {
    /// A line was entered (without the trailing newline).
    Line(String),
    /// End of input (Ctrl+D on an empty line, or EOF on a non-console stdin).
    Eof,
    /// Ctrl+C was pressed; the line was discarded.
    Interrupted,
}

/// One completion candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Full text that replaces the word being completed (already escaped).
    pub replace: String,
    /// What to show when listing candidates.
    pub display: String,
    /// Append a space if this is the single unique completion.
    pub append_space: bool,
}

/// Completer result: candidates for the word at `line[start..cursor]`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Completion {
    /// Byte offset in the line where the word being completed starts.
    pub start: usize,
    pub candidates: Vec<Candidate>,
}

/// Line editor state that persists between `read_line` calls.
pub struct LineEditor {
    history: Vec<String>,
    max_history: usize,
    kill_buf: String,
    last_search: String,
}

impl Default for LineEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl LineEditor {
    pub fn new() -> Self {
        LineEditor {
            history: Vec::new(),
            max_history: 500,
            kill_buf: String::new(),
            last_search: String::new(),
        }
    }

    /// True if stdin is an interactive terminal.
    pub fn is_console() -> bool {
        sys::is_console(sys::STD_INPUT_HANDLE)
    }

    /// Read one line.  See the module docs for details.
    pub fn read_line(
        &mut self,
        prompt: &str,
        completer: &mut dyn FnMut(&str, usize) -> Completion,
    ) -> ReadResult {
        if !Self::is_console() {
            return read_line_fallback(prompt);
        }
        let Some(out) = OutputHandle::find() else {
            return read_line_fallback(prompt);
        };
        let Some(_guard) = ModeGuard::enter(sys::STD_INPUT_HANDLE as Handle) else {
            return read_line_fallback(prompt);
        };
        // Drop order: session, then _guard (restores the terminal mode), then out.
        let mut session = Session::new(self, out.handle, prompt);
        session.run(completer)
    }

    /// Push a history entry (dups/empties are the caller's business).
    pub fn add_history(&mut self, line: &str) {
        self.history.push(line.to_string());
        self.trim_history();
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    pub fn set_history(&mut self, h: Vec<String>) {
        self.history = h;
        self.trim_history();
    }

    pub fn set_max_history(&mut self, n: usize) {
        self.max_history = n;
        self.trim_history();
    }

    fn trim_history(&mut self) {
        if self.history.len() > self.max_history {
            let excess = self.history.len() - self.max_history;
            self.history.drain(..excess);
        }
    }
}

// ===========================================================================
// Terminal
// ===========================================================================

/// Where we draw: stdout if it is a terminal, else stderr.
struct OutputHandle {
    handle: Handle,
}

impl OutputHandle {
    fn find() -> Option<OutputHandle> {
        for fd in [sys::STD_OUTPUT_HANDLE, sys::STD_ERROR_HANDLE] {
            if sys::is_console(fd) {
                return Some(OutputHandle { handle: fd as Handle });
            }
        }
        None
    }
}

/// Puts the terminal into raw mode and restores it on drop (including on panic).
struct ModeGuard {
    saved: sys::Termios,
}

impl ModeGuard {
    fn enter(fd: Handle) -> Option<ModeGuard> {
        let saved = sys::get_termios(fd)?;
        let mut raw = saved;
        sys::raw_termios(&mut raw);
        if !sys::set_termios(fd, &raw) {
            return None;
        }
        Some(ModeGuard { saved })
    }
}

impl Drop for ModeGuard {
    fn drop(&mut self) {
        sys::set_termios(sys::STD_INPUT_HANDLE as Handle, &self.saved);
    }
}

fn console_cols(h: Handle) -> usize {
    sys::winsize(h).map(|(c, _)| c).unwrap_or(80)
}

/// Ask the terminal for the cursor column (`ESC[6n`). Bytes that arrive before the
/// answer (type-ahead) go back into the queue.
fn cursor_column(h: Handle, q: &mut InputQueue) -> usize {
    if sys::write_fd(h, b"\x1b[6n") <= 0 {
        return 0;
    }
    let mut seen = Vec::new();
    loop {
        let Some(b) = q.read_byte(Some(50)) else { break };
        seen.push(b);
        if b == b'R' {
            if let Some(i) = seen.windows(2).rposition(|w| w == b"\x1b[") {
                let resp = String::from_utf8_lossy(&seen[i + 2..seen.len() - 1]).into_owned();
                if let Some(c) = resp.split(';').nth(1).and_then(|x| x.parse().ok()) {
                    q.unread(&seen[..i]);
                    return c;
                }
            }
            break;
        }
        if seen.len() > 32 {
            break;
        }
    }
    q.unread(&seen);
    0
}

fn write_console(h: Handle, s: &str) {
    let bytes = s.as_bytes();
    let mut off = 0;
    while off < bytes.len() {
        let n = sys::write_fd(h, &bytes[off..]);
        if n <= 0 {
            if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        off += n as usize;
    }
}

// ===========================================================================
// Non-console fallback
// ===========================================================================

fn read_line_fallback(prompt: &str) -> ReadResult {
    let p = strip_markers(prompt);
    if !p.is_empty() {
        use std::io::Write;
        let mut e = std::io::stderr();
        let _ = e.write_all(p.as_bytes());
        let _ = e.flush();
    }
    match read_raw_line() {
        None => ReadResult::Eof,
        Some(mut bytes) => {
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            match String::from_utf8(bytes) {
                Ok(s) => ReadResult::Line(s),
                Err(e) => ReadResult::Line(String::from_utf8_lossy(e.as_bytes()).into_owned()),
            }
        }
    }
}

/// Read up to (not including) the next '\n', one byte at a time, never consuming
/// input past the newline: it may be meant for a child.
fn read_raw_line() -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut b = 0u8;
    loop {
        let n = sys::read_fd(sys::STD_INPUT_HANDLE as Handle, std::slice::from_mut(&mut b));
        if n == 0 {
            break;
        }
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if b == b'\n' {
            return Some(out);
        }
        out.push(b);
    }
    if out.is_empty() { None } else { Some(out) }
}

// ===========================================================================
// Display width
// ===========================================================================

/// Zero-width code points (combining marks, ZW joiners, variation selectors,
/// Hangul medial vowels...).  Rough but sorted; checked before `WIDE`.
const ZERO_WIDTH: &[(u32, u32)] = &[
    (0x0300, 0x036F), (0x0483, 0x0489), (0x0591, 0x05BD), (0x05BF, 0x05BF),
    (0x05C1, 0x05C2), (0x05C4, 0x05C5), (0x05C7, 0x05C7), (0x0610, 0x061A),
    (0x064B, 0x065F), (0x0670, 0x0670), (0x06D6, 0x06DC), (0x06DF, 0x06E4),
    (0x06E7, 0x06E8), (0x06EA, 0x06ED), (0x0711, 0x0711), (0x0730, 0x074A),
    (0x07A6, 0x07B0), (0x07EB, 0x07F3), (0x0816, 0x0819), (0x081B, 0x0823),
    (0x0825, 0x0827), (0x0829, 0x082D), (0x0859, 0x085B), (0x08D3, 0x08E1),
    (0x08E3, 0x0902), (0x093A, 0x093A), (0x093C, 0x093C), (0x0941, 0x0948),
    (0x094D, 0x094D), (0x0951, 0x0957), (0x0962, 0x0963), (0x0981, 0x0981),
    (0x09BC, 0x09BC), (0x09C1, 0x09C4), (0x09CD, 0x09CD), (0x09E2, 0x09E3),
    (0x0A01, 0x0A02), (0x0A3C, 0x0A3C), (0x0A41, 0x0A42), (0x0A47, 0x0A48),
    (0x0A4B, 0x0A4D), (0x0A70, 0x0A71), (0x0A81, 0x0A82), (0x0ABC, 0x0ABC),
    (0x0AC1, 0x0AC5), (0x0AC7, 0x0AC8), (0x0ACD, 0x0ACD), (0x0B01, 0x0B01),
    (0x0B3C, 0x0B3C), (0x0B3F, 0x0B3F), (0x0B41, 0x0B44), (0x0B4D, 0x0B4D),
    (0x0B82, 0x0B82), (0x0BC0, 0x0BC0), (0x0BCD, 0x0BCD), (0x0C3E, 0x0C40),
    (0x0C46, 0x0C48), (0x0C4A, 0x0C4D), (0x0C55, 0x0C56), (0x0CBC, 0x0CBC),
    (0x0CCC, 0x0CCD), (0x0D41, 0x0D44), (0x0D4D, 0x0D4D), (0x0DCA, 0x0DCA),
    (0x0DD2, 0x0DD4), (0x0DD6, 0x0DD6), (0x0E31, 0x0E31), (0x0E34, 0x0E3A),
    (0x0E47, 0x0E4E), (0x0EB1, 0x0EB1), (0x0EB4, 0x0EBC), (0x0EC8, 0x0ECD),
    (0x0F18, 0x0F19), (0x0F35, 0x0F35), (0x0F37, 0x0F37), (0x0F39, 0x0F39),
    (0x0F71, 0x0F7E), (0x0F80, 0x0F84), (0x0F86, 0x0F87), (0x0F8D, 0x0FBC),
    (0x0FC6, 0x0FC6), (0x102D, 0x1030), (0x1032, 0x1037), (0x1039, 0x103A),
    (0x103D, 0x103E), (0x1058, 0x1059), (0x105E, 0x1060), (0x1071, 0x1074),
    (0x1082, 0x1082), (0x1085, 0x1086), (0x108D, 0x108D), (0x109D, 0x109D),
    (0x1160, 0x11FF), (0x135D, 0x135F), (0x1712, 0x1714), (0x1732, 0x1734),
    (0x1752, 0x1753), (0x1772, 0x1773), (0x17B4, 0x17B5), (0x17B7, 0x17BD),
    (0x17C6, 0x17C6), (0x17C9, 0x17D3), (0x17DD, 0x17DD), (0x180B, 0x180F),
    (0x18A9, 0x18A9), (0x1920, 0x1922), (0x1927, 0x1928), (0x1932, 0x1932),
    (0x1939, 0x193B), (0x1A17, 0x1A18), (0x1AB0, 0x1AFF), (0x1B00, 0x1B03),
    (0x1B34, 0x1B34), (0x1B36, 0x1B3A), (0x1B6B, 0x1B73), (0x1DC0, 0x1DFF),
    (0x200B, 0x200F), (0x202A, 0x202E), (0x2060, 0x206F), (0x20D0, 0x20F0),
    (0x2CEF, 0x2CF1), (0x2D7F, 0x2D7F), (0x2DE0, 0x2DFF), (0x302A, 0x302D),
    (0x3099, 0x309A), (0xA66F, 0xA672), (0xA674, 0xA67D), (0xA69E, 0xA69F),
    (0xA6F0, 0xA6F1), (0xA802, 0xA802), (0xA806, 0xA806), (0xA80B, 0xA80B),
    (0xA825, 0xA826), (0xA8C4, 0xA8C5), (0xA8E0, 0xA8F1), (0xA926, 0xA92D),
    (0xA947, 0xA951), (0xA980, 0xA982), (0xA9B3, 0xA9B3), (0xA9B6, 0xA9B9),
    (0xA9BC, 0xA9BD), (0xAA29, 0xAA2E), (0xAA31, 0xAA32), (0xAA35, 0xAA36),
    (0xABE5, 0xABE5), (0xABE8, 0xABE8), (0xABED, 0xABED), (0xFB1E, 0xFB1E),
    (0xFE00, 0xFE0F), (0xFE20, 0xFE2F), (0xFEFF, 0xFEFF), (0x101FD, 0x101FD),
    (0x10A01, 0x10A0F), (0x10A38, 0x10A3F), (0x11001, 0x11001), (0x11038, 0x11046),
    (0x1D167, 0x1D169), (0x1D173, 0x1D182), (0x1D185, 0x1D18B), (0x1D1AA, 0x1D1AD),
    (0x1F3FB, 0x1F3FF), (0xE0000, 0xE0FFF),
];

/// East-Asian wide / fullwidth / emoji-presentation code points (rough).
const WIDE: &[(u32, u32)] = &[
    (0x1100, 0x115F), (0x231A, 0x231B), (0x2329, 0x232A), (0x23E9, 0x23EC),
    (0x23F0, 0x23F0), (0x23F3, 0x23F3), (0x25FD, 0x25FE), (0x2614, 0x2615),
    (0x2648, 0x2653), (0x267F, 0x267F), (0x2693, 0x2693), (0x26A1, 0x26A1),
    (0x26AA, 0x26AB), (0x26BD, 0x26BE), (0x26C4, 0x26C5), (0x26CE, 0x26CE),
    (0x26D4, 0x26D4), (0x26EA, 0x26EA), (0x26F2, 0x26F3), (0x26F5, 0x26F5),
    (0x26FA, 0x26FA), (0x26FD, 0x26FD), (0x2705, 0x2705), (0x270A, 0x270B),
    (0x2728, 0x2728), (0x274C, 0x274C), (0x274E, 0x274E), (0x2753, 0x2755),
    (0x2757, 0x2757), (0x2795, 0x2797), (0x27B0, 0x27B0), (0x27BF, 0x27BF),
    (0x2B1B, 0x2B1C), (0x2B50, 0x2B50), (0x2B55, 0x2B55), (0x2E80, 0x303E),
    (0x3041, 0x3247), (0x3250, 0x4DBF), (0x4E00, 0xA4CF), (0xA960, 0xA97F),
    (0xAC00, 0xD7A3), (0xF900, 0xFAFF), (0xFE10, 0xFE19), (0xFE30, 0xFE6F),
    (0xFF00, 0xFF60), (0xFFE0, 0xFFE6), (0x16FE0, 0x16FE4), (0x17000, 0x18CFF),
    (0x1B000, 0x1B2FF), (0x1F004, 0x1F004), (0x1F0CF, 0x1F0CF), (0x1F18E, 0x1F18E),
    (0x1F191, 0x1F19A), (0x1F200, 0x1F202), (0x1F210, 0x1F23B), (0x1F240, 0x1F248),
    (0x1F250, 0x1F251), (0x1F260, 0x1F265), (0x1F300, 0x1F320), (0x1F32D, 0x1F335),
    (0x1F337, 0x1F37C), (0x1F37E, 0x1F393), (0x1F3A0, 0x1F3CA), (0x1F3CF, 0x1F3D3),
    (0x1F3E0, 0x1F3F0), (0x1F3F4, 0x1F3F4), (0x1F3F8, 0x1F3FA), (0x1F400, 0x1F43E),
    (0x1F440, 0x1F440), (0x1F442, 0x1F4FC), (0x1F4FF, 0x1F53D), (0x1F54B, 0x1F54E),
    (0x1F550, 0x1F567), (0x1F57A, 0x1F57A), (0x1F595, 0x1F596), (0x1F5A4, 0x1F5A4),
    (0x1F5FB, 0x1F64F), (0x1F680, 0x1F6C5), (0x1F6CC, 0x1F6CC), (0x1F6D0, 0x1F6D2),
    (0x1F6D5, 0x1F6D7), (0x1F6DC, 0x1F6DF), (0x1F6EB, 0x1F6EC), (0x1F6F4, 0x1F6FC),
    (0x1F7E0, 0x1F7EB), (0x1F7F0, 0x1F7F0), (0x1F90C, 0x1F93A), (0x1F93C, 0x1F945),
    (0x1F947, 0x1F9FF), (0x1FA70, 0x1FAFF), (0x20000, 0x2FFFD), (0x30000, 0x3FFFD),
];

fn in_table(table: &[(u32, u32)], u: u32) -> bool {
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < u {
                std::cmp::Ordering::Less
            } else if lo > u {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Terminal column width of a char: 0 for control chars and zero-width
/// marks, 2 for wide chars, else 1.
fn char_width(c: char) -> usize {
    let u = c as u32;
    if u < 0x20 || (0x7F..0xA0).contains(&u) {
        return 0;
    }
    if u < 0x300 {
        return 1;
    }
    if in_table(ZERO_WIDTH, u) {
        return 0;
    }
    if in_table(WIDE, u) {
        return 2;
    }
    1
}

fn str_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// A zero-width mark that attaches to the preceding char (not a control).
fn is_zero_width_mark(c: char) -> bool {
    let u = c as u32;
    u >= 0x300 && in_table(ZERO_WIDTH, u)
}

// ===========================================================================
// Prompt parsing
// ===========================================================================

/// One unit of horizontal layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Glyph {
    /// A printed cell group of the given width (0 = combining).
    W(u8),
    /// Carriage return: column 0.
    Cr,
    /// Horizontal tab: next multiple of 8.
    Tab,
}

#[derive(Debug, Clone)]
struct Prompt {
    /// Everything up to and including the last '\n', ready to print
    /// (markers stripped, "\n" -> "\r\n").
    head: String,
    /// The last line, ready to print (markers stripped, prefixed with the SGR
    /// sequences carried over from `head` so colours survive redraws).
    last: String,
    /// Visible glyphs of the last line.
    glyphs: Vec<Glyph>,
}

enum PTok {
    Ch(char),
    Zero(String),
    Nl,
}

/// Index just past the escape sequence starting at `chars[i]` (an ESC).
fn escape_end(chars: &[char], i: usize) -> usize {
    let n = chars.len();
    if i + 1 >= n {
        return n;
    }
    match chars[i + 1] {
        '[' => {
            // CSI: parameter/intermediate bytes 0x20-0x3F, final 0x40-0x7E.
            let mut j = i + 2;
            while j < n {
                let d = chars[j] as u32;
                if (0x40..=0x7E).contains(&d) {
                    return j + 1;
                }
                if !(0x20..0x40).contains(&d) {
                    return j; // malformed: stop before the offending char
                }
                j += 1;
            }
            n
        }
        ']' | 'P' | 'X' | '^' | '_' => {
            // OSC / DCS / SOS / PM / APC: until BEL or ST (ESC \).
            let mut j = i + 2;
            while j < n {
                if chars[j] == '\x07' {
                    return j + 1;
                }
                if chars[j] == '\x1b' && j + 1 < n && chars[j + 1] == '\\' {
                    return j + 2;
                }
                j += 1;
            }
            n
        }
        ' '..='/' => {
            // nF escape: intermediates then one final byte.
            let mut j = i + 1;
            while j < n && (0x20..0x30).contains(&(chars[j] as u32)) {
                j += 1;
            }
            (j + 1).min(n)
        }
        _ => i + 2,
    }
}

fn tokenize_prompt(p: &str) -> Vec<PTok> {
    let chars: Vec<char> = p.chars().collect();
    let mut toks: Vec<PTok> = Vec::new();
    let mut invisible = false;
    let mut i = 0;
    let push_zero = |toks: &mut Vec<PTok>, s: &str| {
        if let Some(PTok::Zero(z)) = toks.last_mut() {
            z.push_str(s);
        } else {
            toks.push(PTok::Zero(s.to_string()));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\x01' => {
                invisible = true;
                i += 1;
            }
            '\x02' => {
                invisible = false;
                i += 1;
            }
            _ if invisible => {
                let mut b = [0u8; 4];
                push_zero(&mut toks, c.encode_utf8(&mut b));
                i += 1;
            }
            '\x1b' => {
                let end = escape_end(&chars, i);
                let s: String = chars[i..end].iter().collect();
                push_zero(&mut toks, &s);
                i = end;
            }
            '\n' => {
                toks.push(PTok::Nl);
                i += 1;
            }
            _ => {
                toks.push(PTok::Ch(c));
                i += 1;
            }
        }
    }
    toks
}

/// Append every SGR sequence (`ESC [ ... m`) found in `s` to `out`.
fn extract_sgr(s: &str, out: &mut String) {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\x1b' {
            let end = escape_end(&chars, i);
            if end > i + 2 && chars[i + 1] == '[' && chars[end - 1] == 'm' {
                out.extend(&chars[i..end]);
            }
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }
}

fn parse_prompt(p: &str) -> Prompt {
    let toks = tokenize_prompt(p);
    let split = toks.iter().rposition(|t| matches!(t, PTok::Nl)).map_or(0, |k| k + 1);
    let mut head = String::new();
    let mut carry = String::new();
    for t in &toks[..split] {
        match t {
            PTok::Ch(c) => head.push(*c),
            PTok::Zero(z) => {
                head.push_str(z);
                extract_sgr(z, &mut carry);
            }
            PTok::Nl => {
                if !head.ends_with('\r') {
                    head.push('\r');
                }
                head.push('\n');
            }
        }
    }
    let mut last = carry;
    let mut glyphs = Vec::new();
    for t in &toks[split..] {
        match t {
            PTok::Ch(c) => {
                last.push(*c);
                match *c {
                    '\r' => glyphs.push(Glyph::Cr),
                    '\t' => glyphs.push(Glyph::Tab),
                    c if (c as u32) < 0x20 || c == '\x7f' => {} // raw, zero width
                    c => glyphs.push(Glyph::W(char_width(c) as u8)),
                }
            }
            PTok::Zero(z) => last.push_str(z),
            PTok::Nl => {}
        }
    }
    Prompt { head, last, glyphs }
}

/// Remove readline's `\x01` / `\x02` invisible markers.
fn strip_markers(p: &str) -> String {
    p.chars().filter(|&c| c != '\x01' && c != '\x02').collect()
}

// ===========================================================================
// Layout
// ===========================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    /// Row/col (relative to the start row) where the cursor must be shown.
    cursor: (usize, usize),
    /// Row/col of the terminal cursor after printing everything (normalised:
    /// if the text exactly filled the last column this is the start of the
    /// next row and `pending_wrap` is true).
    end: (usize, usize),
    /// Text ended exactly at the right margin; the renderer must emit
    /// " \r" so the terminal really is at `end`.
    pending_wrap: bool,
}

/// Lay out glyphs starting at `start_col` on a terminal `cols` wide.
/// `cursor_idx` is the glyph index the cursor sits on (== len for the end).
fn layout(glyphs: &[Glyph], cols: usize, start_col: usize, cursor_idx: usize) -> Layout {
    let cols = cols.max(1);
    let mut row = 0usize;
    let mut col = start_col.min(cols - 1);
    let mut cursor = None;
    for (i, g) in glyphs.iter().enumerate() {
        if i == cursor_idx {
            cursor = Some(cursor_cell(row, col, Some(*g), cols));
        }
        match *g {
            Glyph::W(0) => {}
            Glyph::W(w) => {
                let w = w as usize;
                // A glyph that doesn't fit (incl. a wide char at the last
                // column, or anything while in pending-wrap) goes to the next row.
                if col + w > cols && col > 0 {
                    row += 1;
                    col = 0;
                }
                col += w;
            }
            Glyph::Cr => col = 0,
            Glyph::Tab => {
                if col >= cols {
                    row += 1;
                    col = 0;
                }
                col = ((col / 8 + 1) * 8).min(cols - 1);
            }
        }
    }
    let cursor = cursor.unwrap_or_else(|| cursor_cell(row, col, None, cols));
    let (end, pending_wrap) = if col >= cols { ((row + 1, 0), true) } else { ((row, col), false) };
    Layout { cursor, end, pending_wrap }
}

/// Where the cursor shows when the next glyph to draw is `next`.
fn cursor_cell(row: usize, col: usize, next: Option<Glyph>, cols: usize) -> (usize, usize) {
    if col >= cols {
        return (row + 1, 0);
    }
    match next {
        Some(Glyph::W(w)) if w > 0 && col > 0 && col + w as usize > cols => (row + 1, 0),
        _ => (row, col),
    }
}

/// Render one buffer char: control chars as caret notation (two 1-wide
/// glyphs), C1 controls as '?', everything else verbatim.
fn render_char(c: char, out: &mut String, glyphs: &mut Vec<Glyph>) {
    let u = c as u32;
    if u < 0x20 || u == 0x7F {
        out.push('^');
        out.push(((u as u8) ^ 0x40) as char);
        glyphs.push(Glyph::W(1));
        glyphs.push(Glyph::W(1));
    } else if (0x80..0xA0).contains(&u) {
        out.push('?');
        glyphs.push(Glyph::W(1));
    } else {
        out.push(c);
        glyphs.push(Glyph::W(char_width(c) as u8));
    }
}

// ===========================================================================
// Pure text-editing helpers (byte offsets, always on char boundaries)
// ===========================================================================

/// Start of the cluster (char + trailing zero-width marks) before `pos`.
fn prev_boundary(s: &str, pos: usize) -> usize {
    let mut i = pos;
    for (idx, c) in s[..pos].char_indices().rev() {
        i = idx;
        if !is_zero_width_mark(c) {
            break;
        }
    }
    i
}

/// End of the cluster starting at `pos`.
fn next_boundary(s: &str, pos: usize) -> usize {
    let mut it = s[pos..].char_indices();
    let Some((_, c)) = it.next() else { return pos };
    let mut i = pos + c.len_utf8();
    for (idx, c) in it {
        if !is_zero_width_mark(c) {
            break;
        }
        i = pos + idx + c.len_utf8();
    }
    i
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || is_zero_width_mark(c)
}

/// readline backward-word: skip non-word chars, then word chars.
fn word_left(s: &str, pos: usize) -> usize {
    let mut i = pos;
    let mut it = s[..pos].char_indices().rev().peekable();
    while let Some(&(idx, c)) = it.peek() {
        if is_word_char(c) {
            break;
        }
        i = idx;
        it.next();
    }
    while let Some(&(idx, c)) = it.peek() {
        if !is_word_char(c) {
            break;
        }
        i = idx;
        it.next();
    }
    i
}

/// readline forward-word: skip non-word chars, then word chars (end of word).
fn word_right(s: &str, pos: usize) -> usize {
    let mut i = pos;
    let mut it = s[pos..].char_indices().peekable();
    while let Some(&(idx, c)) = it.peek() {
        if is_word_char(c) {
            break;
        }
        i = pos + idx + c.len_utf8();
        it.next();
    }
    while let Some(&(idx, c)) = it.peek() {
        if !is_word_char(c) {
            break;
        }
        i = pos + idx + c.len_utf8();
        it.next();
    }
    i
}

/// unix-word-rubout start: skip whitespace backward, then non-whitespace.
fn unix_word_left(s: &str, pos: usize) -> usize {
    let mut i = pos;
    let mut it = s[..pos].char_indices().rev().peekable();
    while let Some(&(idx, c)) = it.peek() {
        if !c.is_whitespace() {
            break;
        }
        i = idx;
        it.next();
    }
    while let Some(&(idx, c)) = it.peek() {
        if c.is_whitespace() {
            break;
        }
        i = idx;
        it.next();
    }
    i
}

/// readline transpose-chars.  Returns false (beep) if impossible.
fn transpose(s: &mut String, cur: &mut usize) -> bool {
    if *cur == 0 {
        return false;
    }
    let mut pos = *cur;
    if pos == s.len() {
        pos = prev_boundary(s, pos);
    }
    let a_start = prev_boundary(s, pos);
    if a_start == pos {
        return false;
    }
    let b_end = next_boundary(s, pos);
    let swapped = format!("{}{}", &s[pos..b_end], &s[a_start..pos]);
    s.replace_range(a_start..b_end, &swapped);
    *cur = b_end;
    true
}

/// Longest common prefix (on char boundaries).
fn common_prefix<'a>(mut items: impl Iterator<Item = &'a str>) -> &'a str {
    let Some(first) = items.next() else { return "" };
    let mut len = first.len();
    for s in items {
        let mut other = s.chars();
        let mut l = 0;
        for (i, a) in first[..len].char_indices() {
            match other.next() {
                Some(b) if b == a => l = i + a.len_utf8(),
                _ => break,
            }
        }
        len = l;
        if len == 0 {
            break;
        }
    }
    &first[..len]
}

fn last_word(s: &str) -> Option<&str> {
    s.split_whitespace().last()
}

/// Search history for `query` starting at index `from` (inclusive), going
/// backward (towards 0) or forward.  Entries equal to `skip` are skipped.
/// Returns (entry index, byte offset of the match).
fn hist_find(
    hist: &[String],
    query: &str,
    from: usize,
    backward: bool,
    skip: Option<&str>,
) -> Option<(usize, usize)> {
    if query.is_empty() || hist.is_empty() {
        return None;
    }
    let check = |i: usize| -> Option<(usize, usize)> {
        let e = hist[i].as_str();
        if Some(e) == skip {
            return None;
        }
        let p = if backward { e.rfind(query) } else { e.find(query) };
        p.map(|p| (i, p))
    };
    if backward {
        let from = from.min(hist.len() - 1);
        (0..=from).rev().find_map(check)
    } else {
        (from..hist.len()).find_map(check)
    }
}

fn sanitize_display(s: &str) -> String {
    s.chars().map(|c| if char_width(c) == 0 && c.is_control() { '?' } else { c }).collect()
}

/// Format completion candidates in columns (sorted down, then across, like
/// readline).  Every row ends with "\r\n".
fn format_columns(items: &[String], cols: usize) -> String {
    let n = items.len();
    if n == 0 {
        return String::new();
    }
    let widths: Vec<usize> = items.iter().map(|s| str_width(s)).collect();
    let maxw = widths.iter().copied().max().unwrap_or(0);
    let colw = maxw + 2;
    let ncols = ((cols.max(1) + 2) / colw).max(1);
    let nrows = n.div_ceil(ncols);
    let mut out = String::new();
    for r in 0..nrows {
        for c in 0..ncols {
            let i = c * nrows + r;
            if i >= n {
                break;
            }
            out.push_str(&items[i]);
            if c + 1 < ncols && i + nrows < n {
                for _ in widths[i]..colw {
                    out.push(' ');
                }
            }
        }
        out.push_str("\r\n");
    }
    out
}

// ===========================================================================
// Key decoding
// ===========================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    /// A sequence we don't bind (mouse reports, function keys...).
    Ignore,
    Char(char),
    Ctrl(char),
    Alt(char),
    Enter,
    Tab,
    Esc,
    Backspace,
    AltBackspace,
    Delete,
    CtrlDelete,
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

// ===========================================================================
// Input queue
// ===========================================================================

/// Terminal input reader. Bytes are consumed one at a time, so nothing past the
/// key that ends a line is taken from the terminal: it stays queued for a child.
struct InputQueue {
    fd: Handle,
    pending: VecDeque<u8>,
}

impl InputQueue {
    fn new(fd: Handle) -> Self {
        InputQueue { fd, pending: VecDeque::new() }
    }

    fn unread(&mut self, bytes: &[u8]) {
        for &b in bytes.iter().rev() {
            self.pending.push_front(b);
        }
    }

    /// More input is available without blocking.
    fn has_pending(&self) -> bool {
        !self.pending.is_empty() || sys::wait_fd(self.fd, 0)
    }

    /// Next byte, blocking unless `timeout` is given (milliseconds).
    fn read_byte(&mut self, timeout: Option<u32>) -> Option<u8> {
        if let Some(b) = self.pending.pop_front() {
            return Some(b);
        }
        if let Some(ms) = timeout {
            if !sys::wait_fd(self.fd, ms) {
                return None;
            }
        }
        let mut b = 0u8;
        loop {
            let n = sys::read_fd(self.fd, std::slice::from_mut(&mut b));
            if n == 1 {
                return Some(b);
            }
            if n == 0 {
                return None;
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return None;
            }
        }
    }

    /// Next key, blocking. `None` on end of input.
    fn next_key(&mut self) -> Option<Key> {
        let b = self.read_byte(None)?;
        Some(self.decode(b))
    }

    fn decode(&mut self, b: u8) -> Key {
        match b {
            0x1b => {
                let Some(b2) = self.read_byte(Some(20)) else {
                    return Key::Esc;
                };
                match b2 {
                    b'[' | b'O' => self.decode_csi(),
                    0x7f | 0x08 => Key::AltBackspace,
                    _ if b2 < 0x80 => Key::Alt(b2 as char),
                    _ => self.utf8_char(b2),
                }
            }
            0x7f | 0x08 => Key::Backspace,
            b'\r' | b'\n' => Key::Enter,
            b'\t' => Key::Tab,
            0x01..=0x1a => Key::Ctrl((b'a' + b - 1) as char),
            0x1c..=0x1f => Key::Ctrl((b + 0x40) as char),
            b if b < 0x80 => Key::Char(b as char),
            b => self.utf8_char(b),
        }
    }

    fn decode_csi(&mut self) -> Key {
        let mut seq = String::new();
        let mut fin = 0u8;
        for _ in 0..16 {
            let Some(b) = self.read_byte(Some(20)) else {
                return Key::Esc;
            };
            if (0x40..=0x7e).contains(&b) {
                fin = b;
                break;
            }
            seq.push(b as char);
        }
        let nums: Vec<u32> = seq.split(';').filter_map(|s| s.parse().ok()).collect();
        let ctrl = matches!(nums.get(1), Some(5 | 6));
        let alt = matches!(nums.get(1), Some(3 | 4));
        match fin {
            b'A' => Key::Up,
            b'B' => Key::Down,
            b'C' => if ctrl || alt { Key::WordRight } else { Key::Right },
            b'D' => if ctrl || alt { Key::WordLeft } else { Key::Left },
            b'H' => Key::Home,
            b'F' => Key::End,
            b'~' => match nums.first().copied().unwrap_or(0) {
                1 | 7 => Key::Home,
                3 => if ctrl || alt { Key::CtrlDelete } else { Key::Delete },
                4 | 8 => Key::End,
                5 => Key::PageUp,
                6 => Key::PageDown,
                _ => Key::Ignore,
            },
            _ => Key::Ignore,
        }
    }

    fn utf8_char(&mut self, first: u8) -> Key {
        let len = match first {
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => return Key::Ignore,
        };
        let mut bytes = vec![first];
        for _ in 1..len {
            let Some(b) = self.read_byte(Some(20)) else {
                return Key::Ignore;
            };
            bytes.push(b);
        }
        match std::str::from_utf8(&bytes).ok().and_then(|s| s.chars().next()) {
            Some(c) => Key::Char(c),
            None => Key::Ignore,
        }
    }
}

// ===========================================================================
// Interactive session
// ===========================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LastCmd {
    Other,
    Kill,
    Tab,
    YankArg,
}

struct Search {
    query: String,
    original: String,
    original_cursor: usize,
    /// History position when the search started.
    start: usize,
    /// Current match: (history index, byte offset).
    found: Option<(usize, usize)>,
    failed: bool,
    forward: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchStep {
    /// Query grew: re-check from the current match (inclusive).
    Extend,
    /// Query changed arbitrarily: search again from the start position.
    Restart,
    /// Ctrl+R / Ctrl+S: next match beyond the current one.
    Next,
}

struct YankArg {
    hist_pos: usize,
    start: usize,
    end: usize,
}

struct Session<'a> {
    le: &'a mut LineEditor,
    hout: Handle,
    queue: InputQueue,
    prompt: Prompt,

    buf: String,
    cursor: usize,
    hist_idx: usize,
    /// Edited copies of history entries (index == len is the new line).
    edits: HashMap<usize, String>,
    search: Option<Search>,
    last_cmd: LastCmd,
    yank_arg: Option<YankArg>,
    dirty: bool,
    /// The previous Tab left the line unchanged (readline lists only after such a Tab).
    tab_was_noop: bool,

    // What is on screen.
    cursor_row: usize,
    start_col: usize,
    last_cols: usize,
    last_glyphs: Vec<Glyph>,
    last_cursor_glyph: usize,
    last_end: (usize, usize),
    last_pending: bool,
}

type Completer<'c> = dyn FnMut(&str, usize) -> Completion + 'c;

impl<'a> Session<'a> {
    fn new(le: &'a mut LineEditor, hout: Handle, prompt: &str) -> Self {
        let hist_idx = le.history.len();
        Session {
            le,
            hout,
            queue: InputQueue::new(sys::STD_INPUT_HANDLE as Handle),
            prompt: parse_prompt(prompt),
            buf: String::new(),
            cursor: 0,
            hist_idx,
            edits: HashMap::new(),
            search: None,
            last_cmd: LastCmd::Other,
            yank_arg: None,
            dirty: false,
            tab_was_noop: false,
            cursor_row: 0,
            start_col: 0,
            last_cols: 0,
            last_glyphs: Vec::new(),
            last_cursor_glyph: 0,
            last_end: (0, 0),
            last_pending: false,
        }
    }

    fn run(&mut self, completer: &mut Completer<'_>) -> ReadResult {
        // A single-line prompt continues wherever the cursor is (bash prints
        // `foo$ ` after output lacking a newline); a multi-line prompt's last
        // line always starts at column 0.
        self.start_col = if self.prompt.head.is_empty() { cursor_column(self.hout, &mut self.queue) } else { 0 };
        let head = self.prompt.head.clone();
        self.refresh_with(&head);
        loop {
            // Never block with a stale screen; skip redraws while more input
            // (e.g. a paste) is already queued.
            if self.dirty && !self.queue.has_pending() {
                self.refresh();
            }
            let Some(key) = self.queue.next_key() else {
                return ReadResult::Eof;
            };
            if let Some(r) = self.handle_key(key, completer) {
                if self.dirty {
                    self.refresh();
                }
                return r;
            }
        }
    }

    // ----- output --------------------------------------------------------

    fn write(&self, s: &str) {
        write_console(self.hout, s);
    }

    fn beep(&self) {
        self.write("\x07");
    }

    fn refresh(&mut self) {
        self.refresh_with("");
    }

    /// Repaint the last prompt line and the buffer in a single write,
    /// preceded by `pre` (e.g. the prompt head or a clear-screen sequence).
    fn refresh_with(&mut self, pre: &str) {
        let cols = console_cols(self.hout);
        // The terminal reflowed our wrapped line on resize: recompute which
        // row the cursor is on now.
        if self.last_cols != 0 && cols != self.last_cols && !self.last_glyphs.is_empty() {
            let l = layout(&self.last_glyphs, cols, self.start_col, self.last_cursor_glyph);
            self.cursor_row = l.cursor.0;
        }

        let (prompt_text, mut glyphs, text, cur) = match &self.search {
            Some(s) => {
                let p = format!(
                    "({}{}i-search)`{}': ",
                    if s.failed { "failed " } else { "" },
                    if s.forward { "" } else { "reverse-" },
                    s.query
                );
                let g: Vec<Glyph> = p.chars().map(|c| Glyph::W(char_width(c) as u8)).collect();
                let (t, c) = match s.found {
                    Some((i, pos)) => (self.le.history[i].clone(), pos),
                    None => (s.original.clone(), s.original_cursor),
                };
                (p, g, t, c)
            }
            None => (
                self.prompt.last.clone(),
                self.prompt.glyphs.clone(),
                self.buf.clone(),
                self.cursor,
            ),
        };

        let mut out = String::with_capacity(pre.len() + prompt_text.len() + text.len() * 2 + 32);
        out.push_str(pre);
        out.push_str("\x1b[?25l");
        if self.cursor_row > 0 {
            let _ = write!(out, "\x1b[{}A", self.cursor_row);
        }
        out.push('\r');
        if self.start_col > 0 {
            let _ = write!(out, "\x1b[{}C", self.start_col);
        }
        out.push_str(&prompt_text);
        let mut cursor_glyph = None;
        for (i, c) in text.char_indices() {
            if i == cur {
                cursor_glyph = Some(glyphs.len());
            }
            render_char(c, &mut out, &mut glyphs);
        }
        let cursor_glyph = cursor_glyph.unwrap_or(glyphs.len());
        let l = layout(&glyphs, cols, self.start_col, cursor_glyph);
        if l.pending_wrap {
            // Leave the pending-wrap state so the terminal cursor really is
            // at the start of the next row (the space is erased by ESC[J).
            out.push_str(" \r");
        }
        out.push_str("\x1b[J");
        if l.end != l.cursor {
            if l.end.0 > l.cursor.0 {
                let _ = write!(out, "\x1b[{}A", l.end.0 - l.cursor.0);
            }
            out.push('\r');
            if l.cursor.1 > 0 {
                let _ = write!(out, "\x1b[{}C", l.cursor.1);
            }
        }
        out.push_str("\x1b[?25h");
        self.write(&out);

        self.cursor_row = l.cursor.0;
        self.last_cols = cols;
        self.last_glyphs = glyphs;
        self.last_cursor_glyph = cursor_glyph;
        self.last_end = l.end;
        self.last_pending = l.pending_wrap;
        self.dirty = false;
    }

    /// Escape sequence moving from the cursor to the end of the (freshly
    /// drawn) line and onto a new line.
    fn goto_end_newline(&self) -> String {
        let mut s = String::new();
        let (er, ec) = self.last_end;
        if er > self.cursor_row {
            let _ = write!(s, "\x1b[{}B", er - self.cursor_row);
        }
        s.push('\r');
        if ec > 0 {
            let _ = write!(s, "\x1b[{}C", ec);
        }
        if !self.last_pending {
            s.push_str("\r\n");
        }
        s
    }

    /// Print `pre`, then the whole prompt and the line, from column 0.
    fn redraw_full(&mut self, pre: &str) {
        self.cursor_row = 0;
        self.start_col = 0;
        self.last_cols = 0;
        self.last_glyphs.clear();
        let mut s = String::from(pre);
        s.push_str(&self.prompt.head);
        self.refresh_with(&s);
    }

    // ----- key handling --------------------------------------------------

    fn handle_key(&mut self, key: Key, completer: &mut Completer<'_>) -> Option<ReadResult> {
        self.dirty = true;
        if self.search.is_some() {
            match key {
                Key::Char(c) => {
                    if let Some(s) = self.search.as_mut() {
                        s.query.push(c);
                    }
                    self.search_step(SearchStep::Extend);
                    return None;
                }
                Key::Backspace => {
                    let popped = self.search.as_mut().and_then(|s| s.query.pop()).is_some();
                    if popped {
                        self.search_step(SearchStep::Restart);
                    } else {
                        self.beep();
                    }
                    return None;
                }
                Key::Ctrl('r') | Key::Ctrl('s') => {
                    let forward = key == Key::Ctrl('s');
                    let last = self.le.last_search.clone();
                    let s = self.search.as_mut()?;
                    s.forward = forward;
                    if !s.query.is_empty() {
                        self.search_step(SearchStep::Next);
                    } else if !last.is_empty() {
                        // Empty query: reuse the previous search string.
                        s.query = last;
                        self.search_step(SearchStep::Restart);
                    }
                    return None;
                }
                Key::Esc | Key::Ctrl('g') => {
                    if let Some(s) = self.search.take() {
                        self.buf = s.original;
                        self.cursor = s.original_cursor;
                    }
                    return None;
                }
                Key::Ctrl('c') => {} // interrupt() restores the original line
                Key::Enter => {
                    self.search_accept();
                    return Some(self.finish_line());
                }
                _ => self.search_accept(), // then run the key normally
            }
        }

        let prev = std::mem::replace(&mut self.last_cmd, LastCmd::Other);
        match key {
            Key::Char(c) => {
                let mut b = [0u8; 4];
                self.insert(c.encode_utf8(&mut b));
            }
            Key::Enter => return Some(self.finish_line()),
            Key::Tab => {
                self.complete(completer, prev == LastCmd::Tab);
                self.last_cmd = LastCmd::Tab;
            }
            Key::Backspace => {
                if self.cursor > 0 {
                    let p = prev_boundary(&self.buf, self.cursor);
                    self.buf.replace_range(p..self.cursor, "");
                    self.cursor = p;
                }
            }
            Key::Delete => self.delete_forward(),
            Key::Left | Key::Ctrl('b') => self.cursor = prev_boundary(&self.buf, self.cursor),
            Key::Right | Key::Ctrl('f') => self.cursor = next_boundary(&self.buf, self.cursor),
            Key::Home | Key::Ctrl('a') => self.cursor = 0,
            Key::End | Key::Ctrl('e') => self.cursor = self.buf.len(),
            Key::WordLeft | Key::Alt('b') | Key::Alt('B') => {
                self.cursor = word_left(&self.buf, self.cursor)
            }
            Key::WordRight | Key::Alt('f') | Key::Alt('F') => {
                self.cursor = word_right(&self.buf, self.cursor)
            }
            Key::Up | Key::Ctrl('p') => {
                if self.hist_idx > 0 {
                    self.hist_goto(self.hist_idx - 1);
                }
            }
            Key::Down | Key::Ctrl('n') => {
                if self.hist_idx < self.le.history.len() {
                    self.hist_goto(self.hist_idx + 1);
                }
            }
            Key::PageUp | Key::Alt('<') => self.hist_goto(0),
            Key::PageDown | Key::Alt('>') => self.hist_goto(self.le.history.len()),
            Key::Ctrl('d') => {
                if self.buf.is_empty() {
                    return Some(ReadResult::Eof);
                }
                self.delete_forward();
            }
            Key::Ctrl('w') => {
                let s = unix_word_left(&self.buf, self.cursor);
                self.kill(s..self.cursor, true, prev);
            }
            Key::AltBackspace => {
                let s = word_left(&self.buf, self.cursor);
                self.kill(s..self.cursor, true, prev);
            }
            Key::Alt('d') | Key::Alt('D') | Key::CtrlDelete => {
                let e = word_right(&self.buf, self.cursor);
                self.kill(self.cursor..e, false, prev);
            }
            Key::Ctrl('k') => self.kill(self.cursor..self.buf.len(), false, prev),
            Key::Ctrl('u') => self.kill(0..self.cursor, true, prev),
            Key::Ctrl('y') => {
                let k = self.le.kill_buf.clone();
                self.insert(&k);
            }
            Key::Ctrl('t') => {
                if !transpose(&mut self.buf, &mut self.cursor) {
                    self.beep();
                }
            }
            Key::Ctrl('l') => self.redraw_full("\x1b[H\x1b[2J"),
            Key::Ctrl('c') => return Some(self.interrupt()),
            Key::Esc => {
                self.buf.clear();
                self.cursor = 0;
            }
            Key::Ctrl('r') | Key::Ctrl('s') => {
                self.search = Some(Search {
                    query: String::new(),
                    original: self.buf.clone(),
                    original_cursor: self.cursor,
                    start: self.hist_idx,
                    found: None,
                    failed: false,
                    forward: key == Key::Ctrl('s'),
                });
            }
            Key::Alt('.') | Key::Alt('_') => self.yank_last_arg(prev == LastCmd::YankArg),
            Key::Ctrl('g') => self.beep(),
            _ => {}
        }
        None
    }

    fn insert(&mut self, s: &str) {
        self.buf.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    fn delete_forward(&mut self) {
        if self.cursor < self.buf.len() {
            let e = next_boundary(&self.buf, self.cursor);
            self.buf.replace_range(self.cursor..e, "");
        }
    }

    /// Kill `r` into the kill buffer; consecutive kills accumulate.
    fn kill(&mut self, r: Range<usize>, backward: bool, prev: LastCmd) {
        if r.is_empty() {
            if prev == LastCmd::Kill {
                self.last_cmd = LastCmd::Kill;
            }
            return;
        }
        let text = self.buf[r.clone()].to_string();
        self.buf.replace_range(r.clone(), "");
        self.cursor = r.start;
        if prev == LastCmd::Kill {
            if backward {
                self.le.kill_buf.insert_str(0, &text);
            } else {
                self.le.kill_buf.push_str(&text);
            }
        } else {
            self.le.kill_buf = text;
        }
        self.last_cmd = LastCmd::Kill;
    }

    fn finish_line(&mut self) -> ReadResult {
        self.cursor = self.buf.len();
        self.refresh();
        if !self.last_pending {
            self.write("\r\n");
        }
        ReadResult::Line(std::mem::take(&mut self.buf))
    }

    fn interrupt(&mut self) -> ReadResult {
        if let Some(s) = self.search.take() {
            self.buf = s.original;
        }
        self.cursor = self.buf.len();
        self.refresh();
        self.write("^C\r\n");
        ReadResult::Interrupted
    }

    // ----- history -------------------------------------------------------

    fn hist_goto(&mut self, idx: usize) {
        let len = self.le.history.len();
        if idx > len || idx == self.hist_idx {
            return;
        }
        let cur = self.hist_idx;
        if cur < len && self.le.history[cur] == self.buf {
            self.edits.remove(&cur);
        } else {
            self.edits.insert(cur, std::mem::take(&mut self.buf));
        }
        self.hist_idx = idx;
        self.buf = match self.edits.get(&idx) {
            Some(s) => s.clone(),
            None if idx < len => self.le.history[idx].clone(),
            None => String::new(),
        };
        self.cursor = self.buf.len();
    }

    fn search_step(&mut self, step: SearchStep) {
        let hist = &self.le.history;
        let Some(s) = self.search.as_mut() else { return };
        if s.query.is_empty() {
            s.found = None;
            s.failed = false;
            return;
        }
        let backward = !s.forward;
        let from_start = |s: &Search| {
            if backward {
                s.start.checked_sub(1).and_then(|f| hist_find(hist, &s.query, f, true, None))
            } else {
                hist_find(hist, &s.query, s.start + 1, false, None)
            }
        };
        let res = match (step, s.found) {
            (SearchStep::Restart, _) | (_, None) => from_start(s),
            (SearchStep::Extend, Some((i, _))) => hist_find(hist, &s.query, i, backward, None),
            (SearchStep::Next, Some((i, _))) => {
                let skip = Some(hist[i].as_str());
                if backward {
                    i.checked_sub(1).and_then(|f| hist_find(hist, &s.query, f, true, skip))
                } else {
                    hist_find(hist, &s.query, i + 1, false, skip)
                }
            }
        };
        let failed = match res {
            Some(f) => {
                s.found = Some(f);
                s.failed = false;
                false
            }
            None => {
                s.failed = true;
                true
            }
        };
        if failed {
            self.beep();
        }
    }

    /// Leave search mode, putting the match (if any) into the buffer.
    fn search_accept(&mut self) {
        let Some(s) = self.search.take() else { return };
        if !s.query.is_empty() {
            self.le.last_search = s.query.clone();
        }
        self.buf = s.original;
        self.cursor = s.original_cursor;
        if let Some((i, pos)) = s.found {
            self.hist_goto(i);
            self.buf = self.le.history[i].clone();
            self.cursor = pos;
        }
    }

    /// Alt+. : insert the last word of the previous history entry; repeating
    /// replaces it with the last word of the entry before that.
    fn yank_last_arg(&mut self, repeat: bool) {
        let from = match (&self.yank_arg, repeat) {
            (Some(y), true) => y.hist_pos,
            _ => self.le.history.len(),
        };
        let found = (0..from)
            .rev()
            .find_map(|i| last_word(&self.le.history[i]).map(|w| (i, w.to_string())));
        let Some((i, word)) = found else {
            self.beep();
            if repeat {
                self.last_cmd = LastCmd::YankArg;
            }
            return;
        };
        // Replace the previously yanked word when repeating.
        let prev = if repeat { self.yank_arg.take() } else { None };
        if let Some(y) = prev.filter(|y| {
            y.start <= y.end
                && y.end <= self.buf.len()
                && self.buf.is_char_boundary(y.start)
                && self.buf.is_char_boundary(y.end)
        }) {
            self.buf.replace_range(y.start..y.end, "");
            self.cursor = y.start;
        }
        let start = self.cursor;
        self.insert(&word);
        self.yank_arg = Some(YankArg { hist_pos: i, start, end: self.cursor });
        self.last_cmd = LastCmd::YankArg;
    }

    // ----- completion ----------------------------------------------------

    fn complete(&mut self, completer: &mut Completer<'_>, consecutive: bool) {
        let list_ok = consecutive && self.tab_was_noop;
        let before = (self.buf.clone(), self.cursor);
        self.complete_inner(completer, list_ok);
        self.tab_was_noop = before.0 == self.buf && before.1 == self.cursor;
    }

    fn complete_inner(&mut self, completer: &mut Completer<'_>, consecutive: bool) {
        let comp = completer(&self.buf, self.cursor);
        let mut start = comp.start.min(self.cursor);
        while !self.buf.is_char_boundary(start) {
            start -= 1;
        }
        let cands = comp.candidates;
        match cands.len() {
            0 => self.beep(),
            1 => {
                let c = &cands[0];
                let mut r = c.replace.clone();
                if c.append_space {
                    r.push(' ');
                }
                self.buf.replace_range(start..self.cursor, &r);
                self.cursor = start + r.len();
            }
            _ => {
                let lcp = common_prefix(cands.iter().map(|c| c.replace.as_str()));
                if lcp.len() > self.cursor - start {
                    let lcp = lcp.to_string();
                    self.buf.replace_range(start..self.cursor, &lcp);
                    self.cursor = start + lcp.len();
                } else if consecutive {
                    self.list_candidates(&cands);
                } else {
                    self.beep();
                }
            }
        }
    }

    fn list_candidates(&mut self, cands: &[Candidate]) {
        if self.dirty {
            self.refresh();
        }
        let mut items: Vec<String> = cands.iter().map(|c| sanitize_display(&c.display)).collect();
        items.sort();
        items.dedup();
        let mut out = self.goto_end_newline();
        if items.len() > 100 {
            let _ = write!(out, "Display all {} possibilities? (y or n)", items.len());
            self.write(&out);
            out.clear();
            let yes = self.ask_yes_no();
            out.push_str("\r\n");
            if !yes {
                self.redraw_full(&out);
                return;
            }
        }
        out.push_str(&format_columns(&items, console_cols(self.hout)));
        self.redraw_full(&out);
    }

    fn ask_yes_no(&mut self) -> bool {
        loop {
            match self.queue.next_key() {
                None => return false,
                Some(Key::Char('y' | 'Y' | ' ')) => return true,
                Some(Key::Char('n' | 'N') | Key::Esc | Key::Backspace | Key::Ctrl('g') | Key::Ctrl('c')) => return false,
                Some(_) => self.beep(),
            }
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(n: usize) -> Vec<Glyph> {
        vec![Glyph::W(1); n]
    }

    #[test]
    fn width_tables_sorted() {
        for t in [ZERO_WIDTH, WIDE] {
            for w in t.windows(2) {
                assert!(w[0].0 <= w[0].1 && w[0].1 < w[1].0, "{:x?}", w);
            }
        }
    }

    #[test]
    fn widths() {
        assert_eq!(char_width('a'), 1);
        assert_eq!(char_width('é'), 1);
        assert_eq!(char_width('中'), 2);
        assert_eq!(char_width('한'), 2);
        assert_eq!(char_width('Ａ'), 2); // fullwidth A
        assert_eq!(char_width('\u{301}'), 0);
        assert_eq!(char_width('\u{200D}'), 0);
        assert_eq!(char_width('\x07'), 0);
        assert_eq!(char_width('😀'), 2);
        assert_eq!(char_width('ｱ'), 1); // halfwidth katakana
        assert_eq!(str_width("a中\u{301}b"), 4);
    }

    #[test]
    fn prompt_markers_and_escapes() {
        let p = parse_prompt("\x01\x1b[32m\x02user\x01\x1b[0m\x02$ ");
        assert_eq!(p.head, "");
        assert_eq!(p.glyphs.len(), 6);
        assert!(!p.last.contains('\x01') && !p.last.contains('\x02'));
        assert_eq!(p.last, "\x1b[32muser\x1b[0m$ ");

        // Escapes without markers are still zero width.
        let p = parse_prompt("\x1b[1;34m~\x1b[0m$ ");
        assert_eq!(p.glyphs, ws(3));
        // OSC terminated by BEL and by ST.
        assert_eq!(parse_prompt("\x1b]0;title\x07$ ").glyphs, ws(2));
        assert_eq!(parse_prompt("\x1b]0;t\x1b\\$ ").glyphs, ws(2));
        // Wide chars in prompt.
        assert_eq!(parse_prompt("中> ").glyphs, vec![Glyph::W(2), Glyph::W(1), Glyph::W(1)]);
    }

    #[test]
    fn prompt_multiline() {
        let p = parse_prompt("\x1b[31mline1\nline2 $ ");
        assert_eq!(p.head, "\x1b[31mline1\r\n");
        assert_eq!(p.last, "\x1b[31mline2 $ "); // SGR carried over
        assert_eq!(p.glyphs, ws(8));
        let p = parse_prompt("a\r\nb\n\x01\x1b]0;x\x07\x02$ ");
        assert_eq!(p.head, "a\r\nb\r\n");
        assert_eq!(p.last, "\x1b]0;x\x07$ ");
        assert_eq!(p.glyphs, ws(2));
        let p = parse_prompt("x\ry");
        assert_eq!(p.glyphs, vec![Glyph::W(1), Glyph::Cr, Glyph::W(1)]);
    }

    #[test]
    fn markers_stripped() {
        assert_eq!(strip_markers("\x01\x1b[1m\x02$ "), "\x1b[1m$ ");
    }

    #[test]
    fn lcp() {
        assert_eq!(common_prefix(["foobar", "foobaz", "foo"].into_iter()), "foo");
        assert_eq!(common_prefix(["é1", "é2"].into_iter()), "é");
        assert_eq!(common_prefix(["é", "è"].into_iter()), ""); // share a UTF-8 lead byte only
        assert_eq!(common_prefix(["abc"].into_iter()), "abc");
        assert_eq!(common_prefix(std::iter::empty()), "");
    }

    #[test]
    fn word_motion() {
        let s = "foo bar-baz  qux";
        assert_eq!(word_left(s, s.len()), 13);
        assert_eq!(word_left(s, 13), 8);
        assert_eq!(word_left(s, 8), 4);
        assert_eq!(word_left(s, 2), 0);
        assert_eq!(word_right(s, 0), 3);
        assert_eq!(word_right(s, 3), 7);
        assert_eq!(word_right(s, 7), 11);
        assert_eq!(word_right(s, 11), 16);
        assert_eq!(word_right(s, 16), 16);
        let s = "ls -la /tmp/x ";
        assert_eq!(unix_word_left(s, s.len()), 7);
        assert_eq!(unix_word_left(s, 7), 3);
        assert_eq!(unix_word_left("abc", 0), 0);
        // Combining marks stay with their base char.
        let s = "cafe\u{301} x";
        assert_eq!(word_right(s, 0), 6);
    }

    #[test]
    fn clusters() {
        let s = "e\u{301}x";
        assert_eq!(next_boundary(s, 0), 3);
        assert_eq!(prev_boundary(s, 3), 0);
        assert_eq!(next_boundary(s, 3), 4);
        assert_eq!(prev_boundary(s, 4), 3);
        assert_eq!(next_boundary(s, 4), 4);
        assert_eq!(prev_boundary("中文", 6), 3);
    }

    #[test]
    fn transpose_chars() {
        let (mut s, mut c) = ("ab".to_string(), 2);
        assert!(transpose(&mut s, &mut c));
        assert_eq!((s.as_str(), c), ("ba", 2));
        let (mut s, mut c) = ("abc".to_string(), 1);
        assert!(transpose(&mut s, &mut c));
        assert_eq!((s.as_str(), c), ("bac", 2));
        let (mut s, mut c) = ("abc".to_string(), 0);
        assert!(!transpose(&mut s, &mut c));
        let (mut s, mut c) = ("a".to_string(), 1);
        assert!(!transpose(&mut s, &mut c));
        let (mut s, mut c) = ("x中".to_string(), 4);
        assert!(transpose(&mut s, &mut c));
        assert_eq!((s.as_str(), c), ("中x", 4));
    }

    #[test]
    fn layout_basic() {
        // 5 chars on 10 cols, cursor at 2.
        let l = layout(&ws(5), 10, 0, 2);
        assert_eq!(l, Layout { cursor: (0, 2), end: (0, 5), pending_wrap: false });
        // Exactly fills the row: pending wrap, cursor on the next row.
        let l = layout(&ws(10), 10, 0, 10);
        assert_eq!(l, Layout { cursor: (1, 0), end: (1, 0), pending_wrap: true });
        // 11 chars wrap.
        let l = layout(&ws(11), 10, 0, 11);
        assert_eq!(l, Layout { cursor: (1, 1), end: (1, 1), pending_wrap: false });
        // Cursor on the first char of the second row.
        let l = layout(&ws(15), 10, 0, 10);
        assert_eq!(l.cursor, (1, 0));
        // Start column offset.
        let l = layout(&ws(7), 10, 3, 7);
        assert_eq!(l, Layout { cursor: (1, 0), end: (1, 0), pending_wrap: true });
        let l = layout(&ws(25), 10, 0, 25);
        assert_eq!(l, Layout { cursor: (2, 5), end: (2, 5), pending_wrap: false });
    }

    #[test]
    fn layout_wide() {
        // 9 narrow + 1 wide on 10 cols: wide char wraps, leaving col 9 empty.
        let mut g = ws(9);
        g.push(Glyph::W(2));
        let l = layout(&g, 10, 0, 9);
        assert_eq!(l.cursor, (1, 0));
        assert_eq!(l.end, (1, 2));
        // 8 narrow + 1 wide exactly fills: pending wrap.
        let mut g = ws(8);
        g.push(Glyph::W(2));
        let l = layout(&g, 10, 0, 9);
        assert_eq!(l, Layout { cursor: (1, 0), end: (1, 0), pending_wrap: true });
        // Combining mark doesn't advance.
        let g = vec![Glyph::W(1), Glyph::W(0), Glyph::W(1)];
        assert_eq!(layout(&g, 10, 0, 3).end, (0, 2));
    }

    #[test]
    fn layout_cr_tab() {
        let g = vec![Glyph::W(1), Glyph::W(1), Glyph::Cr, Glyph::W(1)];
        assert_eq!(layout(&g, 10, 0, 4).end, (0, 1));
        let g = vec![Glyph::W(1), Glyph::Tab, Glyph::W(1)];
        assert_eq!(layout(&g, 20, 0, 3).end, (0, 9));
    }

    #[test]
    fn render_controls() {
        let mut out = String::new();
        let mut g = Vec::new();
        render_char('\t', &mut out, &mut g);
        render_char('\x7f', &mut out, &mut g);
        render_char('中', &mut out, &mut g);
        assert_eq!(out, "^I^?中");
        assert_eq!(g, vec![Glyph::W(1), Glyph::W(1), Glyph::W(1), Glyph::W(1), Glyph::W(2)]);
    }

    #[test]
    fn keys() {
        use std::io::Write;
        let (mut r, mut w) = std::io::pipe().unwrap();
        w.write_all(b"a\x01\x7f\x1b[D\x1b[1;5C\x1b[3~\x1b[1;3D\r\x1bb\x1b[Z\xc3\xa9").unwrap();
        drop(w);
        let mut q = InputQueue::new(std::os::fd::AsRawFd::as_raw_fd(&r));
        let mut got = Vec::new();
        while let Some(k) = q.next_key() {
            got.push(k);
        }
        assert_eq!(
            got,
            vec![
                Key::Char('a'),
                Key::Ctrl('a'),
                Key::Backspace,
                Key::Left,
                Key::WordRight,
                Key::Delete,
                Key::WordLeft,
                Key::Enter,
                Key::Alt('b'),
                Key::Ignore,
                Key::Char('é'),
            ]
        );
    }

    #[test]
    fn history_search() {
        let h: Vec<String> = ["git status", "ls -la", "git commit", "echo hi", "git commit"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(hist_find(&h, "git", 4, true, None), Some((4, 0)));
        assert_eq!(hist_find(&h, "git", 3, true, None), Some((2, 0)));
        // Skip duplicates of the current match.
        assert_eq!(hist_find(&h, "git", 3, true, Some("git commit")), Some((0, 0)));
        assert_eq!(hist_find(&h, "nope", 4, true, None), None);
        assert_eq!(hist_find(&h, "l", 4, true, None), Some((1, 4))); // rightmost match
        assert_eq!(hist_find(&h, "git", 1, false, None), Some((2, 0)));
        assert_eq!(hist_find(&h, "", 4, true, None), None);
    }

    #[test]
    fn last_words() {
        assert_eq!(last_word("ls -la /tmp  "), Some("/tmp"));
        assert_eq!(last_word("   "), None);
    }

    #[test]
    fn columns() {
        let items: Vec<String> = (0..10).map(|i| format!("item{i}")).collect();
        let out = format_columns(&items, 20);
        // colw = 7, ncols = (20+2)/7 = 3, nrows = 4
        let rows: Vec<&str> = out.split("\r\n").filter(|r| !r.is_empty()).collect();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0], "item0  item4  item8");
        assert_eq!(rows[3], "item3  item7");
        for r in &rows {
            assert!(str_width(r) <= 20);
        }
        let out = format_columns(&["averyveryverylongname".to_string(), "b".to_string()], 10);
        assert_eq!(out, "averyveryverylongname\r\nb\r\n");
    }

    #[test]
    fn history_cap() {
        let mut le = LineEditor::new();
        le.set_max_history(3);
        for i in 0..5 {
            le.add_history(&i.to_string());
        }
        assert_eq!(le.history(), &["2", "3", "4"]);
        le.set_max_history(2);
        assert_eq!(le.history(), &["3", "4"]);
        le.set_history(vec!["a".into(), "b".into(), "c".into()]);
        assert_eq!(le.history(), &["b", "c"]);
        le.set_max_history(0);
        le.add_history("x");
        assert!(le.history().is_empty());
    }
}

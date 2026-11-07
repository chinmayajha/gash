//! Minimal POSIX FFI (no external crates): terminals, signals, processes, CPU time.

use std::sync::atomic::{AtomicBool, Ordering};

/// A file descriptor. The shell's handle table is keyed by the same numbers.
pub type HANDLE = i32;
pub type DWORD = u32;

pub const STD_INPUT_HANDLE: DWORD = 0;
pub const STD_OUTPUT_HANDLE: DWORD = 1;
pub const STD_ERROR_HANDLE: DWORD = 2;
pub const FILE_TYPE_DISK: DWORD = 1;
const FILE_TYPE_CHAR: DWORD = 2;
const FILE_TYPE_PIPE: DWORD = 3;

type CInt = i32;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Termios {
    c_iflag: u64,
    c_oflag: u64,
    c_cflag: u64,
    c_lflag: u64,
    c_cc: [u8; 20],
    c_ispeed: u64,
    c_ospeed: u64,
}

// c_lflag / c_iflag bits (macOS sys/termios.h).
const ISIG: u64 = 0x0000_0080;
const ICANON: u64 = 0x0000_0100;
const ECHO: u64 = 0x0000_0008;
const IEXTEN: u64 = 0x0000_0400;
const IXON: u64 = 0x0000_0200;
const ICRNL: u64 = 0x0000_0100;
const INLCR: u64 = 0x0000_0040;
const IGNCR: u64 = 0x0000_0080;
const BRKINT: u64 = 0x0000_0002;
const ISTRIP: u64 = 0x0000_0020;
const VMIN: usize = 16;
const VTIME: usize = 17;
const TCSANOW: CInt = 0;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Winsize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

// `struct stat` as the compiler lays it out on 64-bit macOS.
#[repr(C)]
struct Stat {
    st_dev: i32,
    st_mode: u16,
    st_nlink: u16,
    st_ino: u64,
    st_uid: u32,
    st_gid: u32,
    st_rdev: i32,
    st_atime: i64,
    st_atime_nsec: i64,
    st_mtime: i64,
    st_mtime_nsec: i64,
    st_ctime: i64,
    st_ctime_nsec: i64,
    st_birthtime: i64,
    st_birthtime_nsec: i64,
    st_size: i64,
    st_blocks: i64,
    st_blksize: i32,
    st_flags: u32,
    st_gen: u32,
    st_lspare: i32,
    st_qspare: [i64; 2],
}

const S_IFMT: u16 = 0o170000;
const S_IFREG: u16 = 0o100000;
const S_IFCHR: u16 = 0o020000;
const S_IFBLK: u16 = 0o060000;
const S_IFIFO: u16 = 0o010000;
const S_IFSOCK: u16 = 0o140000;

#[repr(C)]
struct Timeval {
    tv_sec: i64,
    tv_usec: i32,
    _pad: i32,
}

#[repr(C)]
struct Rusage {
    ru_utime: Timeval,
    ru_stime: Timeval,
    _rest: [i64; 14],
}

const RUSAGE_SELF: CInt = 0;
const RUSAGE_CHILDREN: CInt = -1;

#[repr(C)]
struct PollFd {
    fd: CInt,
    events: i16,
    revents: i16,
}
const POLLIN: i16 = 0x0001;

const SIGINT: CInt = 2;

#[repr(C)]
struct SigAction {
    handler: usize,
    mask: u32,
    flags: CInt,
}

#[repr(C)]
struct Passwd {
    pw_name: *mut core::ffi::c_char,
    pw_passwd: *mut core::ffi::c_char,
    pw_uid: u32,
    pw_gid: u32,
    _rest: [u64; 8],
}

#[cfg(target_os = "macos")]
const TIOCGWINSZ: u64 = 0x4008_7468;
#[cfg(not(target_os = "macos"))]
const TIOCGWINSZ: u64 = 0x5413;

extern "C" {
    fn isatty(fd: CInt) -> CInt;
    fn ioctl(fd: CInt, request: u64, ...) -> CInt;
    fn tcgetattr(fd: CInt, t: *mut Termios) -> CInt;
    fn tcsetattr(fd: CInt, action: CInt, t: *const Termios) -> CInt;
    fn fstat(fd: CInt, st: *mut Stat) -> CInt;
    fn read(fd: CInt, buf: *mut core::ffi::c_void, n: usize) -> isize;
    fn write(fd: CInt, buf: *const core::ffi::c_void, n: usize) -> isize;
    fn poll(fds: *mut PollFd, n: u32, timeout: CInt) -> CInt;
    fn close(fd: CInt) -> CInt;
    fn kill(pid: CInt, sig: CInt) -> CInt;
    fn getrusage(who: CInt, usage: *mut Rusage) -> CInt;
    fn gethostname(name: *mut core::ffi::c_char, len: usize) -> CInt;
    fn getuid() -> u32;
    fn getpwuid(uid: u32) -> *mut Passwd;
    fn sigaction(sig: CInt, act: *const SigAction, old: *mut SigAction) -> CInt;
}

fn fstat_fd(fd: CInt) -> Option<Stat> {
    let mut st = Stat {
        st_dev: 0,
        st_mode: 0,
        st_nlink: 0,
        st_ino: 0,
        st_uid: 0,
        st_gid: 0,
        st_rdev: 0,
        st_atime: 0,
        st_atime_nsec: 0,
        st_mtime: 0,
        st_mtime_nsec: 0,
        st_ctime: 0,
        st_ctime_nsec: 0,
        st_birthtime: 0,
        st_birthtime_nsec: 0,
        st_size: 0,
        st_blocks: 0,
        st_blksize: 0,
        st_flags: 0,
        st_gen: 0,
        st_lspare: 0,
        st_qspare: [0; 2],
    };
    (unsafe { fstat(fd, &mut st) } == 0).then_some(st)
}

pub fn std_handle(n: DWORD) -> HANDLE {
    n as HANDLE
}

pub fn is_console(n: DWORD) -> bool {
    unsafe { isatty(n as CInt) != 0 }
}

pub fn file_type(n: DWORD) -> DWORD {
    match fstat_fd(n as CInt).map(|s| s.st_mode & S_IFMT) {
        Some(S_IFREG) => FILE_TYPE_DISK,
        Some(S_IFCHR) | Some(S_IFBLK) => FILE_TYPE_CHAR,
        Some(S_IFIFO) | Some(S_IFSOCK) => FILE_TYPE_PIPE,
        _ => 0,
    }
}

/// Terminal size (columns, lines) of the first standard stream that has one.
pub fn console_size() -> Option<(usize, usize)> {
    for n in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE, STD_INPUT_HANDLE] {
        if let Some(s) = winsize(n as CInt) {
            return Some(s);
        }
    }
    None
}

pub fn winsize(fd: CInt) -> Option<(usize, usize)> {
    let mut ws = Winsize::default();
    if unsafe { ioctl(fd, TIOCGWINSZ, &mut ws as *mut Winsize) } == 0 && ws.ws_col > 0 {
        Some((ws.ws_col as usize, ws.ws_row as usize))
    } else {
        None
    }
}

pub fn read_fd(fd: CInt, buf: &mut [u8]) -> isize {
    unsafe { read(fd, buf.as_mut_ptr() as *mut _, buf.len()) }
}

pub fn write_fd(fd: CInt, buf: &[u8]) -> isize {
    unsafe { write(fd, buf.as_ptr() as *const _, buf.len()) }
}

/// Wait until stdin has input or `ms` elapses. Returns true if input is ready.
pub fn wait_stdin(ms: u32) -> bool {
    wait_fd(STD_INPUT_HANDLE as CInt, ms)
}

pub fn close_fd(fd: CInt) {
    unsafe { close(fd) };
}

pub fn wait_fd(fd: CInt, ms: u32) -> bool {
    let mut p = PollFd { fd, events: POLLIN, revents: 0 };
    unsafe { poll(&mut p, 1, ms.min(i32::MAX as u32) as CInt) > 0 }
}

pub fn get_termios(fd: CInt) -> Option<Termios> {
    let mut t = Termios::default();
    (unsafe { tcgetattr(fd, &mut t) } == 0).then_some(t)
}

pub fn set_termios(fd: CInt, t: &Termios) -> bool {
    unsafe { tcsetattr(fd, TCSANOW, t) == 0 }
}

/// No line buffering, no echo, no signal keys: every byte reaches the editor.
pub fn raw_termios(t: &mut Termios) {
    t.c_lflag &= !(ICANON | ECHO | ISIG | IEXTEN);
    t.c_iflag &= !(IXON | ICRNL | INLCR | IGNCR | BRKINT | ISTRIP);
    t.c_cc[VMIN] = 1;
    t.c_cc[VTIME] = 0;
}

/// Canonical input without echo (the `read -s` case).
pub fn no_echo_termios(t: &mut Termios) {
    t.c_lflag &= !ECHO;
}

/// Terminal title, as an xterm OSC sequence. A no-op when stdout is not a terminal.
pub fn set_title(t: &str) {
    if is_console(STD_OUTPUT_HANDLE) {
        let s = format!("\x1b]0;{}\x07", t);
        let _ = write_fd(STD_OUTPUT_HANDLE as CInt, s.as_bytes());
    }
}

pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: CInt) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Take Ctrl+C ourselves so a running command can be cut short and the shell stays alive.
pub fn install_ctrl_handler() {
    let act = SigAction { handler: on_sigint as *const () as usize, mask: 0, flags: 0 };
    unsafe {
        sigaction(SIGINT, &act, std::ptr::null_mut());
    }
}

pub fn clear_ctrl_c_ignore() {
    // Installing our own handler above already replaces an inherited SIG_IGN.
}

pub fn take_interrupt() -> bool {
    INTERRUPTED.swap(false, Ordering::SeqCst)
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// Whether this process has a terminal on any standard stream.
pub fn has_console() -> bool {
    is_console(STD_INPUT_HANDLE) || is_console(STD_OUTPUT_HANDLE) || is_console(STD_ERROR_HANDLE)
}

/// Send signal `sig` to `pid`. The exit status bash reports (128+sig) is built by the caller.
pub fn kill_pid(pid: u32, sig: u32) -> bool {
    unsafe { kill(pid as CInt, sig as CInt) == 0 }
}

pub fn pid_alive(pid: u32) -> bool {
    if unsafe { kill(pid as CInt, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(1) // EPERM: exists, not ours
}

fn rusage(who: CInt) -> (f64, f64) {
    let mut r = Rusage { ru_utime: Timeval { tv_sec: 0, tv_usec: 0, _pad: 0 }, ru_stime: Timeval { tv_sec: 0, tv_usec: 0, _pad: 0 }, _rest: [0; 14] };
    if unsafe { getrusage(who, &mut r) } != 0 {
        return (0.0, 0.0);
    }
    let f = |t: &Timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    (f(&r.ru_utime), f(&r.ru_stime))
}

/// (user, system) CPU seconds of this process.
pub fn process_times() -> (f64, f64) {
    rusage(RUSAGE_SELF)
}

/// (user, system) CPU seconds of this process plus waited-for children.
pub fn total_times() -> (f64, f64) {
    let (u1, s1) = rusage(RUSAGE_SELF);
    let (u2, s2) = rusage(RUSAGE_CHILDREN);
    (u1 + u2, s1 + s2)
}

pub fn user_name() -> String {
    if let Ok(u) = std::env::var("USER") {
        if !u.is_empty() {
            return u;
        }
    }
    unsafe {
        let pw = getpwuid(getuid());
        if !pw.is_null() && !(*pw).pw_name.is_null() {
            return std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned();
        }
    }
    "user".into()
}

pub fn host_name() -> String {
    let mut buf = [0u8; 256];
    if unsafe { gethostname(buf.as_mut_ptr() as *mut _, buf.len()) } == 0 {
        if let Some(n) = buf.iter().position(|&b| b == 0) {
            return String::from_utf8_lossy(&buf[..n]).into_owned();
        }
    }
    "localhost".into()
}

// The Windows shell needed these; on Unix the terminal already handles them.
pub fn enable_vt_output() {}
pub fn make_append_only_stdio_writable() {}
pub fn seek_std_append(_n: DWORD) {}
pub fn set_utf8_console() -> Option<()> {
    None
}

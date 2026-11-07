//! Minimal POSIX FFI (no external crates): terminals, signals, processes, limits.

use std::ffi::{CStr, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub type Fd = i32;

pub const STDIN_FD: Fd = 0;
pub const STDOUT_FD: Fd = 1;
pub const STDERR_FD: Fd = 2;

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
    pw_change: i64,
    pw_class: *mut core::ffi::c_char,
    pw_gecos: *mut core::ffi::c_char,
    pw_dir: *mut core::ffi::c_char,
    pw_shell: *mut core::ffi::c_char,
    pw_expire: i64,
}

#[repr(C)]
struct RLimit {
    cur: u64,
    max: u64,
}

#[repr(C)]
struct Utsname {
    sysname: [u8; 256],
    nodename: [u8; 256],
    release: [u8; 256],
    version: [u8; 256],
    machine: [u8; 256],
}

const TIOCGWINSZ: u64 = 0x4008_7468;

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
    fn geteuid() -> u32;
    fn getegid() -> u32;
    fn getppid() -> CInt;
    fn getpwuid(uid: u32) -> *mut Passwd;
    fn getpwnam(name: *const core::ffi::c_char) -> *mut Passwd;
    fn sigaction(sig: CInt, act: *const SigAction, old: *mut SigAction) -> CInt;
    fn pthread_sigmask(how: CInt, set: *const u32, old: *mut u32) -> CInt;
    fn umask(mask: u32) -> u32;
    fn access(path: *const core::ffi::c_char, mode: CInt) -> CInt;
    fn mkfifo(path: *const core::ffi::c_char, mode: u32) -> CInt;
    fn getrlimit(res: CInt, lim: *mut RLimit) -> CInt;
    fn setrlimit(res: CInt, lim: *const RLimit) -> CInt;
    fn uname(u: *mut Utsname) -> CInt;
    fn waitpid(pid: CInt, status: *mut CInt, options: CInt) -> CInt;
}

fn fstat_fd(fd: CInt) -> Option<Stat> {
    let mut st = unsafe { std::mem::zeroed::<Stat>() };
    (unsafe { fstat(fd, &mut st) } == 0).then_some(st)
}

pub fn is_tty(fd: Fd) -> bool {
    unsafe { isatty(fd) != 0 }
}

pub fn is_regular_file(fd: Fd) -> bool {
    fstat_fd(fd).map(|s| s.st_mode & S_IFMT == S_IFREG).unwrap_or(false)
}

/// Terminal size (columns, lines) of the first standard stream that has one.
pub fn terminal_size() -> Option<(usize, usize)> {
    [STDOUT_FD, STDERR_FD, STDIN_FD].into_iter().find_map(winsize)
}

pub fn winsize(fd: Fd) -> Option<(usize, usize)> {
    let mut ws = Winsize::default();
    if unsafe { ioctl(fd, TIOCGWINSZ, &mut ws as *mut Winsize) } == 0 && ws.ws_col > 0 {
        Some((ws.ws_col as usize, ws.ws_row as usize))
    } else {
        None
    }
}

pub fn read_fd(fd: Fd, buf: &mut [u8]) -> isize {
    unsafe { read(fd, buf.as_mut_ptr() as *mut _, buf.len()) }
}

pub fn write_fd(fd: Fd, buf: &[u8]) -> isize {
    unsafe { write(fd, buf.as_ptr() as *const _, buf.len()) }
}

/// Wait until stdin has input or `ms` elapses. Returns true if input is ready.
pub fn wait_stdin(ms: u32) -> bool {
    wait_fd(STDIN_FD, ms)
}

pub fn close_fd(fd: Fd) {
    unsafe { close(fd) };
}

pub fn wait_fd(fd: Fd, ms: u32) -> bool {
    let mut p = PollFd { fd, events: POLLIN, revents: 0 };
    unsafe { poll(&mut p, 1, ms.min(i32::MAX as u32) as CInt) > 0 }
}

pub fn get_termios(fd: Fd) -> Option<Termios> {
    let mut t = Termios::default();
    (unsafe { tcgetattr(fd, &mut t) } == 0).then_some(t)
}

pub fn set_termios(fd: Fd, t: &Termios) -> bool {
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

pub const SIGNALS: &[(&str, i32)] = &[
    ("HUP", 1),
    ("INT", 2),
    ("QUIT", 3),
    ("ILL", 4),
    ("TRAP", 5),
    ("ABRT", 6),
    ("EMT", 7),
    ("FPE", 8),
    ("KILL", 9),
    ("BUS", 10),
    ("SEGV", 11),
    ("SYS", 12),
    ("PIPE", 13),
    ("ALRM", 14),
    ("TERM", 15),
    ("URG", 16),
    ("STOP", 17),
    ("TSTP", 18),
    ("CONT", 19),
    ("CHLD", 20),
    ("TTIN", 21),
    ("TTOU", 22),
    ("IO", 23),
    ("XCPU", 24),
    ("XFSZ", 25),
    ("VTALRM", 26),
    ("PROF", 27),
    ("WINCH", 28),
    ("INFO", 29),
    ("USR1", 30),
    ("USR2", 31),
];

pub const SIGHUP: i32 = 1;
pub const SIGINT: i32 = 2;
pub const SIGQUIT: i32 = 3;
pub const SIGTERM: i32 = 15;
pub const SIGCHLD: i32 = 20;
pub const SIGWINCH: i32 = 28;

const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

pub fn signal_name(n: i32) -> Option<&'static str> {
    SIGNALS.iter().find(|(_, v)| *v == n).map(|(k, _)| *k)
}

pub fn signal_number(name: &str) -> Option<i32> {
    SIGNALS.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
}

pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static PENDING: AtomicU64 = AtomicU64::new(0);
static INTERACTIVE: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(sig: CInt) {
    if sig == SIGINT {
        INTERRUPTED.store(true, Ordering::SeqCst);
    } else {
        PENDING.fetch_or(1 << sig, Ordering::SeqCst);
    }
}

extern "C" fn on_signal_ignored(_: CInt) {}

fn set_handler(sig: i32, handler: usize) {
    let act = SigAction { handler, mask: 0, flags: 0 };
    unsafe {
        sigaction(sig, &act, std::ptr::null_mut());
    }
}

/// How a signal reaches the shell: `Catch` queues it for the interpreter, `Ignore` drops it
/// (and children inherit that), `Default` restores the shell's own baseline for the signal.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SignalMode {
    Catch,
    Ignore,
    Default,
}

pub fn set_signal(sig: i32, mode: SignalMode) {
    let interactive = INTERACTIVE.load(Ordering::SeqCst);
    let handler = match mode {
        SignalMode::Catch => on_signal as *const () as usize,
        SignalMode::Ignore => SIG_IGN,
        SignalMode::Default => match sig {
            SIGINT | SIGHUP => on_signal as *const () as usize,
            SIGTERM | SIGQUIT if interactive => on_signal_ignored as *const () as usize,
            SIGTERM => on_signal as *const () as usize,
            _ => SIG_DFL,
        },
    };
    set_handler(sig, handler);
}

/// Take Ctrl+C and hangup ourselves so a running command can be cut short and the shell can
/// leave through its normal exit path. An interactive shell also ignores the job-control
/// stop signals, because it has no job control.
static INTERPRETER_THREAD: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();

pub fn on_interpreter_thread() -> bool {
    INTERPRETER_THREAD.get() == Some(&std::thread::current().id())
}

pub fn first_pending() -> Option<i32> {
    let bits = PENDING.load(Ordering::SeqCst);
    (1..64).find(|s| bits & (1 << s) != 0)
}

pub fn install_signal_handlers(interactive: bool) {
    let _ = INTERPRETER_THREAD.set(std::thread::current().id());
    INTERACTIVE.store(interactive, Ordering::SeqCst);
    for sig in [SIGINT, SIGHUP, SIGTERM, SIGQUIT] {
        if interactive || sig != SIGQUIT {
            set_signal(sig, SignalMode::Default);
        }
    }
    if interactive {
        for sig in [18, 21, 22] {
            set_handler(sig, SIG_IGN);
        }
    }
}

pub fn block_catchable_signals_here() {
    let set = [SIGINT, SIGHUP, SIGTERM, SIGQUIT].iter().fold(0u32, |m, s| m | 1 << (s - 1));
    unsafe {
        pthread_sigmask(1, &set, std::ptr::null_mut());
    }
}

pub fn take_interrupt() -> bool {
    INTERRUPTED.swap(false, Ordering::SeqCst)
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

pub fn signal_pending() -> bool {
    PENDING.load(Ordering::SeqCst) != 0
}

/// Signals received since the last call, in ascending order.
pub fn take_pending() -> Vec<i32> {
    let bits = PENDING.swap(0, Ordering::SeqCst);
    (1..64).filter(|s| bits & (1 << s) != 0).collect()
}

/// Send signal `sig` to `pid`; a negative `pid` addresses a process group.
pub fn kill_pid(pid: i32, sig: i32) -> bool {
    unsafe { kill(pid, sig) == 0 }
}

pub fn pid_alive(pid: i32) -> bool {
    if unsafe { kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(1)
}

pub enum WaitOutcome {
    Exited(i32),
    Interrupted,
    Failed,
}

/// Block until child `pid` ends, or until a signal wakes the wait.
pub fn wait_pid(pid: i32) -> WaitOutcome {
    let mut status: CInt = 0;
    if unsafe { waitpid(pid, &mut status, 0) } == pid {
        return WaitOutcome::Exited(status);
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(4) => WaitOutcome::Interrupted,
        _ => WaitOutcome::Failed,
    }
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

pub fn user_id() -> u32 {
    unsafe { getuid() }
}

pub fn effective_user_id() -> u32 {
    unsafe { geteuid() }
}

pub fn effective_group_id() -> u32 {
    unsafe { getegid() }
}

pub fn parent_pid() -> u32 {
    unsafe { getppid() as u32 }
}

unsafe fn cstr(p: *const core::ffi::c_char) -> Option<String> {
    (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
}

pub fn user_name() -> String {
    if let Ok(u) = std::env::var("USER") {
        if !u.is_empty() {
            return u;
        }
    }
    unsafe {
        let pw = getpwuid(getuid());
        if !pw.is_null() {
            if let Some(n) = cstr((*pw).pw_name) {
                return n;
            }
        }
    }
    "user".into()
}

pub fn user_home(user: &str) -> Option<String> {
    let name = CString::new(user).ok()?;
    unsafe {
        let pw = getpwnam(name.as_ptr());
        if pw.is_null() {
            None
        } else {
            cstr((*pw).pw_dir)
        }
    }
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

/// Kernel release as `uname -r` prints it, for example `24.5.0`.
pub fn os_release() -> String {
    let mut u = unsafe { std::mem::zeroed::<Utsname>() };
    if unsafe { uname(&mut u) } != 0 {
        return String::new();
    }
    let end = u.release.iter().position(|&b| b == 0).unwrap_or(u.release.len());
    String::from_utf8_lossy(&u.release[..end]).into_owned()
}

pub fn cpu_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    }
}

/// The process umask at startup. Read it before any thread starts: reading means setting it.
pub fn startup_umask() -> u32 {
    static MASK: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MASK.get_or_init(|| unsafe {
        let m = umask(0o22);
        umask(m);
        m
    })
}

pub fn set_process_umask(mask: u32) {
    unsafe {
        umask(mask);
    }
}

pub const R_OK: i32 = 4;
pub const W_OK: i32 = 2;
pub const X_OK: i32 = 1;

pub fn can_access(path: &Path, mode: i32) -> bool {
    match CString::new(path.as_os_str().as_encoded_bytes()) {
        Ok(c) => unsafe { access(c.as_ptr(), mode) == 0 },
        Err(_) => false,
    }
}

pub fn make_fifo(path: &Path) -> std::io::Result<()> {
    let c = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    if unsafe { mkfifo(c.as_ptr(), 0o600) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub const RLIM_INFINITY: u64 = i64::MAX as u64;
pub const RLIMIT_CPU: i32 = 0;
pub const RLIMIT_FSIZE: i32 = 1;
pub const RLIMIT_DATA: i32 = 2;
pub const RLIMIT_STACK: i32 = 3;
pub const RLIMIT_CORE: i32 = 4;
pub const RLIMIT_RSS: i32 = 5;
pub const RLIMIT_MEMLOCK: i32 = 6;
pub const RLIMIT_NPROC: i32 = 7;
pub const RLIMIT_NOFILE: i32 = 8;

/// (soft, hard) limit for resource `res`.
pub fn get_rlimit(res: i32) -> Option<(u64, u64)> {
    let mut l = RLimit { cur: 0, max: 0 };
    (unsafe { getrlimit(res, &mut l) } == 0).then_some((l.cur, l.max))
}

/// Lowest descriptor number the process may not use, from the open-files limit.
pub fn max_fds() -> i32 {
    get_rlimit(RLIMIT_NOFILE).map(|(soft, _)| soft.min(i32::MAX as u64) as i32).unwrap_or(i32::MAX)
}

pub fn set_rlimit(res: i32, soft: u64, hard: u64) -> bool {
    let l = RLimit { cur: soft, max: hard };
    unsafe { setrlimit(res, &l) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn ffi_layouts() {
        assert_eq!(size_of::<Termios>(), 72);
        assert_eq!(size_of::<Winsize>(), 8);
        assert_eq!(size_of::<Stat>(), 144);
        assert_eq!(size_of::<Rusage>(), 144);
        assert_eq!(size_of::<Passwd>(), 72);
        assert_eq!(size_of::<RLimit>(), 16);
        assert_eq!(size_of::<Utsname>(), 1280);
    }

    #[test]
    fn signal_table_is_consistent() {
        assert_eq!(signal_number("USR1"), Some(30));
        assert_eq!(signal_name(20), Some("CHLD"));
        assert!(SIGNALS.windows(2).all(|w| w[0].1 + 1 == w[1].1));
    }
}

//! File-descriptor table and handle abstraction. The shell never relies on process-wide
//! stdio state, so subshells can run in threads with their own fd tables.

use crate::sys;
use std::fs::File;
use std::io::{self, BufRead, PipeReader, PipeWriter, Read, Seek, SeekFrom, Write};
use std::mem::ManuallyDrop;
use std::os::unix::io::{AsFd, FromRawFd};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Here-document and here-string input, held in memory until a child needs it as a pipe or file.
#[derive(Debug, Default)]
pub struct MemReader {
    data: Vec<u8>,
    pos: usize,
    /// Where the input went once a child needed it.
    shared: Option<Handle>,
}

/// Input up to this size goes through a pipe, as in bash; larger input goes in a file.
const HEREDOC_PIPE_MAX: usize = 64 * 1024;

impl MemReader {
    pub fn new(data: Vec<u8>) -> MemReader {
        MemReader { data, pos: 0, shared: None }
    }

    /// The pipe or file holding the unread input, creating it on first use.
    fn shared(&mut self) -> io::Result<Handle> {
        if let Some(h) = &self.shared {
            return Ok(h.clone());
        }
        let h = if self.data.len() - self.pos <= HEREDOC_PIPE_MAX {
            let (r, mut w) = io::pipe()?;
            let unread = self.data[self.pos..].to_vec();
            std::thread::spawn(move || {
                let _ = w.write_all(&unread);
            });
            Handle::PipeR(Arc::new(r))
        } else {
            let mut f = temp_file()?;
            f.write_all(&self.data)?;
            f.seek(SeekFrom::Start(self.pos as u64))?;
            Handle::File(Arc::new(f))
        };
        self.data = Vec::new();
        self.pos = 0;
        self.shared = Some(h.clone());
        Ok(h)
    }
}

/// A temporary file with no directory entry; it lives while a descriptor to it is open.
fn temp_file() -> io::Result<File> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mut last = None;
    for _ in 0..16 {
        let name = format!(
            "gash-heredoc-{}-{}-{:08x}.tmp",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
            nanos
        );
        match std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(dir.join(&name)) {
            Ok(f) => {
                let _ = std::fs::remove_file(dir.join(&name));
                return Ok(f);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::new(io::ErrorKind::Other, "cannot create temporary file")))
}

#[derive(Clone)]
pub enum Handle {
    Stdin,
    Stdout,
    Stderr,
    File(Arc<File>),
    /// A file opened with `>>`; `O_APPEND` makes every write atomic.
    Append(Arc<File>),
    PipeR(Arc<PipeReader>),
    PipeW(Arc<PipeWriter>),
    Buf(Arc<Mutex<Vec<u8>>>),
    Mem(Arc<Mutex<MemReader>>),
    Null,
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Handle::Stdin => "stdin",
            Handle::Stdout => "stdout",
            Handle::Stderr => "stderr",
            Handle::File(_) | Handle::Append(_) => "file",
            Handle::PipeR(_) => "pipe-r",
            Handle::PipeW(_) => "pipe-w",
            Handle::Buf(_) => "buf",
            Handle::Mem(_) => "mem",
            Handle::Null => "null",
        };
        f.write_str(s)
    }
}

fn raw_std(fd: sys::Fd) -> ManuallyDrop<File> {
    unsafe { ManuallyDrop::new(File::from_raw_fd(fd)) }
}

pub fn console_stdin() -> bool {
    sys::is_tty(sys::STDIN_FD)
}

/// Write to the process stdout or stderr unbuffered, so the order matches what children write.
fn write_std(err: bool, data: &[u8]) -> io::Result<()> {
    let f = raw_std(if err { sys::STDERR_FD } else { sys::STDOUT_FD });
    (&*f).write_all(data)
}

impl Handle {
    pub fn write_all(&self, data: &[u8]) -> io::Result<()> {
        match self {
            Handle::Stdout => write_std(false, data),
            Handle::Stderr => write_std(true, data),
            Handle::File(f) => (&**f).write_all(data),
            Handle::Append(f) => (&**f).write_all(data),
            Handle::PipeW(p) => (&**p).write_all(data),
            Handle::Buf(b) => {
                b.lock().unwrap().extend_from_slice(data);
                Ok(())
            }
            Handle::Null => Ok(()),
            Handle::Stdin | Handle::PipeR(_) | Handle::Mem(_) => {
                Err(io::Error::new(io::ErrorKind::Other, "Bad file descriptor"))
            }
        }
    }

    pub fn is_console(&self) -> bool {
        match self {
            Handle::Stdin => sys::is_tty(sys::STDIN_FD),
            Handle::Stdout => sys::is_tty(sys::STDOUT_FD),
            Handle::Stderr => sys::is_tty(sys::STDERR_FD),
            _ => false,
        }
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            // Unbuffered: a buffered stdin would read past what this call was asked for
            // and swallow input meant for a child or the next command.
            Handle::Stdin => {
                let f = raw_std(sys::STDIN_FD);
                (&*f).read(buf)
            }
            Handle::File(f) => (&**f).read(buf),
            Handle::PipeR(p) => match (&**p).read(buf) {
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(0),
                r => r,
            },
            Handle::Mem(m) => {
                let mut m = m.lock().unwrap();
                if let Some(h) = m.shared.clone() {
                    drop(m);
                    return h.read(buf);
                }
                let n = buf.len().min(m.data.len() - m.pos);
                let p = m.pos;
                buf[..n].copy_from_slice(&m.data[p..p + n]);
                m.pos += n;
                Ok(n)
            }
            Handle::Null => Ok(0),
            _ => Err(io::Error::new(io::ErrorKind::Other, "Bad file descriptor")),
        }
    }

    pub fn read_to_end(&self) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut buf = vec![0u8; 65536];
        loop {
            let n = self.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        Ok(out)
    }

    /// Read up to and including `delim` without consuming input beyond it (so that
    /// child processes sharing the handle see the rest). Returns (data without delim, found_delim).
    pub fn read_until(&self, delim: u8, max: Option<usize>) -> io::Result<(Vec<u8>, bool)> {
        let mut out = Vec::new();
        let limit = max.unwrap_or(usize::MAX);
        match self {
            Handle::Mem(m) => {
                let mut m = m.lock().unwrap();
                if let Some(h) = m.shared.clone() {
                    drop(m);
                    return h.read_until(delim, max);
                }
                while m.pos < m.data.len() && out.len() < limit {
                    let b = m.data[m.pos];
                    m.pos += 1;
                    if b == delim {
                        return Ok((out, true));
                    }
                    out.push(b);
                }
                Ok((out, false))
            }
            Handle::File(f) => read_until_seekable(&f, delim, limit),
            Handle::Stdin if console_stdin() => {
                let stdin = io::stdin();
                let mut lock = stdin.lock();
                if max.is_none() {
                    let n = lock.read_until(delim, &mut out)?;
                    if n > 0 && out.last() == Some(&delim) {
                        out.pop();
                        return Ok((out, true));
                    }
                    return Ok((out, false));
                }
                let mut b = [0u8; 1];
                while out.len() < limit {
                    if lock.read(&mut b)? == 0 {
                        return Ok((out, false));
                    }
                    if b[0] == delim {
                        return Ok((out, true));
                    }
                    out.push(b[0]);
                }
                Ok((out, false))
            }
            Handle::Stdin if sys::is_regular_file(sys::STDIN_FD) => {
                let f = raw_std(sys::STDIN_FD);
                read_until_seekable(&f, delim, limit)
            }
            _ => {
                let mut b = [0u8; 1];
                while out.len() < limit {
                    if self.read(&mut b)? == 0 {
                        return Ok((out, false));
                    }
                    if b[0] == delim {
                        return Ok((out, true));
                    }
                    out.push(b[0]);
                }
                Ok((out, false))
            }
        }
    }

    /// Convert to a Stdio for a child process. Returns an optional helper thread
    /// (pump) that must be joined after the child exits.
    pub fn to_stdio(&self, fd: i32) -> io::Result<(Stdio, Option<JoinHandle<()>>)> {
        Ok(match self {
            Handle::Stdin => {
                if fd == 0 {
                    (Stdio::inherit(), None)
                } else {
                    (dup_std(io::stdin().as_fd())?, None)
                }
            }
            Handle::Stdout => {
                if fd == 1 {
                    (Stdio::inherit(), None)
                } else {
                    (dup_std(io::stdout().as_fd())?, None)
                }
            }
            Handle::Stderr => {
                if fd == 2 {
                    (Stdio::inherit(), None)
                } else {
                    (dup_std(io::stderr().as_fd())?, None)
                }
            }
            Handle::File(f) => (Stdio::from(f.try_clone()?), None),
            // The descriptor carries O_APPEND, so the child's writes append atomically too.
            Handle::Append(f) => (Stdio::from(f.try_clone()?), None),
            Handle::PipeR(p) => (Stdio::from(p.try_clone()?), None),
            Handle::PipeW(p) => (Stdio::from(p.try_clone()?), None),
            Handle::Null => (Stdio::null(), None),
            Handle::Buf(b) => {
                let (mut r, w) = io::pipe()?;
                let b = b.clone();
                let h = std::thread::spawn(move || {
                    let mut tmp = vec![0u8; 65536];
                    loop {
                        match r.read(&mut tmp) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => b.lock().unwrap().extend_from_slice(&tmp[..n]),
                        }
                    }
                });
                (Stdio::from(w), Some(h))
            }
            Handle::Mem(m) => {
                let mut m = m.lock().unwrap();
                match m.shared() {
                    Ok(h) => h.to_stdio(fd)?,
                    // No pipe or file to share: feed the child through a pump thread
                    // instead. It gets all of the unread input, so none is left for
                    // later readers.
                    Err(_) => {
                        let data = std::mem::take(&mut m.data).split_off(m.pos);
                        m.pos = 0;
                        let (r, mut w) = io::pipe()?;
                        let h = std::thread::spawn(move || {
                            let _ = w.write_all(&data);
                        });
                        (Stdio::from(r), Some(h))
                    }
                }
            }
        })
    }
}

fn dup_std(h: std::os::unix::io::BorrowedFd<'_>) -> io::Result<Stdio> {
    match h.try_clone_to_owned() {
        Ok(o) => Ok(Stdio::from(o)),
        Err(_) => Ok(Stdio::null()),
    }
}

fn read_until_seekable(f: &File, delim: u8, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut f = f;
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let want = chunk.len().min(limit.saturating_sub(out.len()).max(1));
        let n = f.read(&mut chunk[..want])?;
        if n == 0 {
            return Ok((out, false));
        }
        if let Some(i) = chunk[..n].iter().position(|&b| b == delim) {
            out.extend_from_slice(&chunk[..i]);
            let back = (n - i - 1) as i64;
            if back > 0 {
                f.seek(SeekFrom::Current(-back))?;
            }
            return Ok((out, true));
        }
        let take = n.min(limit - out.len());
        out.extend_from_slice(&chunk[..take]);
        if take < n {
            f.seek(SeekFrom::Current(-((n - take) as i64)))?;
        }
        if out.len() >= limit {
            return Ok((out, false));
        }
    }
}

#[derive(Clone)]
pub struct Fds {
    pub v: Vec<Option<Handle>>,
}

impl Fds {
    pub fn std() -> Fds {
        Fds { v: vec![Some(Handle::Stdin), Some(Handle::Stdout), Some(Handle::Stderr)] }
    }
    pub fn get(&self, fd: i32) -> Option<&Handle> {
        if fd < 0 {
            return None;
        }
        self.v.get(fd as usize).and_then(|h| h.as_ref())
    }
    pub fn set(&mut self, fd: i32, h: Option<Handle>) -> Option<Handle> {
        let i = fd as usize;
        if self.v.len() <= i {
            self.v.resize(i + 1, None);
        }
        std::mem::replace(&mut self.v[i], h)
    }
    pub fn free_fd(&self, from: i32) -> i32 {
        let mut fd = from;
        while self.get(fd).is_some() {
            fd += 1;
        }
        fd
    }
}

//! Differential compatibility tests against bash 5.2.
//!
//! Every `tests/scripts/*.sh` is run through the `gash` binary
//! (`gash NNN_topic.sh`, cwd = `tests/scripts`, stdin = null) and its stdout
//! bytes and exit code are compared with `NNN_topic.out` / `NNN_topic.code`.
//! Stderr is captured for diagnostics only.
//!
//! All scripts are run; the test fails once at the end with a summary.
//!
//! Environment knobs:
//! * `GASH_COMPAT_FILTER` - only run scripts whose file name contains this substring.
//! * `GASH_COMPAT_JOBS`   - number of scripts run in parallel (default 1).
//!
//! The child environment is normalized: `LC_ALL=C`, and `LANG`, `LC_*`, `BASH_ENV`, `ENV`,
//! `SHELLOPTS`, `BASHOPTS`, `CDPATH`, `GLOBIGNORE`, `POSIXLY_CORRECT` are removed. A `wc` that
//! prints GNU-style output (no column padding) comes first in `PATH`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(20);
/// How long to keep waiting for stdout/stderr EOF after the process exited
/// (a leaked background process may keep the pipes open).
const PIPE_GRACE: Duration = Duration::from_secs(3);
const MAX_DIFF_LINES: usize = 15;
const MAX_STDERR_LINES: usize = 10;
const MAX_LINE_CHARS: usize = 200;
/// Above this many lines on either side, fall back to a positional diff.
const MAX_LCS_LINES: usize = 3000;

const REMOVED_ENV: &[&str] = &[
    "LANG",
    "BASH_ENV",
    "ENV",
    "SHELLOPTS",
    "BASHOPTS",
    "CDPATH",
    "GLOBIGNORE",
    "POSIXLY_CORRECT",
];

struct RunOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    code: Option<i32>,
    timed_out: bool,
    pipes_left_open: bool,
    spawn_error: Option<String>,
    elapsed: Duration,
}

struct Outcome {
    name: String,
    passed: bool,
    report: String,
}

#[test]
fn compat() {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_gash"));
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("scripts");
    let filter = std::env::var("GASH_COMPAT_FILTER").unwrap_or_default();

    let mut scripts: Vec<String> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".sh") && name.contains(filter.as_str()))
        .collect();
    scripts.sort();
    assert!(
        !scripts.is_empty(),
        "no scripts in {} match GASH_COMPAT_FILTER={filter:?}",
        dir.display()
    );

    let jobs = std::env::var("GASH_COMPAT_JOBS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(1)
        .min(scripts.len());

    let started = Instant::now();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Outcome>>> =
        Mutex::new((0..scripts.len()).map(|_| None).collect());
    thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= scripts.len() {
                        break;
                    }
                    let outcome = check_script(&exe, &dir, &scripts[i]);
                    results.lock().unwrap()[i] = Some(outcome);
                }
            });
        }
    });
    let outcomes: Vec<Outcome> = results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|o| o.expect("every script produces an outcome"))
        .collect();

    let total = outcomes.len();
    let passed = outcomes.iter().filter(|o| o.passed).count();
    let failed: Vec<&Outcome> = outcomes.iter().filter(|o| !o.passed).collect();

    let mut summary = String::new();
    for o in &failed {
        summary.push_str(&o.report);
        summary.push('\n');
    }
    if !failed.is_empty() {
        summary.push_str("failing scripts:");
        for o in &failed {
            summary.push(' ');
            summary.push_str(&o.name);
        }
        summary.push('\n');
    }
    let final_line = format!(
        "passed {passed}/{total} ({:.1}s, {jobs} jobs)",
        started.elapsed().as_secs_f64()
    );
    summary.push_str(&final_line);

    // Written straight to the process stdout so it is visible even when the
    // test harness captures output of passing tests.
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "\n{final_line}");
    let _ = out.flush();
    drop(out);

    if !failed.is_empty() {
        panic!("{} of {total} compat scripts failed\n\n{summary}", failed.len());
    }
}

fn check_script(exe: &Path, dir: &Path, name: &str) -> Outcome {
    let stem = name.strip_suffix(".sh").unwrap_or(name);
    let out_path = dir.join(format!("{stem}.out"));
    let code_path = dir.join(format!("{stem}.code"));

    let expected_out = fs::read(&out_path);
    let expected_code = fs::read_to_string(&code_path)
        .map_err(|e| e.to_string())
        .and_then(|s| s.trim().parse::<i32>().map_err(|e| format!("bad contents {s:?}: {e}")));

    let run = run_script(exe, dir, name);
    let mut report = String::new();
    let mut ok = true;

    if let Some(err) = &run.spawn_error {
        report.push_str(&format!("  could not spawn {}: {err}\n", exe.display()));
        ok = false;
    }
    if run.timed_out {
        report.push_str(&format!("  TIMED OUT after {}s (process killed)\n", TIMEOUT.as_secs()));
        ok = false;
    }
    if run.pipes_left_open {
        report.push_str("  note: stdout/stderr were still open after exit (background process?); output may be truncated\n");
    }

    match &expected_code {
        Ok(want) => {
            if run.code != Some(*want) {
                ok = false;
                let got = run.code.map_or("none (killed?)".to_string(), |c| c.to_string());
                report.push_str(&format!("  exit code: expected {want}, got {got}\n"));
            }
        }
        Err(e) => {
            ok = false;
            report.push_str(&format!("  cannot read {}: {e}\n", code_path.display()));
        }
    }

    match &expected_out {
        Ok(want) => {
            if *want != run.stdout {
                ok = false;
                report.push_str(&stdout_diff(want, &run.stdout));
            }
        }
        Err(e) => {
            ok = false;
            report.push_str(&format!("  cannot read {}: {e}\n", out_path.display()));
        }
    }

    if !ok {
        report.push_str(&stderr_excerpt(&run.stderr));
        report = format!(
            "FAIL {name} ({:.1}s)\n{report}",
            run.elapsed.as_secs_f64()
        );
    }
    Outcome {
        name: name.to_string(),
        passed: ok,
        report,
    }
}

fn run_script(exe: &Path, dir: &Path, name: &str) -> RunOutput {
    let start = Instant::now();
    let mut cmd = Command::new(exe);
    cmd.arg(name)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    for (key, _) in std::env::vars_os() {
        if let Some(k) = key.to_str() {
            let upper = k.to_ascii_uppercase();
            if upper.starts_with("LC_") || REMOVED_ENV.contains(&upper.as_str()) {
                cmd.env_remove(&key);
            }
        }
    }
    cmd.env("LC_ALL", "C");
    cmd.env("PATH", format!("{}:{}", tool_shims().display(), std::env::var("PATH").unwrap_or_default()));

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return RunOutput {
                stdout: Vec::new(),
                stderr: Vec::new(),
                code: None,
                timed_out: false,
                pipes_left_open: false,
                spawn_error: Some(e.to_string()),
                elapsed: start.elapsed(),
            };
        }
    };

    let stdout = PipeReader::start(child.stdout.take().expect("stdout is piped"));
    let stderr = PipeReader::start(child.stderr.take().expect("stderr is piped"));

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if start.elapsed() >= TIMEOUT {
            timed_out = true;
            kill_tree(&mut child);
            break child.wait().ok();
        }
        thread::sleep(Duration::from_millis(5));
    };

    let deadline = Instant::now() + PIPE_GRACE;
    let (stdout, out_open) = stdout.finish(deadline);
    let (stderr, err_open) = stderr.finish(deadline);
    RunOutput {
        stdout,
        stderr,
        code: status.and_then(|s| s.code()),
        timed_out,
        pipes_left_open: out_open || err_open,
        spawn_error: None,
        elapsed: start.elapsed(),
    }
}

/// Kills the child's process group, so an orphaned grandchild does not keep our pipes open.
fn kill_tree(child: &mut std::process::Child) {
    let _ = Command::new("kill")
        .args(["-KILL", &format!("-{}", child.id())])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
}

/// A directory with a `wc` that strips the column padding BSD `wc` adds.
fn tool_shims() -> &'static Path {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        use std::os::unix::fs::PermissionsExt;
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("shims");
        fs::create_dir_all(&dir).expect("create shim directory");
        let wc = dir.join("wc");
        fs::write(&wc, "#!/bin/sh\n/usr/bin/wc \"$@\" | sed -E 's/^ +//; s/ +/ /g'\n").expect("write wc shim");
        fs::set_permissions(&wc, fs::Permissions::from_mode(0o755)).expect("make wc shim executable");
        dir
    })
}

/// Drains a pipe on a background thread so the child never blocks on a full
/// pipe while we poll `try_wait`; partial data is still available if the pipe
/// never reaches EOF.
struct PipeReader {
    data: Arc<Mutex<Vec<u8>>>,
    done: mpsc::Receiver<()>,
}

impl PipeReader {
    fn start<R: Read + Send + 'static>(mut pipe: R) -> PipeReader {
        let data = Arc::new(Mutex::new(Vec::new()));
        let (tx, done) = mpsc::channel();
        let sink = Arc::clone(&data);
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => sink.lock().unwrap().extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            let _ = tx.send(());
        });
        PipeReader { data, done }
    }

    /// Returns the collected bytes and whether the pipe was still open at the deadline.
    fn finish(self, deadline: Instant) -> (Vec<u8>, bool) {
        let wait = deadline.saturating_duration_since(Instant::now());
        let still_open = self.done.recv_timeout(wait).is_err();
        let bytes = std::mem::take(&mut *self.data.lock().unwrap());
        (bytes, still_open)
    }
}

/// Splits into lines that keep their trailing `\n` (the last one may lack it).
fn split_lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|&b| b == b'\n').collect()
}

#[derive(Clone, Copy)]
enum Edit {
    Equal,
    Delete(usize), // line index in expected
    Insert(usize), // line index in actual
}

fn diff_lines(a: &[&[u8]], b: &[&[u8]]) -> Vec<Edit> {
    if a.len() > MAX_LCS_LINES || b.len() > MAX_LCS_LINES {
        // Positional comparison for huge outputs.
        let mut edits = Vec::new();
        for i in 0..a.len().max(b.len()) {
            match (a.get(i), b.get(i)) {
                (Some(x), Some(y)) if x == y => edits.push(Edit::Equal),
                (x, y) => {
                    if x.is_some() {
                        edits.push(Edit::Delete(i));
                    }
                    if y.is_some() {
                        edits.push(Edit::Insert(i));
                    }
                }
            }
        }
        return edits;
    }
    // lcs[i][j] = length of the LCS of a[i..] and b[j..]
    let (n, m) = (a.len(), b.len());
    let width = m + 1;
    let mut lcs = vec![0u16; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * width + j] = if a[i] == b[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut edits = Vec::new();
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            edits.push(Edit::Equal);
            i += 1;
            j += 1;
        } else if j >= m || (i < n && lcs[(i + 1) * width + j] >= lcs[i * width + j + 1]) {
            edits.push(Edit::Delete(i));
            i += 1;
        } else {
            edits.push(Edit::Insert(j));
            j += 1;
        }
    }
    edits
}

fn stdout_diff(expected: &[u8], actual: &[u8]) -> String {
    let a = split_lines(expected);
    let b = split_lines(actual);
    let edits = diff_lines(&a, &b);
    let changed: Vec<String> = edits
        .iter()
        .filter_map(|e| match *e {
            Edit::Equal => None,
            Edit::Delete(i) => Some(format!("    - {:>4} | {}", i + 1, show_line(a[i]))),
            Edit::Insert(j) => Some(format!("    + {:>4} | {}", j + 1, show_line(b[j]))),
        })
        .collect();
    let mut s = format!(
        "  stdout differs ({} expected lines, {} actual; {} changed; - expected, + actual):\n",
        a.len(),
        b.len(),
        changed.len()
    );
    for line in changed.iter().take(MAX_DIFF_LINES) {
        s.push_str(line);
        s.push('\n');
    }
    if changed.len() > MAX_DIFF_LINES {
        s.push_str(&format!("    ... {} more changed lines\n", changed.len() - MAX_DIFF_LINES));
    }
    s
}

fn stderr_excerpt(stderr: &[u8]) -> String {
    if stderr.is_empty() {
        return "  stderr: (empty)\n".to_string();
    }
    let lines = split_lines(stderr);
    let mut s = format!(
        "  stderr (first {} of {} lines):\n",
        lines.len().min(MAX_STDERR_LINES),
        lines.len()
    );
    for line in lines.iter().take(MAX_STDERR_LINES) {
        s.push_str("    | ");
        s.push_str(&show_line(line));
        s.push('\n');
    }
    s
}

/// Renders one output line for the report: control characters escaped,
/// missing final newline marked, very long lines truncated.
fn show_line(line: &[u8]) -> String {
    let (body, has_newline) = match line.strip_suffix(b"\n") {
        Some(body) => (body, true),
        None => (line, false),
    };
    let text = String::from_utf8_lossy(body);
    let mut out = String::new();
    for (count, c) in text.chars().enumerate() {
        if count >= MAX_LINE_CHARS {
            out.push_str("...");
            break;
        }
        match c {
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    if !has_newline {
        out.push_str("  [no newline at end]");
    }
    out
}

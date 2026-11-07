//! Interactive read-eval-print loop: prompts, history, completion, job notifications.

use crate::lineedit::{Candidate, Completion, LineEditor, ReadResult};
use crate::parser::{ParseError, Parser};
use crate::path;
use crate::shell::*;
use crate::vars::*;

fn history_file(sh: &mut Shell) -> Option<std::path::PathBuf> {
    let hf = sh.get_str("HISTFILE").unwrap_or_default();
    if hf.is_empty() {
        return None;
    }
    Some(path::to_win(&hf, &sh.cwd))
}

fn load_history(sh: &mut Shell, ed: &mut LineEditor) {
    let max: usize = sh.get_str("HISTSIZE").and_then(|s| s.parse().ok()).unwrap_or(500);
    ed.set_max_history(max.max(1));
    if let Some(p) = history_file(sh) {
        if let Ok(s) = std::fs::read_to_string(&p) {
            let lines: Vec<String> = s
                .lines()
                .filter(|l| !l.starts_with('#') || l.len() < 2 || !l[1..].chars().all(|c| c.is_ascii_digit()))
                .map(|l| l.trim_end_matches('\r').to_string())
                .collect();
            let start = lines.len().saturating_sub(max);
            let h: Vec<String> = lines[start..].to_vec();
            *sh.history.lock().unwrap() = h.clone();
            ed.set_history(h);
        }
    }
}

fn save_history(sh: &mut Shell, new_lines: &[String]) {
    let p = match history_file(sh) {
        Some(p) => p,
        None => return,
    };
    let filesize: usize = sh
        .get_str("HISTFILESIZE")
        .and_then(|s| s.parse().ok())
        .or_else(|| sh.get_str("HISTSIZE").and_then(|s| s.parse().ok()))
        .unwrap_or(500);
    // Always merge with the file so concurrent sessions don't clobber each other's history.
    let mut all: Vec<String> = std::fs::read_to_string(&p).map(|s| s.lines().map(|l| l.to_string()).collect()).unwrap_or_default();
    all.extend(new_lines.iter().cloned());
    let start = all.len().saturating_sub(filesize);
    let mut out = all[start..].join("\n");
    out.push('\n');
    let _ = std::fs::write(&p, out);
}

/// History expansion: !!, !$, !^, !*, !n, !-n, !prefix, ^old^new
fn hist_expand(line: &str, hist: &[String]) -> Result<Option<String>, String> {
    if !line.contains('!') && !line.starts_with('^') {
        return Ok(None);
    }
    let last = hist.last().cloned();
    if let Some(rest) = line.strip_prefix('^') {
        let parts: Vec<&str> = rest.splitn(3, '^').collect();
        if parts.len() >= 2 {
            let prev = last.ok_or("!!: event not found")?;
            if !prev.contains(parts[0]) {
                return Err(format!("^{}^{}: substitution failed", parts[0], parts[1]));
            }
            return Ok(Some(prev.replacen(parts[0], parts[1], 1) + parts.get(2).copied().unwrap_or("")));
        }
    }
    let c: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut changed = false;
    let mut in_sq = false;
    let words = |s: &str| -> Vec<String> { s.split_whitespace().map(|x| x.to_string()).collect() };
    while i < c.len() {
        let ch = c[i];
        if ch == '\'' {
            in_sq = !in_sq;
        }
        if ch == '\\' && i + 1 < c.len() {
            out.push(ch);
            out.push(c[i + 1]);
            i += 2;
            continue;
        }
        if ch != '!' || in_sq || i + 1 >= c.len() || matches!(c[i + 1], ' ' | '\t' | '=' | '(' | '"') {
            out.push(ch);
            i += 1;
            continue;
        }
        let n = c[i + 1];
        let event: String;
        let mut j = i + 1;
        if n == '!' {
            event = last.clone().ok_or("!!: event not found")?;
            j += 1;
        } else if n == '$' {
            let l = last.clone().ok_or("!$: event not found")?;
            event = words(&l).last().cloned().unwrap_or_default();
            out.push_str(&event);
            i = j + 1;
            changed = true;
            continue;
        } else if n == '^' {
            let l = last.clone().ok_or("!^: event not found")?;
            out.push_str(words(&l).get(1).map(|s| s.as_str()).unwrap_or(""));
            i = j + 1;
            changed = true;
            continue;
        } else if n == '*' {
            let l = last.clone().ok_or("!*: event not found")?;
            out.push_str(&words(&l)[1.min(words(&l).len())..].join(" "));
            i = j + 1;
            changed = true;
            continue;
        } else if n.is_ascii_digit() || n == '-' {
            let mut num = String::new();
            if n == '-' {
                num.push('-');
                j += 1;
            }
            while j < c.len() && c[j].is_ascii_digit() {
                num.push(c[j]);
                j += 1;
            }
            let v: i64 = num.parse().map_err(|_| format!("!{}: event not found", num))?;
            let idx = if v < 0 { hist.len() as i64 + v } else { v - 1 };
            event = hist.get(idx.max(0) as usize).filter(|_| idx >= 0).cloned().ok_or(format!("!{}: event not found", num))?;
        } else if n.is_alphanumeric() || n == '_' || n == '.' || n == '/' {
            let mut pfx = String::new();
            while j < c.len() && !matches!(c[j], ' ' | '\t' | ';' | '&' | '|' | ':' | '"') {
                pfx.push(c[j]);
                j += 1;
            }
            event = hist.iter().rev().find(|h| h.starts_with(&pfx)).cloned().ok_or(format!("!{}: event not found", pfx))?;
        } else {
            out.push(ch);
            i += 1;
            continue;
        }
        // word designators :$ :0 :n :*
        let mut ev = event;
        if j < c.len() && c[j] == ':' {
            let ws = words(&ev);
            if j + 1 < c.len() {
                let d = c[j + 1];
                match d {
                    '$' => {
                        ev = ws.last().cloned().unwrap_or_default();
                        j += 2;
                    }
                    '^' => {
                        ev = ws.get(1).cloned().unwrap_or_default();
                        j += 2;
                    }
                    '*' => {
                        ev = ws.get(1..).map(|s| s.join(" ")).unwrap_or_default();
                        j += 2;
                    }
                    d if d.is_ascii_digit() => {
                        ev = ws.get(d.to_digit(10).unwrap() as usize).cloned().unwrap_or_default();
                        j += 2;
                    }
                    _ => {}
                }
            }
        }
        out.push_str(&ev);
        changed = true;
        i = j;
    }
    Ok(if changed { Some(out) } else { None })
}

pub fn report_jobs(sh: &mut Shell) {
    let n = sh.jobs.list.len();
    let mut msgs = String::new();
    for (k, j) in sh.jobs.list.iter_mut().enumerate() {
        if j.poll().is_some() && !j.notified {
            j.notified = true;
            let mark = if k + 1 == n { '+' } else if k + 2 == n { '-' } else { ' ' };
            let st = crate::builtins::job_state(j);
            msgs.push_str(&format!("[{}]{}  {:<24}{}\n", j.id, mark, st, j.text));
        }
    }
    sh.jobs.list.retain(|j| !j.notified);
    if !msgs.is_empty() {
        sh.write_fd(2, msgs.as_bytes());
    }
}

fn run_prompt_command(sh: &mut Shell) {
    let pc = match sh.get_value("PROMPT_COMMAND") {
        Some(Value::Str(s)) => vec![s],
        Some(Value::Arr(a)) => a.values().cloned().collect(),
        _ => return,
    };
    let saved = sh.status;
    for c in pc {
        if !c.trim().is_empty() {
            let _ = sh.run_source(&c, false);
        }
    }
    sh.status = saved;
}

pub fn repl(sh: &mut Shell) -> i32 {
    let mut ed = LineEditor::new();
    load_history(sh, &mut ed);
    let mut new_hist: Vec<String> = Vec::new();
    let exit_status;
    let mut eof_count = 0;
    'outer: loop {
        crate::sys::take_interrupt();
        report_jobs(sh);
        if sh.sh[sho::checkwinsize()] {
            if let Some((cols, lines)) = crate::sys::console_size() {
                let _ = sh.set_var("COLUMNS", &cols.to_string(), false);
                let _ = sh.set_var("LINES", &lines.to_string(), false);
            }
        }
        run_prompt_command(sh);
        let ps1 = sh.get_str("PS1").unwrap_or_default();
        let prompt = crate::prompt::expand_prompt(sh, &ps1);
        let mut buf = String::new();
        let mut first = true;
        loop {
            let p = if first {
                prompt.clone()
            } else {
                let ps2 = sh.get_str("PS2").unwrap_or_else(|| "> ".into());
                crate::prompt::expand_prompt(sh, &ps2)
            };
            let res = {
                let mut completer = |line: &str, pos: usize| complete(sh, line, pos);
                ed.read_line(&p, &mut completer)
            };
            match res {
                ReadResult::Eof => {
                    if !first {
                        // unexpected EOF in continuation
                        sh.eprint("syntax error: unexpected end of file");
                        continue 'outer;
                    }
                    let ignore: usize = if sh.o[crate::shell::set_opt_index("ignoreeof").unwrap()] {
                        sh.get_str("IGNOREEOF").and_then(|s| s.parse().ok()).unwrap_or(10)
                    } else {
                        0
                    };
                    if eof_count < ignore {
                        eof_count += 1;
                        sh.write_fd(2, b"Use \"exit\" to leave the shell.\n");
                        continue 'outer;
                    }
                    sh.write_fd(2, b"exit\n");
                    exit_status = sh.status;
                    break 'outer;
                }
                ReadResult::Interrupted => {
                    sh.status = 130;
                    continue 'outer;
                }
                ReadResult::Line(l) => {
                    eof_count = 0;
                    if !first {
                        buf.push('\n');
                    }
                    buf.push_str(&l);
                    first = false;
                    let mut p = Parser::new(&buf);
                    p.interactive = true;
                    p.aliases = if sh.sh[sho::expand_aliases()] { Some(sh.aliases.clone()) } else { None };
                    match p.parse_program() {
                        Err(ParseError::Incomplete) => continue,
                        _ => break,
                    }
                }
            }
        }
        if buf.trim().is_empty() {
            continue;
        }
        // history expansion
        if sh.o[O_HISTEXPAND] {
            let hist = ed.history().to_vec();
            match hist_expand(&buf, &hist) {
                Ok(Some(e)) => {
                    sh.write_fd(2, format!("{}\n", e).as_bytes());
                    buf = e;
                }
                Ok(None) => {}
                Err(m) => {
                    sh.eprint(&m);
                    continue;
                }
            }
        }
        // history recording
        let hc = sh.get_str("HISTCONTROL").unwrap_or_default();
        let ignorespace = hc.contains("ignorespace") || hc.contains("ignoreboth");
        let ignoredups = hc.contains("ignoredups") || hc.contains("ignoreboth") || hc.contains("erasedups");
        let entry = history_entry(&buf);
        let skip = (ignorespace && entry.starts_with(' ')) || (ignoredups && ed.history().last() == Some(&entry));
        if !skip && sh.o[O_HISTORY] {
            if hc.contains("erasedups") {
                let mut h: Vec<String> = ed.history().iter().filter(|x| **x != entry).cloned().collect();
                h.push(entry.clone());
                ed.set_history(h);
            } else {
                ed.add_history(&entry);
            }
            sh.history.lock().unwrap().push(entry.clone());
            new_hist.push(entry);
        }
        let r = sh.run_source(&buf, false);
        match r {
            Ok(s) => {
                sh.status = s;
                // a foreground child died from Ctrl+C: echo it like the terminal would
                if crate::sys::take_interrupt() {
                    sh.write_fd(2, b"^C\n");
                }
            }
            Err(Flow::Exit(s)) => {
                exit_status = s;
                break;
            }
            Err(Flow::Fatal(s)) | Err(Flow::Abort(s)) => {
                sh.status = s;
                if s == 130 {
                    sh.write_fd(2, b"^C\n");
                }
            }
            Err(Flow::Return(s)) => sh.status = s,
            Err(_) => {}
        }
    }
    save_history(sh, &new_hist);
    sh.status = exit_status;
    sh.run_exit_trap();
    sh.status
}

/// Turn a multi-line command into a single history line, like bash's `cmdhist`:
/// lines are joined with "; " (or a space after `do`, `then`, `|`, `{`, ...).
/// Newlines inside quotes are kept; here-documents, comments, or anything whose
/// joined form wouldn't parse keep the original text.
pub fn history_entry(buf: &str) -> String {
    if !buf.contains('\n') || buf.contains("<<") {
        return buf.to_string();
    }
    let mut out = String::new();
    let (mut in_sq, mut in_dq) = (false, false);
    for (i, line) in buf.split('\n').enumerate() {
        if i > 0 {
            if in_sq || in_dq {
                out.push('\n');
            } else if out.ends_with('\\') {
                out.pop();
            } else if line.trim().is_empty() {
                continue;
            } else {
                let t = out.trim_end();
                let last_word = t.rsplit([' ', '\t', ';', '&', '|']).next().unwrap_or("");
                let space = t.is_empty()
                    || t.ends_with([';', '&', '|', '(', '{'])
                    || matches!(last_word, "do" | "then" | "else" | "elif" | "if" | "while" | "until" | "in" | "!" | "time");
                out.truncate(t.len());
                out.push_str(if space { " " } else { "; " });
            }
        }
        // track quote state and bail out on comments
        let mut it = line.chars().peekable();
        let mut prev_blank = true;
        while let Some(c) = it.next() {
            if in_sq {
                if c == '\'' {
                    in_sq = false;
                }
            } else if in_dq {
                match c {
                    '\\' => {
                        it.next();
                    }
                    '"' => in_dq = false,
                    _ => {}
                }
            } else {
                match c {
                    '\\' => {
                        it.next();
                    }
                    '\'' => in_sq = true,
                    '"' => in_dq = true,
                    '#' if prev_blank => return buf.to_string(),
                    _ => {}
                }
            }
            prev_blank = matches!(c, ' ' | '\t' | ';' | '&' | '|' | '(');
        }
        out.push_str(line);
    }
    match Parser::new(&out).parse_program() {
        Ok(_) => out,
        Err(_) => buf.to_string(),
    }
}

// ---------------- completion ----------------

fn escape_for_shell(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        if " \t\n'\"\\$`&|;<>()[]{}*?!#~=".contains(c) {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

fn unescape_word(s: &str) -> String {
    let mut o = String::new();
    let mut it = s.chars();
    let mut q: Option<char> = None;
    while let Some(c) = it.next() {
        match (q, c) {
            (None, '\\') => {
                if let Some(n) = it.next() {
                    o.push(n);
                }
            }
            (None, '\'') | (None, '"') => q = Some(c),
            (Some(x), c) if c == x => q = None,
            (_, c) => o.push(c),
        }
    }
    o
}

/// Find the start of the word under the cursor and whether it's in command position.
fn word_bounds(line: &str, pos: usize) -> (usize, bool) {
    let before = &line[..pos];
    let mut start = 0;
    let mut q: Option<char> = None;
    let mut cmd_pos = true;
    let mut last_sep_cmd = true;
    let chars: Vec<(usize, char)> = before.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (bi, c) = chars[i];
        match q {
            Some(x) => {
                if c == x {
                    q = None;
                } else if c == '\\' && x == '"' {
                    i += 1;
                }
            }
            None => match c {
                '\\' => i += 1,
                '\'' | '"' => q = Some(c),
                ' ' | '\t' | '\n' => {
                    let w = &before[start..bi];
                    if !w.is_empty() {
                        cmd_pos = matches!(w, "sudo" | "time" | "command" | "builtin" | "exec" | "nohup" | "xargs" | "then" | "do" | "else" | "if" | "while" | "until" | "!" | "{" | "elif") && last_sep_cmd
                            || (w.contains('=') && !w.starts_with('=') && last_sep_cmd && cmd_pos);
                        last_sep_cmd = cmd_pos;
                    }
                    start = bi + c.len_utf8();
                }
                ';' | '|' | '&' | '(' | '`' => {
                    cmd_pos = true;
                    last_sep_cmd = true;
                    start = bi + 1;
                }
                '<' | '>' => {
                    cmd_pos = false;
                    start = bi + 1;
                }
                _ => {}
            },
        }
        i += 1;
    }
    let w = &before[start..];
    let in_cmd = cmd_pos && !before[..start].trim_end().ends_with(['<', '>']);
    let _ = w;
    (start, in_cmd)
}

pub fn command_names(sh: &mut Shell, prefix: &str) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let mut out: Vec<String> = Vec::new();
    let pathv = sh.get_str("PATH").unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    for d in path::split_posix_list(&pathv) {
        if d.is_empty() {
            continue;
        }
        let wd = path::to_win(&d, &sh.cwd);
        let rd = match std::fs::read_dir(&wd) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with(prefix) {
                continue;
            }
            let executable = e.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false);
            if executable && seen.insert(name.clone()) {
                out.push(name);
            }
        }
    }
    out
}

pub fn file_candidates(sh: &mut Shell, word: &str, dirs_only: bool) -> Vec<String> {
    file_cands(sh, word, dirs_only, false).into_iter().map(|(p, _)| p).collect()
}

/// Returns (path-as-typed, is_dir).
fn file_cands(sh: &mut Shell, word: &str, dirs_only: bool, exec_only: bool) -> Vec<(String, bool)> {
    let (dir_part, base) = match word.rfind('/') {
        Some(i) => (&word[..=i], &word[i + 1..]),
        None => ("", word),
    };
    let lookup_dir = if dir_part.starts_with('~') {
        let rest = &dir_part[1..];
        let (user, tail) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        format!("{}{}", sh.tilde(user).unwrap_or_default(), tail)
    } else {
        dir_part.to_string()
    };
    let wd = if lookup_dir.is_empty() { sh.cwd.clone() } else { path::to_win(&lookup_dir, &sh.cwd) };
    let mut out = Vec::new();
    // virtual root: offer drive letters for "/"
    let bl = base.to_lowercase();
    if let Ok(rd) = std::fs::read_dir(&wd) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.to_lowercase().starts_with(&bl) {
                continue;
            }
            if name.starts_with('.') && !base.starts_with('.') {
                continue;
            }
            let is_dir = e.file_type().map(|t| t.is_dir() || (t.is_symlink() && e.path().is_dir())).unwrap_or(false);
            if dirs_only && !is_dir {
                continue;
            }
            if exec_only && !is_dir {
                use std::os::unix::fs::PermissionsExt;
                let executable = e.metadata().map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false);
                if !executable {
                    continue;
                }
            }
            out.push((format!("{}{}", dir_part, name), is_dir));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn complete(sh: &mut Shell, line: &str, pos: usize) -> Completion {
    let (start, cmd_pos) = word_bounds(line, pos);
    let raw = &line[start..pos];
    let word = unescape_word(raw);
    let mut cands: Vec<Candidate> = Vec::new();
    // variables
    if let Some(v) = word.strip_prefix('$') {
        let brace = v.starts_with('{');
        let pfx = v.trim_start_matches('{');
        let mut names = sh.vars.names();
        names.sort();
        for n in names {
            if n.starts_with(pfx) {
                let rep = if brace { format!("${{{}}}", n) } else { format!("${}", n) };
                cands.push(Candidate { replace: rep, display: n, append_space: !brace });
            }
        }
        return Completion { start, candidates: cands };
    }
    if cmd_pos && !word.contains('/') && !word.starts_with('~') {
        let mut names: Vec<String> = Vec::new();
        names.extend(sh.aliases.keys().filter(|k| k.starts_with(&word)).cloned());
        names.extend(sh.funcs.keys().filter(|k| k.starts_with(&word)).cloned());
        names.extend(crate::builtins::BUILTINS.iter().filter(|k| k.starts_with(&word) && !k.starts_with("__")).map(|s| s.to_string()));
        names.extend(crate::builtins::KEYWORDS.iter().filter(|k| k.starts_with(&word) && k.len() > 1).map(|s| s.to_string()));
        names.extend(command_names(sh, &word));
        names.sort();
        names.dedup();
        for n in names {
            cands.push(Candidate { replace: escape_for_shell(&n), display: n, append_space: true });
        }
        // also directories (for ./ or plain dir names)
        for (p, is_dir) in file_cands(sh, &word, true, false) {
            if is_dir {
                cands.push(Candidate { replace: escape_for_shell(&p) + "/", display: format!("{}/", p), append_space: false });
            }
        }
        return Completion { start, candidates: cands };
    }
    let exec_only = cmd_pos;
    // Keep the quoting style the user started with: 'dir with<Tab> -> 'dir with space/
    let quote = raw.chars().next().filter(|c| *c == '\'' || *c == '"');
    for (p, is_dir) in file_cands(sh, &word, false, exec_only) {
        let disp = p.rsplit('/').next().unwrap_or(&p).to_string();
        match quote {
            Some(q) => {
                let inner = if q == '\'' {
                    p.replace('\'', "'\\''")
                } else {
                    let mut s = String::new();
                    for c in p.chars() {
                        if matches!(c, '"' | '\\' | '$' | '`') {
                            s.push('\\');
                        }
                        s.push(c);
                    }
                    s
                };
                if is_dir {
                    cands.push(Candidate { replace: format!("{}{}/", q, inner), display: format!("{}/", disp), append_space: false });
                } else {
                    cands.push(Candidate { replace: format!("{}{}{}", q, inner, q), display: disp, append_space: true });
                }
            }
            None => {
                let rep = escape_for_shell(&p).replacen("\\~", "~", 1);
                if is_dir {
                    cands.push(Candidate { replace: rep + "/", display: format!("{}/", disp), append_space: false });
                } else {
                    cands.push(Candidate { replace: rep, display: disp, append_space: true });
                }
            }
        }
    }
    Completion { start, candidates: cands }
}


#[cfg(test)]
mod tests {
    use super::history_entry;

    #[test]
    fn completes_builtin() {
        let mut sh = crate::shell::Shell::new("gash");
        let c = super::complete(&mut sh, "ec", 2);
        assert!(c.candidates.iter().any(|x| x.display == "echo"), "{:?}", c.candidates);
    }
    #[test]
    fn cmdhist_joining() {
        assert_eq!(history_entry("for i in 1 2; do\necho $i\ndone"), "for i in 1 2; do echo $i; done");
        assert_eq!(history_entry("if true\nthen echo a\nfi"), "if true; then echo a; fi");
        assert_eq!(history_entry("f() {\necho hi\n}"), "f() { echo hi; }");
        assert_eq!(history_entry("echo a |\nwc -l"), "echo a | wc -l");
        assert_eq!(history_entry("echo \"a\nb\""), "echo \"a\nb\"");
        assert_eq!(history_entry("echo a \\\nb"), "echo a b");
        assert_eq!(history_entry("case x in\nx) echo x;;\nesac"), "case x in x) echo x;; esac");
        // comments and here-documents keep their newlines
        assert_eq!(history_entry("echo a # c\necho b"), "echo a # c\necho b");
        assert_eq!(history_entry("cat <<EOF\nhi\nEOF"), "cat <<EOF\nhi\nEOF");
    }
}
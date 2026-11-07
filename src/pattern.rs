//! Shell pattern matching (fnmatch with bash extglob) and pathname expansion.
//!
//! Patterns are strings in which a backslash escapes the following character.

use std::path::Path;

#[derive(Debug, Clone)]
enum ClassItem {
    Ch(char),
    Range(char, char),
    Named(String),
}

#[derive(Debug, Clone)]
enum Tok {
    Ch(char),
    Any,
    Star,
    Class { neg: bool, items: Vec<ClassItem> },
    Ext { kind: char, alts: Vec<Vec<Tok>> },
}

#[derive(Debug, Clone)]
pub struct Pattern {
    toks: Vec<Tok>,
    nocase: bool,
}

pub fn has_glob_chars(p: &str) -> bool {
    let b: Vec<char> = p.chars().collect();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            '\\' => i += 1,
            '*' | '?' | '[' => return true,
            '+' | '@' | '!' if b.get(i + 1) == Some(&'(') => return true,
            _ => {}
        }
        i += 1;
    }
    false
}

/// Escape all pattern-special characters.
pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '*' | '?' | '[' | ']' | '(' | ')' | '|' | '!' | '@' | '+' | '&' | '{' | '}' | '^') {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

/// Remove pattern escapes (used when a glob has no matches).
pub fn unescape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            if let Some(n) = it.next() {
                o.push(n);
            } else {
                o.push('\\');
            }
        } else {
            o.push(c);
        }
    }
    o
}

fn parse_class(c: &[char], mut i: usize) -> Option<(Tok, usize)> {
    // c[i] == '['
    i += 1;
    let mut neg = false;
    if i < c.len() && (c[i] == '!' || c[i] == '^') {
        neg = true;
        i += 1;
    }
    let mut items = Vec::new();
    let mut first = true;
    loop {
        if i >= c.len() {
            return None;
        }
        let ch = c[i];
        if ch == ']' && !first {
            return Some((Tok::Class { neg, items }, i + 1));
        }
        first = false;
        if ch == '[' && i + 1 < c.len() && c[i + 1] == ':' {
            // [:name:]
            let mut j = i + 2;
            let mut name = String::new();
            while j + 1 < c.len() && !(c[j] == ':' && c[j + 1] == ']') {
                name.push(c[j]);
                j += 1;
            }
            if j + 1 < c.len() {
                items.push(ClassItem::Named(name));
                i = j + 2;
                continue;
            }
            return None;
        }
        let lo = if ch == '\\' && i + 1 < c.len() {
            i += 1;
            c[i]
        } else {
            ch
        };
        i += 1;
        if i + 1 < c.len() && c[i] == '-' && c[i + 1] != ']' {
            let mut hi = c[i + 1];
            i += 2;
            if hi == '\\' && i < c.len() {
                hi = c[i];
                i += 1;
            }
            items.push(ClassItem::Range(lo, hi));
        } else {
            items.push(ClassItem::Ch(lo));
        }
    }
}

fn compile_seq(c: &[char], mut i: usize, stop_alt: bool, ext: bool) -> (Vec<Tok>, usize) {
    let mut toks = Vec::new();
    while i < c.len() {
        let ch = c[i];
        if stop_alt && (ch == '|' || ch == ')') {
            break;
        }
        match ch {
            '\\' => {
                if i + 1 < c.len() {
                    toks.push(Tok::Ch(c[i + 1]));
                    i += 2;
                } else {
                    toks.push(Tok::Ch('\\'));
                    i += 1;
                }
            }
            '*' | '?' | '+' | '@' | '!' if ext && c.get(i + 1) == Some(&'(') => {
                // extglob
                let kind = ch;
                let mut j = i + 2;
                let mut alts = Vec::new();
                let mut ok = false;
                loop {
                    let (seq, nj) = compile_seq(c, j, true, ext);
                    alts.push(seq);
                    j = nj;
                    if j >= c.len() {
                        break;
                    }
                    if c[j] == '|' {
                        j += 1;
                        continue;
                    }
                    if c[j] == ')' {
                        j += 1;
                        ok = true;
                        break;
                    }
                }
                if ok {
                    toks.push(Tok::Ext { kind, alts });
                    i = j;
                } else {
                    toks.push(Tok::Ch(ch));
                    i += 1;
                }
            }
            '*' => {
                if !matches!(toks.last(), Some(Tok::Star)) {
                    toks.push(Tok::Star);
                }
                i += 1;
            }
            '?' => {
                toks.push(Tok::Any);
                i += 1;
            }
            '[' => match parse_class(c, i) {
                Some((t, ni)) => {
                    toks.push(t);
                    i = ni;
                }
                None => {
                    toks.push(Tok::Ch('['));
                    i += 1;
                }
            },
            _ => {
                toks.push(Tok::Ch(ch));
                i += 1;
            }
        }
    }
    (toks, i)
}

fn class_named(name: &str, c: char) -> bool {
    match name {
        "alpha" => c.is_alphabetic(),
        "digit" => c.is_ascii_digit(),
        "alnum" => c.is_alphanumeric(),
        "upper" => c.is_uppercase(),
        "lower" => c.is_lowercase(),
        "space" => c.is_whitespace(),
        "blank" => c == ' ' || c == '\t',
        "punct" => c.is_ascii_punctuation(),
        "print" => !c.is_control(),
        "graph" => !c.is_control() && !c.is_whitespace(),
        "cntrl" => c.is_control(),
        "xdigit" => c.is_ascii_hexdigit(),
        "word" => c.is_alphanumeric() || c == '_',
        _ => false,
    }
}

fn eq_char(a: char, b: char, nocase: bool) -> bool {
    a == b || (nocase && a.to_lowercase().eq(b.to_lowercase()))
}

fn class_match(neg: bool, items: &[ClassItem], c: char, nocase: bool) -> bool {
    let mut m = false;
    for it in items {
        let hit = match it {
            ClassItem::Ch(x) => eq_char(*x, c, nocase),
            ClassItem::Range(lo, hi) => {
                (*lo <= c && c <= *hi)
                    || (nocase && {
                        let l = c.to_ascii_lowercase();
                        let u = c.to_ascii_uppercase();
                        (*lo <= l && l <= *hi) || (*lo <= u && u <= *hi)
                    })
            }
            ClassItem::Named(n) => class_named(n, c),
        };
        if hit {
            m = true;
            break;
        }
    }
    m != neg
}

impl Pattern {
    pub fn new(p: &str, nocase: bool) -> Pattern {
        Pattern::new_ext(p, nocase, true)
    }

    pub fn new_ext(p: &str, nocase: bool, extglob: bool) -> Pattern {
        let c: Vec<char> = p.chars().collect();
        let (toks, _) = compile_seq(&c, 0, false, extglob);
        Pattern { toks, nocase }
    }

    pub fn matches(&self, s: &str) -> bool {
        let c: Vec<char> = s.chars().collect();
        m(&self.toks, &c, 0, c.len(), self.nocase)
    }

    pub fn matches_chars(&self, c: &[char], start: usize, end: usize) -> bool {
        m(&self.toks, c, start, end, self.nocase)
    }

    /// Is this pattern just a literal string?
    pub fn literal(&self) -> Option<String> {
        let mut s = String::new();
        for t in &self.toks {
            match t {
                Tok::Ch(c) => s.push(*c),
                _ => return None,
            }
        }
        Some(s)
    }
}

/// Does `toks` match exactly s[si..end]?
fn m(toks: &[Tok], s: &[char], si: usize, end: usize, nc: bool) -> bool {
    if toks.is_empty() {
        return si == end;
    }
    match &toks[0] {
        Tok::Ch(c) => si < end && eq_char(*c, s[si], nc) && m(&toks[1..], s, si + 1, end, nc),
        Tok::Any => si < end && m(&toks[1..], s, si + 1, end, nc),
        Tok::Class { neg, items } => si < end && class_match(*neg, items, s[si], nc) && m(&toks[1..], s, si + 1, end, nc),
        Tok::Star => {
            let rest = &toks[1..];
            if rest.is_empty() {
                return true;
            }
            // fast path: next is literal char
            if let Tok::Ch(c) = rest[0] {
                let mut k = si;
                while k < end {
                    if eq_char(c, s[k], nc) && m(rest, s, k, end, nc) {
                        return true;
                    }
                    k += 1;
                }
                return false;
            }
            (si..=end).any(|k| m(rest, s, k, end, nc))
        }
        Tok::Ext { kind, alts } => {
            let rest = &toks[1..];
            let any_alt = |a: usize, b: usize| alts.iter().any(|alt| m(alt, s, a, b, nc));
            match kind {
                '@' => (si..=end).any(|k| any_alt(si, k) && m(rest, s, k, end, nc)),
                '?' => m(rest, s, si, end, nc) || (si..=end).any(|k| any_alt(si, k) && m(rest, s, k, end, nc)),
                '*' | '+' => {
                    // one-or-more repetition then rest; '*' also allows zero
                    if *kind == '*' && m(rest, s, si, end, nc) {
                        return true;
                    }
                    fn rep(alts: &[Vec<Tok>], rest: &[Tok], s: &[char], si: usize, end: usize, nc: bool) -> bool {
                        for k in (si + 1)..=end {
                            if alts.iter().any(|alt| m(alt, s, si, k, nc))
                                && (m(rest, s, k, end, nc) || rep(alts, rest, s, k, end, nc))
                            {
                                return true;
                            }
                        }
                        // allow empty match of alternative only once
                        false
                    }
                    if alts.iter().any(|alt| m(alt, s, si, si, nc)) && m(rest, s, si, end, nc) {
                        return true;
                    }
                    rep(alts, rest, s, si, end, nc)
                }
                '!' => (si..=end).any(|k| !any_alt(si, k) && m(rest, s, k, end, nc)),
                _ => false,
            }
        }
    }
}

pub fn pattern_match(pat: &str, s: &str, nocase: bool) -> bool {
    Pattern::new(pat, nocase).matches(s)
}

pub fn pattern_match_ext(pat: &str, s: &str, nocase: bool, extglob: bool) -> bool {
    Pattern::new_ext(pat, nocase, extglob).matches(s)
}

/// Remove shortest/longest matching prefix.
pub fn trim_prefix(s: &str, pat: &str, longest: bool, nocase: bool, ext: bool) -> String {
    let p = Pattern::new_ext(pat, nocase, ext);
    let c: Vec<char> = s.chars().collect();
    if longest {
        for k in (0..=c.len()).rev() {
            if p.matches_chars(&c, 0, k) {
                return c[k..].iter().collect();
            }
        }
    } else {
        for k in 0..=c.len() {
            if p.matches_chars(&c, 0, k) {
                return c[k..].iter().collect();
            }
        }
    }
    s.to_string()
}

pub fn trim_suffix(s: &str, pat: &str, longest: bool, nocase: bool, ext: bool) -> String {
    let p = Pattern::new_ext(pat, nocase, ext);
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    if longest {
        for k in 0..=n {
            if p.matches_chars(&c, k, n) {
                return c[..k].iter().collect();
            }
        }
    } else {
        for k in (0..=n).rev() {
            if p.matches_chars(&c, k, n) {
                return c[..k].iter().collect();
            }
        }
    }
    s.to_string()
}

/// Find matches for `${x/pat/rep}`. Returns list of (start, end) char ranges.
pub fn find_matches(s: &str, pat: &str, mode: crate::ast::ReplMode, nocase: bool, ext: bool) -> (Vec<char>, Vec<(usize, usize)>) {
    use crate::ast::ReplMode;
    let p = Pattern::new_ext(pat, nocase, ext);
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let mut out = Vec::new();
    match mode {
        ReplMode::Prefix => {
            for k in (0..=n).rev() {
                if p.matches_chars(&c, 0, k) {
                    out.push((0, k));
                    break;
                }
            }
        }
        ReplMode::Suffix => {
            for k in 0..=n {
                if p.matches_chars(&c, k, n) {
                    out.push((k, n));
                    break;
                }
            }
        }
        ReplMode::First | ReplMode::All => {
            let mut i = 0;
            while i <= n {
                let mut found = None;
                for k in (i..=n).rev() {
                    if p.matches_chars(&c, i, k) {
                        found = Some(k);
                        break;
                    }
                }
                match found {
                    Some(k) if k > i || (pat.is_empty()) => {
                        out.push((i, k));
                        if mode == ReplMode::First {
                            break;
                        }
                        i = if k > i { k } else { i + 1 };
                    }
                    Some(k) if k == i && !pat.is_empty() => {
                        // empty match; bash skips empty matches
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
        }
    }
    (c, out)
}

// ---------------- pathname expansion ----------------

#[derive(Clone, Copy, Default)]
pub struct GlobOpts {
    pub dotglob: bool,
    pub nocaseglob: bool,
    pub globstar: bool,
    pub extglob: bool,
    /// Sort with plain byte order (C/POSIX locale) instead of locale-like collation.
    pub c_locale: bool,
}

pub fn sort_names(v: &mut [String], c_locale: bool) {
    if c_locale {
        v.sort();
    } else {
        v.sort_by(|a, b| collate_cmp(a, b));
    }
}

/// Locale-ish collation (approximates en_US.UTF-8 ordering used by Git Bash).
pub fn collate_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let ka: String = a.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect();
    let kb: String = b.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect();
    ka.cmp(&kb).then_with(|| {
        let la = a.to_lowercase();
        let lb = b.to_lowercase();
        la.cmp(&lb).then_with(|| b.cmp(a))
    })
}

fn split_components(pat: &str) -> Vec<String> {
    // split on unescaped '/', keeping escapes
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut it = pat.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            cur.push(c);
            if let Some(n) = it.next() {
                cur.push(n);
            }
        } else if c == '/' {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// Expand a glob pattern (POSIX-style, escaped); `resolve` maps paths against the shell directory.
pub fn glob(pat: &str, opts: GlobOpts, resolve: &dyn Fn(&str) -> std::path::PathBuf) -> Vec<String> {
    let comps = split_components(pat);
    // Find leading literal components.
    let abs = pat.starts_with('/');
    let mut prefix = String::new();
    let mut idx = 0;
    if abs {
        prefix.push('/');
        idx = 1;
    }
    while idx < comps.len() && !has_glob_chars(&comps[idx]) {
        if idx == comps.len() - 1 {
            break;
        }
        prefix.push_str(&unescape(&comps[idx]));
        prefix.push('/');
        idx += 1;
    }
    if idx >= comps.len() {
        return vec![];
    }
    let mut results: Vec<String> = Vec::new();
    expand_from(&prefix, &comps[idx..], opts, resolve, &mut results);
    sort_names(&mut results, opts.c_locale);
    results
}

fn list_dir(dir: &str, resolve: &dyn Fn(&str) -> std::path::PathBuf) -> Vec<(String, bool)> {
    let p = if dir.is_empty() { resolve(".") } else { resolve(dir) };
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&p) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().map(|t| t.is_dir() || (t.is_symlink() && e.path().is_dir())).unwrap_or(false);
            v.push((name, is_dir));
        }
    }
    v
}

fn expand_from(prefix: &str, comps: &[String], opts: GlobOpts, resolve: &dyn Fn(&str) -> std::path::PathBuf, out: &mut Vec<String>) {
    if comps.is_empty() {
        return;
    }
    let comp = &comps[0];
    let last = comps.len() == 1;
    if comp.is_empty() {
        // trailing slash: prefix must be a dir
        if last {
            let p = resolve(prefix.trim_end_matches('/'));
            if p.is_dir() {
                out.push(prefix.to_string());
            }
        } else {
            expand_from(&format!("{}/", prefix.trim_end_matches('/')), &comps[1..], opts, resolve, out);
        }
        return;
    }
    if !has_glob_chars(comp) {
        let name = unescape(comp);
        let path = format!("{}{}", prefix, name);
        let wp = resolve(&path);
        if last {
            if wp.exists() || std::fs::symlink_metadata(&wp).is_ok() {
                out.push(path);
            }
        } else if wp.is_dir() {
            expand_from(&format!("{}/", path), &comps[1..], opts, resolve, out);
        }
        return;
    }
    if opts.globstar && comp == "**" {
        // zero or more directories
        let rest = &comps[1..];
        if rest.is_empty() {
            // `**` alone: all files and dirs recursively (and the directory itself)
            if !prefix.is_empty() {
                out.push(prefix.to_string());
            }
            walk_all(prefix, opts, resolve, out, true);
            return;
        }
        expand_from(prefix, rest, opts, resolve, out);
        let mut dirs = Vec::new();
        walk_dirs(prefix, opts, resolve, &mut dirs);
        for d in dirs {
            expand_from(&format!("{}/", d), rest, opts, resolve, out);
        }
        return;
    }
    let p = Pattern::new_ext(comp, opts.nocaseglob, opts.extglob);
    let explicit_dot = comp.starts_with('.') || comp.starts_with("\\.");
    let dir = prefix.trim_end_matches('/');
    let dir = if dir.is_empty() && prefix == "/" { "/" } else { dir };
    let entries = list_dir(if prefix.is_empty() { "" } else { dir }, resolve);
    for (name, is_dir) in entries {
        if name.starts_with('.') && !explicit_dot && !opts.dotglob {
            continue;
        }
        if !p.matches(&name) {
            continue;
        }
        let path = format!("{}{}", prefix, name);
        if last {
            out.push(path);
        } else if is_dir {
            expand_from(&format!("{}/", path), &comps[1..], opts, resolve, out);
        }
    }
}

fn walk_dirs(prefix: &str, opts: GlobOpts, resolve: &dyn Fn(&str) -> std::path::PathBuf, out: &mut Vec<String>) {
    let dir = prefix.trim_end_matches('/');
    for (name, is_dir) in list_dir(dir, resolve) {
        if name.starts_with('.') && !opts.dotglob {
            continue;
        }
        if is_dir {
            let path = format!("{}{}", prefix, name);
            out.push(path.clone());
            walk_dirs(&format!("{}/", path), opts, resolve, out);
        }
    }
}

fn walk_all(prefix: &str, opts: GlobOpts, resolve: &dyn Fn(&str) -> std::path::PathBuf, out: &mut Vec<String>, _top: bool) {
    let dir = prefix.trim_end_matches('/');
    for (name, is_dir) in list_dir(dir, resolve) {
        if name.starts_with('.') && !opts.dotglob {
            continue;
        }
        let path = format!("{}{}", prefix, name);
        out.push(path.clone());
        if is_dir {
            walk_all(&format!("{}/", path), opts, resolve, out, false);
        }
    }
}

pub fn path_exists(p: &Path) -> bool {
    p.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn basic() {
        assert!(pattern_match("*.rs", "main.rs", false));
        assert!(!pattern_match("*.rs", "main.rc", false));
        assert!(pattern_match("a?c", "abc", false));
        assert!(pattern_match("[a-c]x", "bx", false));
        assert!(pattern_match("[!a-c]x", "dx", false));
        assert!(pattern_match("[[:digit:]]*", "1abc", false));
        assert!(pattern_match("@(foo|bar)", "bar", false));
        assert!(!pattern_match("!(foo)", "foo", false));
        assert!(pattern_match("!(foo)", "fox", false));
        assert!(pattern_match("+(ab)", "ababab", false));
        assert!(pattern_match("*(ab)c", "c", false));
        assert!(pattern_match("\\*", "*", false));
        assert!(!pattern_match("\\*", "x", false));
        assert!(pattern_match("[]]", "]", false));
    }
    #[test]
    fn trims() {
        assert_eq!(trim_prefix("a.b.c", "*.", false, false, false), "b.c");
        assert_eq!(trim_prefix("a.b.c", "*.", true, false, false), "c");
        assert_eq!(trim_suffix("a.b.c", ".*", false, false, false), "a.b");
        assert_eq!(trim_suffix("a.b.c", ".*", true, false, false), "a");
    }
}

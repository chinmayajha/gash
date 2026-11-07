//! Word expansion: brace, tilde, parameter, command substitution, arithmetic,
//! word splitting, pathname expansion and quote removal.

use crate::ast::*;
use crate::io::Handle;
use crate::pattern::{self, Pattern};
use crate::shell::*;
use crate::vars::*;
use std::sync::{Arc, Mutex};

#[derive(Default, Clone, Debug)]
pub struct Field {
    pub s: String,
    pub pat: String,
}

pub struct FB {
    pub fields: Vec<Field>,
    cur: Field,
    active: bool,
    split: bool,
    ifs_ws: String,
    ifs_other: String,
}

impl FB {
    pub fn new(split: bool, ifs: &str) -> FB {
        let ifs_ws: String = ifs.chars().filter(|c| matches!(c, ' ' | '\t' | '\n')).collect();
        let ifs_other: String = ifs.chars().filter(|c| !matches!(c, ' ' | '\t' | '\n')).collect();
        FB { fields: Vec::new(), cur: Field::default(), active: false, split, ifs_ws, ifs_other }
    }
    fn quoted(&mut self, t: &str) {
        self.cur.s.push_str(t);
        self.cur.pat.push_str(&pattern::escape(t));
        self.active = true;
    }
    fn lit(&mut self, t: &str) {
        self.cur.s.push_str(t);
        self.cur.pat.push_str(t);
        if !t.is_empty() {
            self.active = true;
        }
    }
    fn mark_active(&mut self) {
        self.active = true;
    }
    fn unquoted(&mut self, t: &str) {
        if !self.split || (self.ifs_ws.is_empty() && self.ifs_other.is_empty()) {
            self.lit(t);
            return;
        }
        let mut brk = false;
        for c in t.chars() {
            if self.ifs_ws.contains(c) {
                if self.active {
                    brk = true;
                }
                continue;
            }
            if self.ifs_other.contains(c) {
                self.force_break();
                brk = false;
                continue;
            }
            if brk {
                self.end_field();
                brk = false;
            }
            self.cur.s.push(c);
            self.cur.pat.push(c);
            self.active = true;
        }
        if brk {
            self.end_field();
        }
    }
    fn end_field(&mut self) {
        if self.active {
            self.fields.push(std::mem::take(&mut self.cur));
        } else {
            self.cur = Field::default();
        }
        self.active = false;
    }
    fn force_break(&mut self) {
        self.fields.push(std::mem::take(&mut self.cur));
        self.active = false;
    }
    /// Separator between elements of an unquoted list expansion.
    fn list_break(&mut self, sep: &str) {
        if self.split {
            self.end_field();
        } else {
            self.lit(sep);
        }
    }
    pub fn finish(mut self) -> Vec<Field> {
        self.end_field();
        self.fields
    }
}

#[derive(Clone, Copy)]
struct Cx {
    dq: bool,
    operand: bool,
}

enum PR<'a> {
    Str(String),
    List(Vec<String>, bool),
    Unset,
    Word(&'a Word),
}

#[derive(Clone)]
enum Idx {
    At,
    Star,
    Expr(String),
}

// ---------- brace expansion ----------

#[derive(Clone)]
enum BI<'a> {
    C(char),
    P(&'a WordPart),
    /// Quoted text produced by expansion (e.g. a backslash from a {Y..b} range).
    Q(&'static str),
}

fn brace_seq(s: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = s.split("..").collect();
    if parts.len() != 2 && parts.len() != 3 {
        return None;
    }
    let step: i64 = if parts.len() == 3 { parts[2].parse().ok()? } else { 1 };
    let step = if step == 0 { 1 } else { step.abs() };
    if let (Ok(a), Ok(b)) = (parts[0].parse::<i64>(), parts[1].parse::<i64>()) {
        let pad = |x: &str| {
            let t = x.trim_start_matches('-');
            t.len() > 1 && t.starts_with('0')
        };
        let width = if pad(parts[0]) || pad(parts[1]) { parts[0].len().max(parts[1].len()) } else { 0 };
        let mut out = Vec::new();
        let mut i = a;
        loop {
            if (a <= b && i > b) || (a > b && i < b) {
                break;
            }
            if width > 0 {
                if i < 0 {
                    out.push(format!("-{:0w$}", -i, w = width - 1));
                } else {
                    out.push(format!("{:0w$}", i, w = width));
                }
            } else {
                out.push(i.to_string());
            }
            if a <= b {
                i += step;
            } else {
                i -= step;
            }
            if out.len() > 1_000_000 {
                break;
            }
        }
        return Some(out);
    }
    let ca: Vec<char> = parts[0].chars().collect();
    let cb: Vec<char> = parts[1].chars().collect();
    if ca.len() == 1 && cb.len() == 1 && ca[0].is_ascii() && cb[0].is_ascii() {
        let (a, b) = (ca[0] as i64, cb[0] as i64);
        let mut out = Vec::new();
        let mut i = a;
        loop {
            if (a <= b && i > b) || (a > b && i < b) {
                break;
            }
            out.push(((i as u8) as char).to_string());
            if a <= b {
                i += step;
            } else {
                i -= step;
            }
        }
        return Some(out);
    }
    None
}

fn brace_items<'a>(items: &[BI<'a>]) -> Vec<Vec<BI<'a>>> {
    brace_items_x(items).0
}

fn brace_items_x<'a>(items: &[BI<'a>]) -> (Vec<Vec<BI<'a>>>, bool) {
    let n = items.len();
    let mut i = 0;
    while i < n {
        if let BI::C('{') = items[i] {
            // find matching close and commas at depth 1
            let mut depth = 0;
            let mut commas = Vec::new();
            let mut close = None;
            let mut j = i;
            while j < n {
                match items[j] {
                    BI::C('{') => depth += 1,
                    BI::C('}') => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(j);
                            break;
                        }
                    }
                    BI::C(',') if depth == 1 => commas.push(j),
                    _ => {}
                }
                j += 1;
            }
            if let Some(cl) = close {
                let prefix = &items[..i];
                let suffix = &items[cl + 1..];
                if !commas.is_empty() {
                    let mut out = Vec::new();
                    let mut start = i + 1;
                    let mut bounds = commas.clone();
                    bounds.push(cl);
                    for b in bounds {
                        let alt = &items[start..b];
                        let mut v: Vec<BI> = prefix.to_vec();
                        v.extend_from_slice(alt);
                        v.extend_from_slice(suffix);
                        out.extend(brace_items(&v));
                        start = b + 1;
                    }
                    return (out, true);
                }
                // sequence?
                let inner = &items[i + 1..cl];
                if inner.iter().all(|x| matches!(x, BI::C(_))) {
                    let s: String = inner.iter().map(|x| if let BI::C(c) = x { *c } else { ' ' }).collect();
                    if let Some(seq) = brace_seq(&s) {
                        let mut out = Vec::new();
                        for e in seq {
                            let mut v: Vec<BI> = prefix.to_vec();
                            if e == "\\" {
                                v.push(BI::Q(""));
                            } else {
                                v.extend(e.chars().map(BI::C));
                            }
                            v.extend_from_slice(suffix);
                            out.extend(brace_items(&v));
                        }
                        return (out, true);
                    }
                }
            }
        }
        i += 1;
    }
    (vec![items.to_vec()], false)
}

pub fn brace_expand(w: &Word) -> Option<Vec<Word>> {
    let has = w.parts.iter().any(|p| matches!(p, WordPart::Lit(s) if s.contains('{')));
    if !has {
        return None;
    }
    let mut items = Vec::new();
    for p in &w.parts {
        match p {
            WordPart::Lit(s) => items.extend(s.chars().map(BI::C)),
            other => items.push(BI::P(other)),
        }
    }
    let (res, expanded) = brace_items_x(&items);
    if !expanded {
        return None;
    }
    let mut words = Vec::new();
    for r in res {
        let mut parts = Vec::new();
        let mut lit = String::new();
        for it in r {
            match it {
                BI::C(c) => lit.push(c),
                BI::P(p) => {
                    if !lit.is_empty() {
                        parts.push(WordPart::Lit(std::mem::take(&mut lit)));
                    }
                    parts.push(p.clone());
                }
                BI::Q(q) => {
                    if !lit.is_empty() {
                        parts.push(WordPart::Lit(std::mem::take(&mut lit)));
                    }
                    parts.push(WordPart::SQuoted(q.to_string()));
                }
            }
        }
        if !lit.is_empty() {
            parts.push(WordPart::Lit(lit));
        }
        words.push(Word { parts });
    }
    Some(words)
}

pub fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "_-./:=,+@%^".contains(c)) {
        return s.to_string();
    }
    if s.chars().any(|c| c.is_control()) {
        let mut o = String::from("$'");
        for c in s.chars() {
            match c {
                '\n' => o.push_str("\\n"),
                '\t' => o.push_str("\\t"),
                '\r' => o.push_str("\\r"),
                '\x1b' => o.push_str("\\E"),
                '\'' => o.push_str("\\'"),
                '\\' => o.push_str("\\\\"),
                c if c.is_control() => o.push_str(&format!("\\{:03o}", c as u32)),
                c => o.push(c),
            }
        }
        o.push('\'');
        return o;
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Interpret backslash escapes as in $'...'.
pub fn ansi_c_unescape(s: &str) -> String {
    let (b, _) = crate::builtins::unescape(s, crate::builtins::Esc::AnsiC);
    String::from_utf8_lossy(&b).into_owned()
}

impl Shell {
    // ---------- public entry points ----------

    pub fn expand_words(&mut self, words: &[Word]) -> Result<Vec<String>, Flow> {
        let mut out = Vec::new();
        for w in words {
            self.expand_word_into(w, &mut out)?;
        }
        Ok(out)
    }

    pub fn expand_word_into(&mut self, w: &Word, out: &mut Vec<String>) -> Result<(), Flow> {
        if self.o[O_BRACEEXPAND] {
            if let Some(ws) = brace_expand(w) {
                for bw in &ws {
                    self.expand_one_word(bw, out)?;
                }
                return Ok(());
            }
        }
        self.expand_one_word(w, out)
    }

    fn expand_one_word(&mut self, w: &Word, out: &mut Vec<String>) -> Result<(), Flow> {
        // fast path: plain literal without glob chars
        if let [WordPart::Lit(s)] = w.parts.as_slice() {
            if !pattern::has_glob_chars(s) || self.o[O_NOGLOB] {
                out.push(s.clone());
                return Ok(());
            }
        }
        let ifs = self.ifs();
        let mut fb = FB::new(true, &ifs);
        self.expand_parts(&w.parts, &mut fb, Cx { dq: false, operand: false })?;
        for f in fb.finish() {
            if !self.o[O_NOGLOB] && pattern::has_glob_chars(&f.pat) && Pattern::new(&f.pat, false).literal().is_none() {
                let matches = self.glob(&f.pat);
                if matches.is_empty() {
                    if self.sh[sho::nullglob()] {
                        continue;
                    }
                    if self.sh[sho::failglob()] {
                        self.eprint(&format!("no match: {}", f.s));
                        return Err(Flow::Abort(1));
                    }
                    out.push(f.s);
                } else {
                    out.extend(matches);
                }
                continue;
            }
            out.push(f.s);
        }
        Ok(())
    }

    pub fn c_locale(&mut self) -> bool {
        for v in ["LC_ALL", "LC_COLLATE", "LANG"] {
            if let Some(s) = self.get_str(v) {
                if !s.is_empty() {
                    return s == "C" || s == "POSIX" || s.starts_with("C.");
                }
            }
        }
        true
    }

    pub fn glob(&mut self, pat: &str) -> Vec<String> {
        let ignore = self.get_str("GLOBIGNORE").unwrap_or_default();
        let opts = pattern::GlobOpts {
            dotglob: self.sh[sho::dotglob()] || !ignore.is_empty(),
            nocaseglob: self.sh[sho::nocaseglob()],
            globstar: self.sh[sho::globstar()],
            extglob: self.sh[sho::extglob()],
            c_locale: self.c_locale(),
        };
        let cwd = self.cwd.clone();
        let mut v = pattern::glob(pat, opts, &|p| crate::path::resolve(p, &cwd));
        if !ignore.is_empty() {
            let pats: Vec<Pattern> = ignore.split(':').filter(|s| !s.is_empty()).map(|s| Pattern::new_ext(s, false, opts.extglob)).collect();
            v.retain(|m| {
                let base = m.rsplit('/').next().unwrap_or(m);
                base != "." && base != ".." && !pats.iter().any(|p| p.matches(m))
            });
        }
        v
    }

    /// Expand without splitting or globbing (assignments, case words, here-strings, `[[`).
    pub fn expand_word_str(&mut self, w: &Word) -> Result<String, Flow> {
        if let [WordPart::Lit(s)] = w.parts.as_slice() {
            return Ok(s.clone());
        }
        let ifs = self.ifs();
        let mut fb = FB::new(false, &ifs);
        self.expand_parts(&w.parts, &mut fb, Cx { dq: false, operand: false })?;
        let f = fb.finish();
        Ok(f.into_iter().map(|f| f.s).collect::<Vec<_>>().join(" "))
    }

    /// Expand to a pattern string (quoted parts escaped).
    pub fn expand_word_pat(&mut self, w: &Word) -> Result<String, Flow> {
        let ifs = self.ifs();
        let mut fb = FB::new(false, &ifs);
        self.expand_parts(&w.parts, &mut fb, Cx { dq: false, operand: false })?;
        let f = fb.finish();
        Ok(f.into_iter().map(|f| f.pat).collect::<Vec<_>>().join(" "))
    }

    /// Expand a `=~` operand: quoted parts are regex-escaped.
    pub fn expand_word_regex(&mut self, w: &Word) -> Result<String, Flow> {
        let mut out = String::new();
        for p in &w.parts {
            match p {
                WordPart::Lit(s) => out.push_str(s),
                WordPart::SQuoted(s) | WordPart::Escaped(s) => out.push_str(&crate::regex::escape(s)),
                WordPart::DQuoted(_) => {
                    let s = self.expand_word_str(&Word { parts: vec![p.clone()] })?;
                    out.push_str(&crate::regex::escape(&s));
                }
                other => {
                    let s = self.expand_word_str(&Word { parts: vec![other.clone()] })?;
                    out.push_str(&s);
                }
            }
        }
        Ok(out)
    }

    /// Expand a string as if it were the body of an unquoted here-document.
    pub fn expand_string(&mut self, s: &str) -> Result<String, Flow> {
        match crate::parser::parse_heredoc_text(s, None) {
            Ok(w) => self.expand_word_str(&w),
            Err(_) => Ok(s.to_string()),
        }
    }

    // ---------- parts ----------

    fn expand_parts(&mut self, parts: &[WordPart], fb: &mut FB, cx: Cx) -> Result<(), Flow> {
        for p in parts {
            match p {
                WordPart::Lit(t) => {
                    if cx.dq {
                        fb.quoted(t)
                    } else if cx.operand {
                        fb.unquoted(t)
                    } else {
                        fb.lit(t)
                    }
                }
                WordPart::Escaped(t) | WordPart::SQuoted(t) => fb.quoted(t),
                WordPart::DQuoted(inner) => {
                    let only_at = inner.len() == 1
                        && matches!(&inner[0], WordPart::Param(pe) if pe.name == "@" || pe.index.as_ref().and_then(|w| w.as_plain()).map(|s| s == "@").unwrap_or(false) || matches!(pe.op, ParamOp::Keys{..} | ParamOp::PrefixNames{..}));
                    if !only_at {
                        fb.mark_active();
                    }
                    self.expand_parts(inner, fb, Cx { dq: true, operand: cx.operand })?;
                }
                WordPart::Tilde(u) => match self.tilde(u) {
                    Some(v) => fb.quoted(&v),
                    None => fb.lit(&format!("~{}", u)),
                },
                WordPart::Param(pe) => self.expand_param_into(pe, fb, cx)?,
                WordPart::CmdSubst(prog) | WordPart::Backtick(prog) => {
                    let s = self.command_subst(prog)?;
                    if cx.dq {
                        fb.quoted(&s)
                    } else {
                        fb.unquoted(&s)
                    }
                }
                WordPart::Arith(w) => {
                    let v = self.arith_word(w)?;
                    if cx.dq {
                        fb.quoted(&v.to_string())
                    } else {
                        fb.unquoted(&v.to_string())
                    }
                }
                WordPart::ProcSubst { input, prog } => {
                    let path = self.proc_subst(*input, prog)?;
                    fb.quoted(&path);
                }
                WordPart::DeclAssign(a) => {
                    let s = WordPart::DeclAssign(a.clone());
                    let mut src = String::new();
                    s.write_source(&mut src);
                    fb.quoted(&src);
                }
            }
        }
        Ok(())
    }

    pub fn arith_word(&mut self, w: &Word) -> Result<i64, Flow> {
        let s = self.expand_word_str(w)?;
        match crate::arith::eval(&s, self) {
            Ok(v) => Ok(v),
            Err(e) => {
                self.eprint(&format!("{}: {}", s.trim_start(), e));
                Err(Flow::Abort(1))
            }
        }
    }

    pub fn tilde(&mut self, user: &str) -> Option<String> {
        match user {
            "" => Some(self.get_str("HOME").unwrap_or_else(|| crate::path::path_str(&home_dir()))),
            "+" => self.get_str("PWD"),
            "-" => self.get_str("OLDPWD"),
            u => crate::sys::user_home(u),
        }
    }

    // ---------- parameters ----------

    fn expand_param_into(&mut self, pe: &ParamExp, fb: &mut FB, cx: Cx) -> Result<(), Flow> {
        match self.param(pe, cx.dq)? {
            PR::Str(s) => {
                if cx.dq {
                    fb.quoted(&s)
                } else {
                    fb.unquoted(&s)
                }
            }
            PR::Unset => {}
            PR::List(v, star) => {
                let ifs = self.ifs();
                if cx.dq {
                    if star {
                        let sep: String = ifs.chars().next().map(|c| c.to_string()).unwrap_or_default();
                        fb.quoted(&v.join(&sep));
                    } else {
                        for (i, x) in v.iter().enumerate() {
                            if i > 0 {
                                fb.force_break();
                            }
                            fb.quoted(x);
                        }
                    }
                } else if fb.split && !ifs.is_empty() {
                    let sep: String = ifs.chars().next().map(|c| c.to_string()).unwrap_or_default();
                    if sep.chars().all(|c| c == ' ' || c == '\t' || c == '\n') {
                        for (i, x) in v.iter().enumerate() {
                            if i > 0 {
                                fb.list_break(&sep);
                            }
                            fb.unquoted(x);
                        }
                    } else {
                        fb.unquoted(&v.join(&sep));
                    }
                } else {
                    let sep = if star { ifs.chars().next().map(|c| c.to_string()).unwrap_or_default() } else { " ".into() };
                    for (i, x) in v.iter().enumerate() {
                        if i > 0 {
                            fb.list_break(&sep);
                        }
                        fb.unquoted(x);
                    }
                }
            }
            PR::Word(w) => {
                self.expand_parts(&w.parts, fb, Cx { dq: cx.dq, operand: true })?;
            }
        }
        Ok(())
    }

    fn unbound(&mut self, name: &str) -> Flow {
        self.eprint(&format!("{}: unbound variable", name));
        Flow::Fatal(1)
    }

    fn eval_index(&mut self, w: &Word) -> Result<Idx, Flow> {
        if let Some(p) = w.as_plain() {
            if p == "@" {
                return Ok(Idx::At);
            }
            if p == "*" {
                return Ok(Idx::Star);
            }
        }
        Ok(Idx::Expr(self.expand_word_str(w)?))
    }

    fn positional_param(&self, n: usize) -> Option<String> {
        if n == 0 {
            Some(self.arg0.clone())
        } else {
            self.positional.get(n - 1).cloned()
        }
    }

    /// Base lookup: returns (Str | List | Unset).
    fn lookup(&mut self, name: &str, idx: Option<&Idx>) -> Result<PR<'static>, Flow> {
        if let Some(c) = name.chars().next() {
            if c.is_ascii_digit() {
                let n: usize = name.parse().unwrap_or(usize::MAX);
                return Ok(match self.positional_param(n) {
                    Some(s) => PR::Str(s),
                    None => PR::Unset,
                });
            }
        }
        match name {
            "@" => return Ok(PR::List(self.positional.clone(), false)),
            "*" => return Ok(PR::List(self.positional.clone(), true)),
            "#" => return Ok(PR::Str(self.positional.len().to_string())),
            "?" => return Ok(PR::Str(self.status.to_string())),
            "-" => return Ok(PR::Str(self.flags_string())),
            "$" => return Ok(PR::Str(self.pid.to_string())),
            "!" => {
                return Ok(match self.last_bg {
                    Some(p) => PR::Str(p.to_string()),
                    None => PR::Unset,
                })
            }
            _ => {}
        }
        let val = self.get_value(name);
        Ok(match idx {
            None => match val {
                None => PR::Unset,
                Some(Value::Str(s)) => PR::Str(s),
                Some(Value::Arr(a)) => match a.get(&0) {
                    Some(s) => PR::Str(s.clone()),
                    None => PR::Unset,
                },
                Some(Value::Assoc(m)) => match m.get("0") {
                    Some(s) => PR::Str(s.clone()),
                    None => PR::Unset,
                },
            },
            Some(Idx::At) | Some(Idx::Star) => {
                let star = matches!(idx, Some(Idx::Star));
                match val {
                    None => PR::Unset,
                    Some(v) => PR::List(v.values(), star),
                }
            }
            Some(Idx::Expr(e)) => match val {
                None => PR::Unset,
                Some(Value::Assoc(m)) => match m.get(e.as_str()) {
                    Some(s) => PR::Str(s.clone()),
                    None => PR::Unset,
                },
                Some(Value::Arr(a)) => {
                    let mut i = match crate::arith::eval(e, self) {
                        Ok(i) => i,
                        Err(er) => {
                            self.eprint(&format!("{}: {}", e, er));
                            return Err(Flow::Abort(1));
                        }
                    };
                    if i < 0 {
                        let max = a.keys().next_back().map(|k| k + 1).unwrap_or(0);
                        i += max;
                        if i < 0 {
                            self.eprint(&format!("{}[{}]: bad array subscript", name, e));
                            return Err(Flow::Abort(1));
                        }
                    }
                    match a.get(&i) {
                        Some(s) => PR::Str(s.clone()),
                        None => PR::Unset,
                    }
                }
                Some(Value::Str(s)) => {
                    let i = crate::arith::eval(e, self).unwrap_or(-1);
                    if i == 0 || i == -1 {
                        PR::Str(s)
                    } else {
                        PR::Unset
                    }
                }
            },
        })
    }

    fn param<'a>(&mut self, pe: &'a ParamExp, dq: bool) -> Result<PR<'a>, Flow> {
        if let ParamOp::Bad(text) = &pe.op {
            self.eprint(&format!("${{{}}}: bad substitution", text));
            return Err(Flow::Abort(1));
        }
        // resolve name/index (with indirection)
        let (name, idx): (String, Option<Idx>) = if pe.indirect {
            let idx0 = match &pe.index {
                Some(w) => Some(self.eval_index(w)?),
                None => None,
            };
            let target = match self.lookup(&pe.name, idx0.as_ref())? {
                PR::Str(s) => s,
                PR::List(v, _) => v.join(" "),
                _ => {
                    if self.o[O_NOUNSET] {
                        return Err(self.unbound(&pe.name));
                    }
                    String::new()
                }
            };
            if target.is_empty() {
                if matches!(pe.op, ParamOp::None) {
                    self.eprint(&format!("{}: invalid indirect expansion", pe.name));
                    return Err(Flow::Abort(1));
                }
                (String::new(), None)
            } else if let Some(b) = target.find('[') {
                if target.ends_with(']') {
                    let n = target[..b].to_string();
                    let sub = &target[b + 1..target.len() - 1];
                    let i = match sub {
                        "@" => Idx::At,
                        "*" => Idx::Star,
                        s => Idx::Expr(s.to_string()),
                    };
                    (n, Some(i))
                } else {
                    (target, None)
                }
            } else {
                (target, None)
            }
        } else {
            let idx = match &pe.index {
                Some(w) => Some(self.eval_index(w)?),
                None => None,
            };
            (pe.name.clone(), idx)
        };

        match &pe.op {
            ParamOp::PrefixNames { star } => {
                let mut names: Vec<String> = self.vars.names().into_iter().filter(|n| n.starts_with(&pe.name)).collect();
                names.sort();
                return Ok(PR::List(names, *star));
            }
            ParamOp::Keys { star } => {
                let v = self.get_value(&pe.name);
                let keys = match v {
                    None => vec![],
                    Some(v) => v.keys(),
                };
                return Ok(PR::List(keys, *star));
            }
            _ => {}
        }

        let base = if name.is_empty() { PR::Unset } else { self.lookup(&name, idx.as_ref())? };
        let is_list_name = name == "@" || name == "*" || matches!(idx, Some(Idx::At) | Some(Idx::Star));

        let null_or_unset = |b: &PR, colon: bool| -> bool {
            match b {
                PR::Unset => true,
                PR::Str(s) => colon && s.is_empty(),
                PR::List(v, _) => v.is_empty() || (colon && v.iter().all(|x| x.is_empty()) && v.len() <= 1),
                PR::Word(_) => false,
            }
        };

        match &pe.op {
            ParamOp::None => {
                if matches!(base, PR::Unset) && self.o[O_NOUNSET] && !is_list_name {
                    let n = display_name(pe);
                    return Err(self.unbound(&n));
                }
                if let PR::Unset = base {
                    if is_list_name && self.o[O_NOUNSET] && name != "@" && name != "*" {
                        let n = display_name(pe);
                        return Err(self.unbound(&n));
                    }
                }
                Ok(base)
            }
            ParamOp::Length => {
                let n = match &base {
                    PR::Unset => {
                        if self.o[O_NOUNSET] && !is_list_name {
                            let n = display_name(pe);
                            return Err(self.unbound(&n));
                        }
                        0
                    }
                    PR::Str(s) => s.chars().count(),
                    PR::List(v, _) => v.len(),
                    PR::Word(_) => 0,
                };
                Ok(PR::Str(n.to_string()))
            }
            ParamOp::Default { colon, word } => {
                if null_or_unset(&base, *colon) {
                    Ok(PR::Word(word))
                } else {
                    Ok(base)
                }
            }
            ParamOp::Alt { colon, word } => {
                if null_or_unset(&base, *colon) {
                    if dq {
                        Ok(PR::Str(String::new()))
                    } else {
                        Ok(PR::Unset)
                    }
                } else {
                    Ok(PR::Word(word))
                }
            }
            ParamOp::Assign { colon, word } => {
                if null_or_unset(&base, *colon) {
                    let v = self.expand_word_str(word)?;
                    if name.chars().next().map(|c| !(c.is_ascii_alphabetic() || c == '_')).unwrap_or(true) {
                        self.eprint(&format!("${}: cannot assign in this way", name));
                        return Err(Flow::Abort(1));
                    }
                    let r = match &idx {
                        Some(Idx::Expr(k)) => self.set_elem(&name, k, &v, false),
                        _ => self.set_var(&name, &v, false),
                    };
                    if let Err(e) = r {
                        self.eprint(&e);
                        return Err(Flow::Abort(1));
                    }
                    Ok(PR::Str(v))
                } else {
                    Ok(base)
                }
            }
            ParamOp::Error { colon, word } => {
                if null_or_unset(&base, *colon) {
                    let msg = match word {
                        Some(w) => self.expand_word_str(w)?,
                        None => {
                            if matches!(base, PR::Unset) {
                                "parameter null or not set".into()
                            } else {
                                "parameter null or not set".into()
                            }
                        }
                    };
                    let n = display_name(pe);
                    self.eprint(&format!("{}: {}", n, msg));
                    return Err(Flow::Fatal(1));
                }
                Ok(base)
            }
            _ => {
                if matches!(base, PR::Unset) {
                    if self.o[O_NOUNSET] && !is_list_name {
                        let n = display_name(pe);
                        return Err(self.unbound(&n));
                    }
                    if !matches!(pe.op, ParamOp::Substr { .. } | ParamOp::Transform(_)) {
                        return Ok(PR::Unset);
                    }
                }
                self.apply_op(pe, &name, idx.as_ref(), base)
            }
        }
    }

    fn apply_op<'a>(&mut self, pe: &'a ParamExp, name: &str, idx: Option<&Idx>, base: PR<'a>) -> Result<PR<'a>, Flow> {
        let nocase = self.sh[sho::nocasematch()];
        match &pe.op {
            ParamOp::TrimPrefix { longest, pat } | ParamOp::TrimSuffix { longest, pat } => {
                let p = self.expand_word_pat(pat)?;
                let prefix = matches!(pe.op, ParamOp::TrimPrefix { .. });
                let ext = self.sh[sho::extglob()];
                let f = |s: &str| {
                    if prefix {
                        pattern::trim_prefix(s, &p, *longest, nocase, ext)
                    } else {
                        pattern::trim_suffix(s, &p, *longest, nocase, ext)
                    }
                };
                Ok(map_pr(base, f))
            }
            ParamOp::Replace { mode, pat, rep } => {
                let p = self.expand_word_pat(pat)?;
                let (rep_s, rep_pat) = match rep {
                    Some(w) => {
                        let ifs = self.ifs();
                        let mut fb = FB::new(false, &ifs);
                        self.expand_parts(&w.parts, &mut fb, Cx { dq: false, operand: false })?;
                        let f = fb.finish();
                        let s: String = f.iter().map(|x| x.s.clone()).collect::<Vec<_>>().join(" ");
                        let pt: String = f.iter().map(|x| x.pat.clone()).collect::<Vec<_>>().join(" ");
                        (s, pt)
                    }
                    None => (String::new(), String::new()),
                };
                let amp = self.sh[sho::patsub_replacement()] && rep_pat.contains('&');
                let ext = self.sh[sho::extglob()];
                let mode = *mode;
                let f = |s: &str| {
                    if p.is_empty() && !matches!(mode, ReplMode::Prefix | ReplMode::Suffix) {
                        return s.to_string();
                    }
                    let (chars, ms) = pattern::find_matches(s, &p, mode, nocase, ext);
                    if ms.is_empty() {
                        return s.to_string();
                    }
                    let mut out = String::new();
                    let mut last = 0;
                    for (a, b) in ms {
                        out.extend(&chars[last..a]);
                        let matched: String = chars[a..b].iter().collect();
                        if amp {
                            // unescaped & -> matched text; \x -> x
                            let mut it = rep_pat.chars();
                            while let Some(c) = it.next() {
                                if c == '\\' {
                                    if let Some(n) = it.next() {
                                        out.push(n);
                                    }
                                } else if c == '&' {
                                    out.push_str(&matched);
                                } else {
                                    out.push(c);
                                }
                            }
                        } else {
                            out.push_str(&rep_s);
                        }
                        last = b;
                    }
                    out.extend(&chars[last..]);
                    out
                };
                Ok(map_pr(base, f))
            }
            ParamOp::Substr { offset, length } => {
                let off = self.arith_word(offset)?;
                let len = match length {
                    Some(w) => Some(self.arith_word(w)?),
                    None => None,
                };
                match base {
                    PR::Unset => Ok(PR::Unset),
                    PR::Str(s) => {
                        let c: Vec<char> = s.chars().collect();
                        let n = c.len() as i64;
                        let mut start = if off < 0 { n + off } else { off };
                        if start < 0 || start > n {
                            if off < 0 && start < 0 {
                                return Ok(PR::Str(String::new()));
                            }
                            start = start.clamp(0, n);
                        }
                        let end = match len {
                            None => n,
                            Some(l) if l < 0 => {
                                let e = n + l;
                                if e < start {
                                    self.eprint(&format!("{}: substring expression < 0", l));
                                    return Err(Flow::Abort(1));
                                }
                                e
                            }
                            Some(l) => (start + l).min(n),
                        };
                        Ok(PR::Str(c[start as usize..end.max(start) as usize].iter().collect()))
                    }
                    PR::List(v, star) => {
                        let positional = name == "@" || name == "*";
                        if positional {
                            let mut all = vec![self.arg0.clone()];
                            all.extend(v);
                            let n = all.len() as i64;
                            let start = if off < 0 { n + off } else { off };
                            if start < 0 || start >= n {
                                return Ok(PR::List(vec![], star));
                            }
                            let end = match len {
                                None => n,
                                Some(l) if l < 0 => {
                                    self.eprint(&format!("{}: substring expression < 0", l));
                                    return Err(Flow::Abort(1));
                                }
                                Some(l) => (start + l).min(n),
                            };
                            return Ok(PR::List(all[start as usize..end as usize].to_vec(), star));
                        }
                        // arrays: by index
                        let arr = match self.get_value(name) {
                            Some(Value::Arr(a)) => a,
                            Some(Value::Assoc(m)) => {
                                let vals: Vec<String> = m.values().cloned().collect();
                                let n = vals.len() as i64;
                                let start = if off < 0 { (n + off).max(0) } else { off.min(n) };
                                let end = match len {
                                    None => n,
                                    Some(l) => (start + l.max(0)).min(n),
                                };
                                return Ok(PR::List(vals[start as usize..end as usize].to_vec(), star));
                            }
                            Some(Value::Str(s)) => {
                                let mut a = std::collections::BTreeMap::new();
                                a.insert(0, s);
                                a
                            }
                            None => return Ok(PR::List(vec![], star)),
                        };
                        let _ = idx;
                        let max = arr.keys().next_back().map(|k| k + 1).unwrap_or(0);
                        let start = if off < 0 { max + off } else { off };
                        if start < 0 {
                            return Ok(PR::List(vec![], star));
                        }
                        if let Some(l) = len {
                            if l < 0 {
                                self.eprint(&format!("{}: substring expression < 0", l));
                                return Err(Flow::Abort(1));
                            }
                        }
                        let mut out = Vec::new();
                        for (_, val) in arr.range(start..) {
                            if let Some(l) = len {
                                if out.len() as i64 >= l {
                                    break;
                                }
                            }
                            out.push(val.clone());
                        }
                        Ok(PR::List(out, star))
                    }
                    other => Ok(other),
                }
            }
            ParamOp::Case { upper, all, pat } => {
                let p = match pat {
                    Some(w) => Some(Pattern::new(&self.expand_word_pat(w)?, false)),
                    None => None,
                };
                let upper = *upper;
                let all = *all;
                let f = |s: &str| {
                    let mut out = String::new();
                    for (i, c) in s.chars().enumerate() {
                        let applies = (all || i == 0) && p.as_ref().map(|p| p.matches(&c.to_string())).unwrap_or(true);
                        if applies {
                            if upper {
                                out.extend(c.to_uppercase());
                            } else {
                                out.extend(c.to_lowercase());
                            }
                        } else {
                            out.push(c);
                        }
                    }
                    out
                };
                Ok(map_pr(base, f))
            }
            ParamOp::Toggle { all, pat } => {
                let p = match pat {
                    Some(w) => Some(Pattern::new(&self.expand_word_pat(w)?, false)),
                    None => None,
                };
                let all = *all;
                let f = |s: &str| {
                    let mut out = String::new();
                    for (i, c) in s.chars().enumerate() {
                        let applies = (all || i == 0) && p.as_ref().map(|p| p.matches(&c.to_string())).unwrap_or(true);
                        if applies && c.is_lowercase() {
                            out.extend(c.to_uppercase());
                        } else if applies && c.is_uppercase() {
                            out.extend(c.to_lowercase());
                        } else {
                            out.push(c);
                        }
                    }
                    out
                };
                Ok(map_pr(base, f))
            }
            ParamOp::Transform(t) => {
                let t = *t;
                match t {
                    'Q' => Ok(map_pr(base, |s| shell_quote_always(s))),
                    'E' => Ok(map_pr(base, ansi_c_unescape)),
                    'P' => {
                        let r = match base {
                            PR::Str(s) => PR::Str(crate::prompt::expand_prompt(self, &s)),
                            PR::List(v, st) => PR::List(v.iter().map(|s| crate::prompt::expand_prompt(self, s)).collect(), st),
                            o => o,
                        };
                        Ok(r)
                    }
                    'U' => Ok(map_pr(base, |s| s.to_uppercase())),
                    'L' => Ok(map_pr(base, |s| s.to_lowercase())),
                    'u' => Ok(map_pr(base, |s| {
                        let mut c = s.chars();
                        match c.next() {
                            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                            None => String::new(),
                        }
                    })),
                    'a' => {
                        let a = self.var_attrs(name);
                        Ok(PR::Str(attr_letters(a, self.get_value(name).as_ref())))
                    }
                    'A' => {
                        let attrs = self.var_attrs(name);
                        let s = match self.get_value(name) {
                            Some(Value::Str(v)) => {
                                let flags = attr_letters(attrs, None);
                                if flags.is_empty() {
                                    format!("{}={}", name, shell_quote_always(&v))
                                } else {
                                    format!("declare -{} {}={}", flags, name, shell_quote_always(&v))
                                }
                            }
                            Some(_) => crate::builtins::declare_line(self, name).unwrap_or_default(),
                            None => String::new(),
                        };
                        Ok(PR::Str(s))
                    }
                    'K' | 'k' => {
                        let v = self.get_value(name);
                        match v {
                            Some(Value::Assoc(m)) => {
                                let items: Vec<String> = m.iter().map(|(k, v)| format!("{} {}", shell_quote_always(k), shell_quote_always(v))).collect();
                                Ok(PR::Str(items.join(" ")))
                            }
                            Some(Value::Arr(a)) => {
                                let items: Vec<String> = a.iter().map(|(k, v)| format!("{} {}", k, shell_quote_always(v))).collect();
                                Ok(PR::Str(items.join(" ")))
                            }
                            _ => Ok(map_pr(base, |s| shell_quote_always(s))),
                        }
                    }
                    _ => {
                        self.eprint(&format!("{}: bad substitution", name));
                        Err(Flow::Abort(1))
                    }
                }
            }
            _ => Ok(base),
        }
    }

    // ---------- command / process substitution ----------

    pub fn command_subst(&mut self, prog: &Program) -> Result<String, Flow> {
        // $(< file)
        if prog.items.len() == 1 {
            let ao = &prog.items[0].and_or;
            if ao.rest.is_empty() && ao.first.cmds.len() == 1 && !ao.first.negate {
                if let Command::Simple(sc) = &ao.first.cmds[0].0 {
                    if sc.words.is_empty() && sc.assigns.is_empty() && sc.redirs.len() == 1 && sc.redirs[0].op == RedirOp::In && sc.redirs[0].fd.is_none() {
                        if let RedirTarget::Word(w) = &sc.redirs[0].target {
                            let p = self.expand_word_str(w)?;
                            let wp = crate::path::resolve(&p, &self.cwd);
                            return match std::fs::read(&wp) {
                                Ok(b) => {
                                    self.status = 0;
                                    self.cmdsubst_status = Some(0);
                                    Ok(clean_subst_output(b))
                                }
                                Err(e) => {
                                    self.eprint(&format!("{}: {}", p, crate::exec::io_err_msg(&e)));
                                    self.status = 1;
                                    self.cmdsubst_status = Some(1);
                                    Ok(String::new())
                                }
                            };
                        }
                    }
                }
            }
        }
        let buf = Arc::new(Mutex::new(Vec::new()));
        let mut sub = self.subshell_clone();
        sub.fds.set(1, Some(Handle::Buf(buf.clone())));
        if !self.sh[sho::inherit_errexit()] && !self.o[O_POSIX] {
            sub.o[O_ERREXIT] = false;
        }
        let r = sub.exec_list(&prog.items);
        let st = sub.finish_subshell(r);
        drop(sub);
        self.status = st;
        self.cmdsubst_status = Some(st);
        let bytes = std::mem::take(&mut *buf.lock().unwrap());
        Ok(clean_subst_output(bytes))
    }

    fn proc_subst(&mut self, input: bool, prog: &Arc<Program>) -> Result<String, Flow> {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let path = loop {
            let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!("gash-ps-{}-{}", std::process::id(), n));
            match crate::sys::make_fifo(&path) {
                Ok(()) => break path,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    self.eprint(&format!("process substitution: {}", crate::exec::io_err_msg(&e)));
                    return Err(Flow::Abort(1));
                }
            }
        };
        let mut sub = self.subshell_clone();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let child_pid = Arc::new(std::sync::atomic::AtomicU32::new(0));
        sub.cancel = cancel.clone();
        sub.child_pid = child_pid.clone();
        let (fifo, prog) = (path.clone(), prog.clone());
        let thread = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(move || {
                let end = std::fs::OpenOptions::new().read(!input).write(input).open(&fifo);
                let Ok(f) = end else {
                    return 1;
                };
                sub.fds.set(if input { 1 } else { 0 }, Some(Handle::File(Arc::new(f))));
                let r = sub.exec_list(&prog.items);
                sub.finish_subshell(r)
            })
            .expect("spawn process substitution thread");
        self.procsubst.push(ProcSubst { path: path.clone(), input, thread, cancel, child_pid });
        Ok(crate::path::path_str(&path))
    }

    pub fn finish_procsubst(&mut self) {
        for ps in std::mem::take(&mut self.procsubst) {
            drop(std::fs::OpenOptions::new().read(true).write(true).open(&ps.path));
            if ps.input {
                ps.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                let pid = ps.child_pid.load(std::sync::atomic::Ordering::SeqCst);
                if pid != 0 {
                    crate::sys::kill_pid(pid as i32, crate::sys::SIGTERM);
                }
            }
            let _ = ps.thread.join();
            let _ = std::fs::remove_file(&ps.path);
        }
    }
}

fn clean_subst_output(bytes: Vec<u8>) -> String {
    let mut s = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    };
    if s.contains('\0') {
        s = s.replace('\0', "");
    }
    while s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
    s
}

fn display_name(pe: &ParamExp) -> String {
    match &pe.index {
        Some(w) => format!("{}[{}]", pe.name, w.to_source()),
        None => {
            if pe.name.chars().all(|c| c.is_ascii_digit()) {
                format!("${}", pe.name)
            } else {
                pe.name.clone()
            }
        }
    }
}

fn map_pr<'a>(b: PR<'a>, f: impl Fn(&str) -> String) -> PR<'a> {
    match b {
        PR::Str(s) => PR::Str(f(&s)),
        PR::List(v, star) => PR::List(v.iter().map(|s| f(s)).collect(), star),
        o => o,
    }
}

pub fn shell_quote_always(s: &str) -> String {
    let q = shell_quote(s);
    if q.starts_with('\'') || q.starts_with("$'") {
        q
    } else {
        format!("'{}'", s)
    }
}

pub fn attr_letters(a: u32, v: Option<&Value>) -> String {
    let mut s = String::new();
    if a & ARRAY != 0 || matches!(v, Some(Value::Arr(_))) {
        s.push('a');
    }
    if a & ASSOC != 0 || matches!(v, Some(Value::Assoc(_))) {
        s.push('A');
    }
    if a & INTEGER != 0 {
        s.push('i');
    }
    if a & LOWER != 0 {
        s.push('l');
    }
    if a & NAMEREF != 0 {
        s.push('n');
    }
    if a & READONLY != 0 {
        s.push('r');
    }
    if a & TRACE != 0 {
        s.push('t');
    }
    if a & UPPER != 0 {
        s.push('u');
    }
    if a & EXPORT != 0 {
        s.push('x');
    }
    s
}

pub fn home_dir() -> std::path::PathBuf {
    std::env::var("HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from("/"))
}

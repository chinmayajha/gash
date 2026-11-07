//! Shell builtins (part 2): printf, read, mapfile, test, declare & friends, getopts, let.

use crate::builtins::{format_function, unescape, Esc};
use crate::expand::shell_quote;
use crate::path;
use crate::shell::*;
use crate::vars::*;
use std::collections::BTreeMap;
use std::sync::Arc;

// ---------------- printf ----------------

fn parse_num_arg(s: &str) -> Result<i64, String> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(0);
    }
    if let Some(c) = t.strip_prefix('\'').or_else(|| t.strip_prefix('"')) {
        return Ok(c.chars().next().map(|c| c as i64).unwrap_or(0));
    }
    match crate::arith::parse_number(t) {
        Ok(v) => Ok(v),
        Err(_) => {
            // leading numeric prefix
            let mut end = 0;
            let b = t.as_bytes();
            if end < b.len() && (b[end] == b'-' || b[end] == b'+') {
                end += 1;
            }
            while end < b.len() && b[end].is_ascii_digit() {
                end += 1;
            }
            let v = t[..end].parse::<i64>().unwrap_or(0);
            Err(format!("{}|{}", v, t))
        }
    }
}

fn parse_float_arg(s: &str) -> Result<f64, String> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(0.0);
    }
    if let Some(c) = t.strip_prefix('\'').or_else(|| t.strip_prefix('"')) {
        return Ok(c.chars().next().map(|c| c as u32 as f64).unwrap_or(0.0));
    }
    if let Ok(v) = t.parse::<f64>() {
        return Ok(v);
    }
    if let Ok(v) = crate::arith::parse_number(t) {
        return Ok(v as f64);
    }
    Err(t.to_string())
}

fn fmt_exp(v: f64, prec: usize, upper: bool, alt: bool) -> String {
    if !v.is_finite() {
        return fmt_special(v, upper);
    }
    let s = format!("{:.*e}", prec, v);
    // Rust: 1.500000e3 -> C: 1.500000e+03
    let (mant, exp) = s.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap_or(0);
    let mut mant = mant.to_string();
    if alt && prec == 0 && !mant.contains('.') {
        mant.push('.');
    }
    let e = if upper { 'E' } else { 'e' };
    format!("{}{}{}{:02}", mant, e, if exp < 0 { '-' } else { '+' }, exp.abs())
}

fn fmt_special(v: f64, upper: bool) -> String {
    let s = if v.is_nan() {
        "nan"
    } else if v > 0.0 {
        "inf"
    } else {
        "-inf"
    };
    if upper {
        s.to_uppercase()
    } else {
        s.to_string()
    }
}

fn fmt_g(v: f64, prec: usize, upper: bool, alt: bool) -> String {
    if !v.is_finite() {
        return fmt_special(v, upper);
    }
    let p = if prec == 0 { 1 } else { prec };
    if v == 0.0 {
        return if alt { format!("{:.*}", p - 1, 0.0) } else { "0".into() };
    }
    let exp = v.abs().log10().floor() as i32;
    // C rule: use %e if exp < -4 or exp >= P
    let s = format!("{:.*e}", p - 1, v);
    let real_exp: i32 = s.split_once('e').map(|(_, e)| e.parse().unwrap_or(exp)).unwrap_or(exp);
    let mut out = if real_exp < -4 || real_exp >= p as i32 {
        fmt_exp(v, p - 1, upper, alt)
    } else {
        let decimals = (p as i32 - 1 - real_exp).max(0) as usize;
        format!("{:.*}", decimals, v)
    };
    if !alt {
        // strip trailing zeros in the fraction
        if let Some(epos) = out.find(['e', 'E']) {
            let (m, e) = out.split_at(epos);
            let mut m = m.to_string();
            if m.contains('.') {
                while m.ends_with('0') {
                    m.pop();
                }
                if m.ends_with('.') {
                    m.pop();
                }
            }
            out = format!("{}{}", m, e);
        } else if out.contains('.') {
            while out.ends_with('0') {
                out.pop();
            }
            if out.ends_with('.') {
                out.pop();
            }
        }
    }
    out
}

struct Spec {
    minus: bool,
    plus: bool,
    space: bool,
    alt: bool,
    zero: bool,
    width: Option<usize>,
    prec: Option<usize>,
}

fn pad(s: String, sp: &Spec, numeric: bool) -> String {
    let w = match sp.width {
        Some(w) => w,
        None => return s,
    };
    let len = s.chars().count();
    if len >= w {
        return s;
    }
    let n = w - len;
    if sp.minus {
        format!("{}{}", s, " ".repeat(n))
    } else if sp.zero && numeric {
        // insert zeros after sign / 0x prefix
        let (sign, rest) = if s.starts_with(['-', '+', ' ']) { s.split_at(1) } else { ("", s.as_str()) };
        let (pfx, rest) = if rest.starts_with("0x") || rest.starts_with("0X") { rest.split_at(2) } else { ("", rest) };
        format!("{}{}{}{}", sign, pfx, "0".repeat(n), rest)
    } else {
        format!("{}{}", " ".repeat(n), s)
    }
}

fn fmt_int(v: i64, conv: char, sp: &Spec) -> String {
    let mut digits = match conv {
        'd' | 'i' => v.unsigned_abs().to_string(),
        'u' => (v as u64).to_string(),
        'o' => format!("{:o}", v as u64),
        'x' => format!("{:x}", v as u64),
        'X' => format!("{:X}", v as u64),
        _ => v.to_string(),
    };
    if let Some(p) = sp.prec {
        if p == 0 && v == 0 {
            digits.clear();
        }
        while digits.len() < p {
            digits.insert(0, '0');
        }
    }
    let mut prefix = String::new();
    if matches!(conv, 'd' | 'i') {
        if v < 0 {
            prefix.push('-');
        } else if sp.plus {
            prefix.push('+');
        } else if sp.space {
            prefix.push(' ');
        }
    }
    if sp.alt {
        match conv {
            'o' if !digits.starts_with('0') => prefix.push('0'),
            'x' if v != 0 => prefix.push_str("0x"),
            'X' if v != 0 => prefix.push_str("0X"),
            _ => {}
        }
    }
    let sp2 = Spec { zero: sp.zero && sp.prec.is_none(), ..*sp };
    pad(format!("{}{}", prefix, digits), &sp2, true)
}

impl Clone for Spec {
    fn clone(&self) -> Self {
        Spec { ..*self }
    }
}
impl Copy for Spec {}

pub fn printf_format(sh: &mut Shell, fmt: &str, args: &[String], out: &mut Vec<u8>) -> i32 {
    let f: Vec<char> = fmt.chars().collect();
    let mut ai = 0usize;
    let mut status = 0;
    loop {
        let start_ai = ai;
        let mut i = 0;
        while i < f.len() {
            let c = f[i];
            if c == '\\' {
                // escape in format
                let mut j = i + 1;
                let mut esc = String::from("\\");
                if j < f.len() {
                    esc.push(f[j]);
                    let e = f[j];
                    j += 1;
                    if e.is_digit(8) {
                        let mut n = 1;
                        while n < 3 && j < f.len() && f[j].is_digit(8) {
                            esc.push(f[j]);
                            j += 1;
                            n += 1;
                        }
                    } else if e == 'x' {
                        let mut n = 0;
                        while n < 2 && j < f.len() && f[j].is_ascii_hexdigit() {
                            esc.push(f[j]);
                            j += 1;
                            n += 1;
                        }
                    } else if e == 'u' || e == 'U' {
                        let max = if e == 'u' { 4 } else { 8 };
                        let mut n = 0;
                        while n < max && j < f.len() && f[j].is_ascii_hexdigit() {
                            esc.push(f[j]);
                            j += 1;
                            n += 1;
                        }
                    }
                }
                let (b, stop) = unescape(&esc, Esc::PrintfFmt);
                out.extend_from_slice(&b);
                if stop {
                    return status;
                }
                i = j;
                continue;
            }
            if c != '%' {
                let mut b = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                i += 1;
                continue;
            }
            i += 1;
            if i >= f.len() {
                out.push(b'%');
                break;
            }
            if f[i] == '%' {
                out.push(b'%');
                i += 1;
                continue;
            }
            let mut sp = Spec { minus: false, plus: false, space: false, alt: false, zero: false, width: None, prec: None };
            while i < f.len() && "-+ #0".contains(f[i]) {
                match f[i] {
                    '-' => sp.minus = true,
                    '+' => sp.plus = true,
                    ' ' => sp.space = true,
                    '#' => sp.alt = true,
                    '0' => sp.zero = true,
                    _ => {}
                }
                i += 1;
            }
            let next_arg = |ai: &mut usize| -> Option<String> {
                let r = args.get(*ai).cloned();
                if r.is_some() {
                    *ai += 1;
                }
                r
            };
            if i < f.len() && f[i] == '*' {
                i += 1;
                let w = next_arg(&mut ai).map(|s| parse_num_arg(&s).unwrap_or(0)).unwrap_or(0);
                if w < 0 {
                    sp.minus = true;
                }
                sp.width = Some(w.unsigned_abs() as usize);
            } else {
                let mut w = String::new();
                while i < f.len() && f[i].is_ascii_digit() {
                    w.push(f[i]);
                    i += 1;
                }
                if !w.is_empty() {
                    sp.width = w.parse().ok();
                }
            }
            if i < f.len() && f[i] == '.' {
                i += 1;
                if i < f.len() && f[i] == '*' {
                    i += 1;
                    let p = next_arg(&mut ai).map(|s| parse_num_arg(&s).unwrap_or(0)).unwrap_or(0);
                    sp.prec = if p >= 0 { Some(p as usize) } else { None };
                } else {
                    let mut p = String::new();
                    while i < f.len() && f[i].is_ascii_digit() {
                        p.push(f[i]);
                        i += 1;
                    }
                    sp.prec = Some(p.parse().unwrap_or(0));
                }
            }
            // length modifiers
            while i < f.len() && "hlLjzt".contains(f[i]) {
                i += 1;
            }
            if i >= f.len() {
                break;
            }
            let conv = f[i];
            i += 1;
            match conv {
                's' => {
                    let a = next_arg(&mut ai).unwrap_or_default();
                    let a = match sp.prec {
                        Some(p) => a.chars().take(p).collect(),
                        None => a,
                    };
                    out.extend_from_slice(pad(a, &sp, false).as_bytes());
                }
                'b' => {
                    let a = next_arg(&mut ai).unwrap_or_default();
                    let (b, stop) = unescape(&a, Esc::PrintfB);
                    let s = String::from_utf8_lossy(&b).into_owned();
                    let s = match sp.prec {
                        Some(p) => s.chars().take(p).collect(),
                        None => s,
                    };
                    if sp.width.is_some() {
                        out.extend_from_slice(pad(s, &sp, false).as_bytes());
                    } else {
                        out.extend_from_slice(&b);
                    }
                    if stop {
                        return status;
                    }
                }
                'q' | 'Q' => {
                    let a = next_arg(&mut ai).unwrap_or_default();
                    let a = if conv == 'Q' {
                        match sp.prec {
                            Some(p) => a.chars().take(p).collect(),
                            None => a,
                        }
                    } else {
                        a
                    };
                    let q = if a.is_empty() { "''".to_string() } else { quote_q(&a) };
                    out.extend_from_slice(pad(q, &sp, false).as_bytes());
                }
                'c' => {
                    let a = next_arg(&mut ai).unwrap_or_default();
                    let ch: String = if a.is_empty() { "\0".to_string() } else { a.chars().take(1).collect() };
                    out.extend_from_slice(pad(ch, &sp, false).as_bytes());
                }
                'd' | 'i' | 'u' | 'o' | 'x' | 'X' => {
                    let a = next_arg(&mut ai).unwrap_or_default();
                    let v = match parse_num_arg(&a) {
                        Ok(v) => v,
                        Err(e) => {
                            let (v, t) = e.split_once('|').unwrap_or(("0", &a));
                            sh.eprint(&format!("printf: {}: invalid number", t));
                            status = 1;
                            v.parse().unwrap_or(0)
                        }
                    };
                    out.extend_from_slice(fmt_int(v, conv, &sp).as_bytes());
                }
                'f' | 'F' | 'e' | 'E' | 'g' | 'G' | 'a' | 'A' => {
                    let a = next_arg(&mut ai).unwrap_or_default();
                    let v = match parse_float_arg(&a) {
                        Ok(v) => v,
                        Err(t) => {
                            sh.eprint(&format!("printf: {}: invalid number", t));
                            status = 1;
                            0.0
                        }
                    };
                    let prec = sp.prec.unwrap_or(6);
                    let mut s = match conv {
                        'f' | 'F' => {
                            if v.is_finite() {
                                let mut s = format!("{:.*}", prec, v);
                                if sp.alt && prec == 0 {
                                    s.push('.');
                                }
                                s
                            } else {
                                fmt_special(v, conv == 'F')
                            }
                        }
                        'e' | 'E' => fmt_exp(v, prec, conv == 'E', sp.alt),
                        'g' | 'G' => fmt_g(v, prec, conv == 'G', sp.alt),
                        _ => format!("{}", v),
                    };
                    if v >= 0.0 && !s.starts_with('-') {
                        if sp.plus {
                            s.insert(0, '+');
                        } else if sp.space {
                            s.insert(0, ' ');
                        }
                    }
                    out.extend_from_slice(pad(s, &sp, v.is_finite()).as_bytes());
                }
                '(' => {
                    // %(fmt)T
                    let mut tf = String::new();
                    while i < f.len() && f[i] != ')' {
                        tf.push(f[i]);
                        i += 1;
                    }
                    i += 1; // ')'
                    if i < f.len() && f[i] == 'T' {
                        i += 1;
                    }
                    let a = next_arg(&mut ai).unwrap_or_default();
                    let t: i64 = if a.is_empty() || a == "-1" {
                        now_epoch()
                    } else if a == "-2" {
                        now_epoch() - sh.start.elapsed().as_secs() as i64
                    } else {
                        parse_num_arg(&a).unwrap_or(0)
                    };
                    let s = strftime(&tf, t);
                    out.extend_from_slice(pad(s, &sp, false).as_bytes());
                }
                other => {
                    sh.eprint(&format!("printf: `{}': invalid format character", other));
                    return 1;
                }
            }
        }
        if ai == start_ai || ai >= args.len() {
            break;
        }
    }
    status
}

fn quote_q(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "_-./:=,+@%^".contains(c)) {
        return s.to_string();
    }
    if s.chars().any(|c| c.is_control()) {
        return shell_quote(s);
    }
    let mut o = String::new();
    for c in s.chars() {
        if !(c.is_alphanumeric() || "_-./:=,+@%^".contains(c)) {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

pub fn printf(sh: &mut Shell, args: &[String]) -> R {
    let mut var: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-v" {
            var = args.get(i + 1).cloned();
            i += 2;
        } else if args[i] == "--" {
            i += 1;
            break;
        } else if let Some(v) = args[i].strip_prefix("-v") {
            if !v.is_empty() {
                var = Some(v.to_string());
                i += 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    let fmt = match args.get(i) {
        Some(f) => f.clone(),
        None => {
            sh.eprint("printf: usage: printf [-v var] format [arguments]");
            return Ok(2);
        }
    };
    let mut out = Vec::new();
    let st = printf_format(sh, &fmt, &args[i + 1..], &mut out);
    match var {
        Some(v) => {
            let s = String::from_utf8_lossy(&out).into_owned();
            let r = if let (Some(b), true) = (v.find('['), v.ends_with(']')) {
                let n = v[..b].to_string();
                let k = v[b + 1..v.len() - 1].to_string();
                sh.set_elem(&n, &k, &s, false)
            } else {
                sh.set_var(&v, &s, false)
            };
            if let Err(e) = r {
                sh.eprint(&format!("printf: {}", e));
                return Ok(1);
            }
        }
        None => sh.out(1, &out)?,
    }
    Ok(st)
}

// ---------------- time ----------------

#[repr(C)]
#[derive(Default)]
struct Tm {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    tm_gmtoff: i64,
    tm_zone: *const core::ffi::c_char,
}

extern "C" {
    fn localtime_r(t: *const i64, tm: *mut Tm) -> *mut Tm;
}

pub fn now_epoch() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Local offset from UTC in seconds, and zone name.
pub fn tz_offset() -> (i64, String) {
    let now = now_epoch();
    let mut tm = Tm::default();
    if unsafe { localtime_r(&now, &mut tm) }.is_null() {
        return (0, "UTC".into());
    }
    let name = if tm.tm_zone.is_null() {
        String::new()
    } else {
        unsafe { std::ffi::CStr::from_ptr(tm.tm_zone) }.to_string_lossy().into_owned()
    };
    (tm.tm_gmtoff, if name.is_empty() { "UTC".into() } else { name })
}

fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn strftime(fmt: &str, epoch: i64) -> String {
    let (off, zname) = tz_offset();
    let t = epoch + off;
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    let (y, mo, d) = civil(days);
    let (h, mi, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let wday = ((days % 7 + 11) % 7) as usize; // 1970-01-01 was Thursday (4)
    let wday = (wday + 0) % 7;
    let jan1 = {
        // day of year
        let mut dd = days;
        let mut k = 0;
        loop {
            let (yy, _, _) = civil(dd - 1);
            if yy != y {
                break;
            }
            dd -= 1;
            k += 1;
            if k > 366 {
                break;
            }
        }
        k + 1
    };
    const WD: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const MN: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let mut out = String::new();
    let mut it = fmt.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('Y') => out.push_str(&y.to_string()),
            Some('y') => out.push_str(&format!("{:02}", y % 100)),
            Some('C') => out.push_str(&format!("{:02}", y / 100)),
            Some('m') => out.push_str(&format!("{:02}", mo)),
            Some('d') => out.push_str(&format!("{:02}", d)),
            Some('e') => out.push_str(&format!("{:2}", d)),
            Some('H') => out.push_str(&format!("{:02}", h)),
            Some('I') => out.push_str(&format!("{:02}", if h % 12 == 0 { 12 } else { h % 12 })),
            Some('l') => out.push_str(&format!("{:2}", if h % 12 == 0 { 12 } else { h % 12 })),
            Some('k') => out.push_str(&format!("{:2}", h)),
            Some('M') => out.push_str(&format!("{:02}", mi)),
            Some('S') => out.push_str(&format!("{:02}", s)),
            Some('p') => out.push_str(if h < 12 { "AM" } else { "PM" }),
            Some('P') => out.push_str(if h < 12 { "am" } else { "pm" }),
            Some('a') => out.push_str(&WD[wday][..3]),
            Some('A') => out.push_str(WD[wday]),
            Some('b') | Some('h') => out.push_str(&MN[(mo - 1) as usize][..3]),
            Some('B') => out.push_str(MN[(mo - 1) as usize]),
            Some('j') => out.push_str(&format!("{:03}", jan1)),
            Some('u') => out.push_str(&(if wday == 0 { 7 } else { wday }).to_string()),
            Some('w') => out.push_str(&wday.to_string()),
            Some('s') => out.push_str(&epoch.to_string()),
            Some('F') => out.push_str(&format!("{}-{:02}-{:02}", y, mo, d)),
            Some('T') => out.push_str(&format!("{:02}:{:02}:{:02}", h, mi, s)),
            Some('R') => out.push_str(&format!("{:02}:{:02}", h, mi)),
            Some('D') => out.push_str(&format!("{:02}/{:02}/{:02}", mo, d, y % 100)),
            Some('c') => out.push_str(&format!("{} {} {:2} {:02}:{:02}:{:02} {}", &WD[wday][..3], &MN[(mo - 1) as usize][..3], d, h, mi, s, y)),
            Some('x') => out.push_str(&format!("{:02}/{:02}/{:02}", mo, d, y % 100)),
            Some('X') => out.push_str(&format!("{:02}:{:02}:{:02}", h, mi, s)),
            Some('z') => {
                let o = off / 60;
                out.push_str(&format!("{}{:02}{:02}", if o < 0 { '-' } else { '+' }, o.abs() / 60, o.abs() % 60));
            }
            Some('Z') => out.push_str(&zname),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('%') => out.push('%'),
            Some(o) => {
                out.push('%');
                out.push(o);
            }
            None => out.push('%'),
        }
    }
    out
}

// ---------------- read ----------------

/// Split a line for `read`: (char, escaped) pairs.
fn read_split(line: &[(char, bool)], ifs: &str, nvars: usize) -> Vec<String> {
    let is_ws = |c: char| ifs.contains(c) && matches!(c, ' ' | '\t' | '\n');
    let is_ifs = |c: char| ifs.contains(c);
    let mut i = 0;
    let n = line.len();
    let mut out = Vec::new();
    while i < n && !line[i].1 && is_ws(line[i].0) {
        i += 1;
    }
    while out.len() + 1 < nvars && i < n {
        let mut f = String::new();
        while i < n && !(!line[i].1 && is_ifs(line[i].0)) {
            f.push(line[i].0);
            i += 1;
        }
        out.push(f);
        if i < n {
            // consume delimiter: ws* [non-ws ifs] ws*
            let mut saw_nonws = false;
            while i < n && !line[i].1 && is_ifs(line[i].0) {
                if is_ws(line[i].0) {
                    i += 1;
                } else if !saw_nonws {
                    saw_nonws = true;
                    i += 1;
                } else {
                    break;
                }
            }
        }
    }
    if out.len() < nvars {
        // rest of line, trailing IFS whitespace removed
        let mut end = n;
        while end > i && !line[end - 1].1 && is_ws(line[end - 1].0) {
            end -= 1;
        }
        if i < n || out.len() + 1 <= nvars {
            let sub = read_split_all(&line[i..end], ifs);
            let rest: String = if sub.len() == 1 { sub[0].clone() } else { line[i..end].iter().map(|p| p.0).collect() };
            if !(rest.is_empty() && i >= n && out.len() + 1 > nvars) {
                out.push(rest);
            }
        }
    }
    out
}

fn read_split_all(line: &[(char, bool)], ifs: &str) -> Vec<String> {
    let is_ws = |c: char| ifs.contains(c) && matches!(c, ' ' | '\t' | '\n');
    let is_ifs = |c: char| ifs.contains(c);
    let mut out = Vec::new();
    let n = line.len();
    let mut i = 0;
    while i < n && !line[i].1 && is_ws(line[i].0) {
        i += 1;
    }
    while i < n {
        let mut f = String::new();
        while i < n && !(!line[i].1 && is_ifs(line[i].0)) {
            f.push(line[i].0);
            i += 1;
        }
        out.push(f);
        let mut saw_nonws = false;
        while i < n && !line[i].1 && is_ifs(line[i].0) {
            if is_ws(line[i].0) {
                i += 1;
            } else if !saw_nonws {
                saw_nonws = true;
                i += 1;
            } else {
                break;
            }
        }
    }
    out
}

pub fn read(sh: &mut Shell, args: &[String]) -> R {
    let mut raw = false;
    let mut silent = false;
    let mut array: Option<String> = None;
    let mut delim = b'\n';
    let mut nchars: Option<usize> = None;
    let mut exact = false;
    let mut prompt: Option<String> = None;
    let mut timeout: Option<f64> = None;
    let mut fd = 0;
    let mut names: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            names.extend(args[i + 1..].iter().cloned());
            break;
        }
        if a.starts_with('-') && a.len() > 1 && names.is_empty() {
            let cs: Vec<char> = a[1..].chars().collect();
            let mut k = 0;
            while k < cs.len() {
                let c = cs[k];
                let take_val = |k: usize, i: &mut usize| -> Option<String> {
                    let rest: String = cs[k + 1..].iter().collect();
                    if !rest.is_empty() {
                        Some(rest)
                    } else {
                        *i += 1;
                        args.get(*i).cloned()
                    }
                };
                match c {
                    'r' => raw = true,
                    's' => silent = true,
                    'e' | 'E' => {}
                    'a' => {
                        array = take_val(k, &mut i);
                        break;
                    }
                    'd' => {
                        let v = take_val(k, &mut i).unwrap_or_default();
                        delim = v.bytes().next().unwrap_or(0);
                        break;
                    }
                    'n' | 'N' => {
                        exact = c == 'N';
                        nchars = take_val(k, &mut i).and_then(|v| v.parse().ok());
                        break;
                    }
                    'p' => {
                        prompt = take_val(k, &mut i);
                        break;
                    }
                    't' => {
                        timeout = take_val(k, &mut i).and_then(|v| v.parse().ok());
                        break;
                    }
                    'u' => {
                        fd = take_val(k, &mut i).and_then(|v| v.parse().ok()).unwrap_or(0);
                        break;
                    }
                    'i' => {
                        let _ = take_val(k, &mut i);
                        break;
                    }
                    _ => {
                        sh.eprint(&format!("read: -{}: invalid option", c));
                        return Ok(2);
                    }
                }
                k += 1;
            }
        } else {
            names.push(a.clone());
        }
        i += 1;
    }
    let h = match sh.fds.get(fd) {
        Some(h) => h.clone(),
        None => {
            sh.eprint(&format!("read: {}: invalid file descriptor: Bad file descriptor", fd));
            return Ok(1);
        }
    };
    let is_console = h.is_console();
    if let Some(p) = &prompt {
        if is_console {
            sh.write_fd(2, p.as_bytes());
        }
    }
    if let Some(t) = timeout {
        if is_console {
            if !crate::sys::wait_stdin((t * 1000.0) as u32) {
                return Ok(142);
            }
        } else if t == 0.0 {
            return Ok(0);
        }
    }
    let _echo_guard = if silent && is_console { Some(EchoOff::new()) } else { None };
    // read data
    let mut chars: Vec<(char, bool)> = Vec::new();
    let mut got_any = false;
    let mut eof = false;
    loop {
        let limit = nchars.map(|n| n.saturating_sub(chars.len()));
        if limit == Some(0) {
            break;
        }
        let (bytes, found) = if exact {
            // -N ignores the delimiter: 0xFF never occurs in UTF-8 text
            match h.read_until(0xFF, limit) {
                Ok(r) => r,
                Err(_) => (Vec::new(), false),
            }
        } else {
            match h.read_until(delim, limit) {
                Ok(r) => r,
                Err(_) => (Vec::new(), false),
            }
        };
        if !bytes.is_empty() || found {
            got_any = true;
        }
        let mut s = String::from_utf8_lossy(&bytes).into_owned();
        if is_console && delim == b'\n' && s.ends_with('\r') {
            s.pop();
        }
        if !found && (limit.is_none() || bytes.len() < limit.unwrap_or(0)) {
            eof = true;
        }
        let mut it = s.chars().peekable();
        let mut continued = false;
        while let Some(c) = it.next() {
            if !raw && c == '\\' {
                match it.next() {
                    Some(n) => chars.push((n, true)),
                    None => {
                        // backslash-newline continuation
                        if found && delim == b'\n' {
                            continued = true;
                        }
                    }
                }
            } else {
                chars.push((c, false));
            }
        }
        if continued && !eof {
            continue;
        }
        break;
    }
    drop(_echo_guard);
    if silent && is_console {
        sh.write_fd(2, b"\n");
    }
    let ifs = sh.ifs();
    let result_status = if eof && (!got_any || nchars.is_none()) { 1 } else { 0 };
    if let Some(arr) = array {
        let fields = read_split_all(&chars, &ifs);
        let elems = fields.into_iter().map(|f| (None, f)).collect();
        let _ = sh.unset_var(&arr);
        if let Err(e) = sh.set_array(&arr, elems, false) {
            sh.eprint(&format!("read: {}", e));
            return Ok(1);
        }
        return Ok(if got_any { result_status.min(if eof { 1 } else { 0 }) } else { 1 });
    }
    if names.is_empty() {
        let line: String = chars.iter().map(|p| p.0).collect();
        if let Err(e) = sh.set_var("REPLY", &line, false) {
            sh.eprint(&format!("read: {}", e));
            return Ok(1);
        }
    } else {
        let fields = read_split(&chars, &ifs, names.len());
        for (k, n) in names.iter().enumerate() {
            let v = fields.get(k).cloned().unwrap_or_default();
            if !crate::parser::is_valid_name(n.split('[').next().unwrap_or("")) {
                sh.eprint(&format!("read: `{}': not a valid identifier", n));
                return Ok(1);
            }
            let r = if let (Some(b), true) = (n.find('['), n.ends_with(']')) {
                sh.set_elem(&n[..b], &n[b + 1..n.len() - 1], &v, false)
            } else {
                sh.set_var(n, &v, false)
            };
            if let Err(e) = r {
                sh.eprint(&format!("read: {}", e));
                return Ok(1);
            }
        }
    }
    Ok(result_status)
}

struct EchoOff {
    saved: Option<crate::sys::Termios>,
}
impl EchoOff {
    fn new() -> EchoOff {
        let saved = crate::sys::get_termios(0);
        if let Some(mut t) = saved {
            crate::sys::no_echo_termios(&mut t);
            crate::sys::set_termios(0, &t);
        }
        EchoOff { saved }
    }
}
impl Drop for EchoOff {
    fn drop(&mut self) {
        if let Some(t) = &self.saved {
            crate::sys::set_termios(0, t);
        }
    }
}

pub fn mapfile(sh: &mut Shell, name: &str, args: &[String]) -> R {
    let mut delim = b'\n';
    let mut count: usize = 0;
    let mut origin: i64 = 0;
    let mut skip: usize = 0;
    let mut strip = false;
    let mut fd = 0;
    let mut arr = "MAPFILE".to_string();
    let mut i = 0;
    let mut callback: Option<String> = None;
    let mut quantum: usize = 5000;
    while i < args.len() {
        let a = &args[i];
        let mut val = || {
            i += 1;
            args.get(i).cloned().unwrap_or_default()
        };
        match a.as_str() {
            "-d" => delim = val().bytes().next().unwrap_or(0),
            "-n" => count = val().parse().unwrap_or(0),
            "-O" => origin = val().parse().unwrap_or(0),
            "-s" => skip = val().parse().unwrap_or(0),
            "-u" => fd = val().parse().unwrap_or(0),
            "-C" => callback = Some(val()),
            "-c" => quantum = val().parse().unwrap_or(5000),
            "-t" => strip = true,
            "--" => {}
            s if s.starts_with('-') && s.len() > 1 => {
                sh.eprint(&format!("{}: {}: invalid option", name, s));
                return Ok(2);
            }
            s => arr = s.to_string(),
        }
        i += 1;
    }
    let h = match sh.fds.get(fd) {
        Some(h) => h.clone(),
        None => {
            sh.eprint(&format!("{}: {}: invalid file descriptor", name, fd));
            return Ok(1);
        }
    };
    if origin == 0 {
        let _ = sh.unset_var(&arr);
        let v = sh.vars.get_mut_or_create(&arr);
        v.value = Some(Value::Arr(BTreeMap::new()));
        v.attrs |= ARRAY;
    }
    let mut n = 0usize;
    let mut idx = origin;
    let mut skipped = 0;
    loop {
        if count > 0 && n >= count {
            break;
        }
        let (mut bytes, found) = match h.read_until(delim, None) {
            Ok(r) => r,
            Err(_) => break,
        };
        if !found && bytes.is_empty() {
            break;
        }
        if skipped < skip {
            skipped += 1;
            continue;
        }
        if !strip && found {
            bytes.push(delim);
        } else if strip && delim == b'\n' && bytes.last() == Some(&b'\r') && false {
            bytes.pop();
        }
        let s = String::from_utf8_lossy(&bytes).into_owned();
        if let Err(e) = sh.set_elem(&arr, &idx.to_string(), &s, false) {
            sh.eprint(&format!("{}: {}", name, e));
            return Ok(1);
        }
        if let Some(cb) = &callback {
            if quantum > 0 && (n + 1) % quantum == 0 {
                let src = format!("{} {} {}", cb, idx, shell_quote(&s));
                sh.run_source(&src, false)?;
            }
        }
        idx += 1;
        n += 1;
        if !found {
            break;
        }
    }
    Ok(0)
}

// ---------------- test ----------------

/// The file a test operator looks at. As with Cygwin, `foo` also names `foo.exe` when only that
/// exists, so `[ -x /usr/bin/ls ]` and `[ -x "$(command -v git)" ]` find the programs.
fn test_path(sh: &Shell, p: &str) -> std::path::PathBuf {
    let w = path::to_win(p, &sh.cwd);
    if !w.exists() && !p.ends_with('/') && !p.ends_with('\\') {
        let mut e = w.clone().into_os_string();
        e.push(".exe");
        let e = std::path::PathBuf::from(e);
        if e.is_file() {
            return e;
        }
    }
    w
}

fn file_meta(sh: &Shell, p: &str) -> Option<std::fs::Metadata> {
    if p.is_empty() {
        return None;
    }
    let md = std::fs::metadata(test_path(sh, p)).ok()?;
    if (p.ends_with('/') || p.ends_with('\\')) && !md.is_dir() {
        return None;
    }
    Some(md)
}

fn is_executable(p: &std::path::Path, md: &std::fs::Metadata) -> bool {
    if md.is_dir() {
        return true;
    }
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if matches!(ext.as_str(), "exe" | "com" | "bat" | "cmd" | "sh" | "dll") {
        return true;
    }
    use std::io::Read;
    if let Ok(mut f) = std::fs::File::open(p) {
        let mut b = [0u8; 2];
        if f.read(&mut b).unwrap_or(0) == 2 {
            return &b == b"#!" || &b == b"MZ";
        }
    }
    false
}

pub fn test_unary(sh: &mut Shell, op: &str, s: &str) -> bool {
    let dev = |s: &str| matches!(s, "/dev/null" | "/dev/stdin" | "/dev/stdout" | "/dev/stderr" | "/dev/tty");
    match op {
        "-z" => s.is_empty(),
        "-n" => !s.is_empty(),
        "-e" | "-a" => dev(s) || file_meta(sh, s).is_some(),
        "-f" => file_meta(sh, s).map(|m| m.is_file()).unwrap_or(false),
        "-d" => file_meta(sh, s).map(|m| m.is_dir()).unwrap_or(false),
        "-s" => file_meta(sh, s).map(|m| m.len() > 0 || m.is_dir()).unwrap_or(false),
        "-r" => dev(s) || file_meta(sh, s).is_some(),
        "-w" => dev(s) || file_meta(sh, s).map(|m| !m.permissions().readonly() || m.is_dir()).unwrap_or(false),
        "-x" => {
            if s.is_empty() {
                return false;
            }
            let p = test_path(sh, s);
            match std::fs::metadata(&p) {
                Ok(m) => is_executable(&p, &m),
                Err(_) => false,
            }
        }
        "-L" | "-h" => {
            !s.is_empty() && std::fs::symlink_metadata(path::to_win(s, &sh.cwd)).map(|m| m.file_type().is_symlink()).unwrap_or(false)
        }
        "-c" => dev(s),
        "-b" | "-p" | "-S" | "-g" | "-u" | "-k" => false,
        "-O" | "-G" => file_meta(sh, s).is_some(),
        "-N" => file_meta(sh, s)
            .map(|m| match (m.modified(), m.accessed()) {
                (Ok(a), Ok(b)) => a > b,
                _ => false,
            })
            .unwrap_or(false),
        "-t" => {
            let fd: i32 = s.parse().unwrap_or(-1);
            sh.fds.get(fd).map(|h| h.is_console()).unwrap_or(false)
        }
        "-v" => {
            if let (Some(b), true) = (s.find('['), s.ends_with(']')) {
                let n = &s[..b];
                let k = &s[b + 1..s.len() - 1];
                match sh.get_value(n) {
                    Some(Value::Arr(a)) => {
                        if k == "@" || k == "*" {
                            return !a.is_empty();
                        }
                        let i = crate::arith::eval(k, sh).unwrap_or(-1);
                        let i = if i < 0 { i + a.keys().next_back().map(|x| x + 1).unwrap_or(0) } else { i };
                        a.contains_key(&i)
                    }
                    Some(Value::Assoc(m)) => {
                        if k == "@" || k == "*" {
                            return !m.is_empty();
                        }
                        m.get(k).is_some()
                    }
                    Some(Value::Str(_)) => k == "0" || k == "@" || k == "*",
                    None => false,
                }
            } else {
                match sh.get_value(s) {
                    Some(Value::Arr(a)) => a.contains_key(&0),
                    Some(Value::Assoc(m)) => m.get("0").is_some(),
                    Some(_) => true,
                    None => s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty() && s.parse::<usize>().map(|n| n >= 1 && n <= sh.positional.len()).unwrap_or(false),
                }
            }
        }
        "-R" => sh.var_attrs(s) & NAMEREF != 0,
        "-o" => set_opt_index(s).map(|i| sh.o[i]).unwrap_or(false),
        _ => false,
    }
}

fn mtime(sh: &Shell, p: &str) -> Option<std::time::SystemTime> {
    file_meta(sh, p).and_then(|m| m.modified().ok())
}

pub fn test_binary(sh: &mut Shell, a: &str, op: &str, b: &str, arith: bool) -> Result<bool, String> {
    let int = |sh: &mut Shell, s: &str| -> Result<i64, String> {
        if arith {
            crate::arith::eval(s, sh)
        } else {
            let t = s.trim();
            t.parse::<i64>().map_err(|_| format!("{}: integer expression expected", s))
        }
    };
    Ok(match op {
        "=" | "==" => a == b,
        "!=" => a != b,
        "<" => a < b,
        ">" => a > b,
        "-eq" => int(sh, a)? == int(sh, b)?,
        "-ne" => int(sh, a)? != int(sh, b)?,
        "-lt" => int(sh, a)? < int(sh, b)?,
        "-le" => int(sh, a)? <= int(sh, b)?,
        "-gt" => int(sh, a)? > int(sh, b)?,
        "-ge" => int(sh, a)? >= int(sh, b)?,
        "-nt" => match (mtime(sh, a), mtime(sh, b)) {
            (Some(x), Some(y)) => x > y,
            (Some(_), None) => true,
            _ => false,
        },
        "-ot" => match (mtime(sh, a), mtime(sh, b)) {
            (Some(x), Some(y)) => x < y,
            (None, Some(_)) => true,
            _ => false,
        },
        "-ef" => {
            let pa = std::fs::canonicalize(path::to_win(a, &sh.cwd));
            let pb = std::fs::canonicalize(path::to_win(b, &sh.cwd));
            match (pa, pb) {
                (Ok(x), Ok(y)) => x.to_string_lossy().to_lowercase() == y.to_string_lossy().to_lowercase(),
                _ => false,
            }
        }
        _ => return Err(format!("{}: binary operator expected", op)),
    })
}

const UNARY_OPS: &[&str] = &[
    "-a", "-b", "-c", "-d", "-e", "-f", "-g", "-h", "-k", "-p", "-r", "-s", "-t", "-u", "-w", "-x", "-G", "-L", "-N", "-O",
    "-S", "-z", "-n", "-o", "-v", "-R",
];
const BINARY_OPS: &[&str] = &["=", "==", "!=", "<", ">", "-eq", "-ne", "-lt", "-le", "-gt", "-ge", "-nt", "-ot", "-ef"];

struct TestP<'a> {
    a: &'a [String],
    i: usize,
}

impl<'a> TestP<'a> {
    fn or(&mut self, sh: &mut Shell) -> Result<bool, String> {
        let mut v = self.and(sh)?;
        while self.i < self.a.len() && self.a[self.i] == "-o" {
            self.i += 1;
            let r = self.and(sh)?;
            v = v || r;
        }
        Ok(v)
    }
    fn and(&mut self, sh: &mut Shell) -> Result<bool, String> {
        let mut v = self.not(sh)?;
        while self.i < self.a.len() && self.a[self.i] == "-a" {
            self.i += 1;
            let r = self.not(sh)?;
            v = v && r;
        }
        Ok(v)
    }
    fn not(&mut self, sh: &mut Shell) -> Result<bool, String> {
        if self.i < self.a.len() && self.a[self.i] == "!" {
            self.i += 1;
            return Ok(!self.not(sh)?);
        }
        self.prim(sh)
    }
    fn prim(&mut self, sh: &mut Shell) -> Result<bool, String> {
        let a = self.a;
        if self.i >= a.len() {
            return Err("argument expected".into());
        }
        if a[self.i] == "(" {
            self.i += 1;
            let v = self.or(sh)?;
            if self.i >= a.len() || a[self.i] != ")" {
                return Err("`)' expected".into());
            }
            self.i += 1;
            return Ok(v);
        }
        if self.i + 2 < a.len() + 0 || self.i + 2 <= a.len() - 1 {
            if BINARY_OPS.contains(&a[self.i + 1].as_str()) {
                let r = test_binary(sh, &a[self.i], &a[self.i + 1], &a[self.i + 2], false)?;
                self.i += 3;
                return Ok(r);
            }
        }
        if UNARY_OPS.contains(&a[self.i].as_str()) && self.i + 1 < a.len() {
            let r = test_unary(sh, &a[self.i], &a[self.i + 1]);
            self.i += 2;
            return Ok(r);
        }
        let v = !a[self.i].is_empty();
        self.i += 1;
        Ok(v)
    }
}

fn test_eval(sh: &mut Shell, a: &[String]) -> Result<bool, String> {
    match a.len() {
        0 => Ok(false),
        1 => Ok(!a[0].is_empty()),
        2 => {
            if a[0] == "!" {
                Ok(a[1].is_empty())
            } else if UNARY_OPS.contains(&a[0].as_str()) {
                Ok(test_unary(sh, &a[0], &a[1]))
            } else {
                Err(format!("{}: unary operator expected", a[0]))
            }
        }
        3 => {
            if BINARY_OPS.contains(&a[1].as_str()) {
                test_binary(sh, &a[0], &a[1], &a[2], false)
            } else if a[1] == "-a" {
                Ok(!a[0].is_empty() && !a[2].is_empty())
            } else if a[1] == "-o" {
                Ok(!a[0].is_empty() || !a[2].is_empty())
            } else if a[0] == "!" {
                Ok(!test_eval(sh, &a[1..])?)
            } else if a[0] == "(" && a[2] == ")" {
                Ok(!a[1].is_empty())
            } else {
                Err(format!("{}: binary operator expected", a[1]))
            }
        }
        4 => {
            if a[0] == "!" {
                Ok(!test_eval(sh, &a[1..])?)
            } else if a[0] == "(" && a[3] == ")" {
                test_eval(sh, &a[1..3])
            } else {
                let mut p = TestP { a, i: 0 };
                let v = p.or(sh)?;
                if p.i < a.len() {
                    return Err("too many arguments".into());
                }
                Ok(v)
            }
        }
        _ => {
            let mut p = TestP { a, i: 0 };
            let v = p.or(sh)?;
            if p.i < a.len() {
                return Err("too many arguments".into());
            }
            Ok(v)
        }
    }
}

pub fn test(sh: &mut Shell, name: &str, args: &[String]) -> R {
    let a: &[String] = if name == "[" {
        match args.last() {
            Some(l) if l == "]" => &args[..args.len() - 1],
            _ => {
                sh.eprint("[: missing `]'");
                return Ok(2);
            }
        }
    } else {
        args
    };
    match test_eval(sh, a) {
        Ok(true) => Ok(0),
        Ok(false) => Ok(1),
        Err(e) => {
            sh.eprint(&format!("{}: {}", name, e));
            Ok(2)
        }
    }
}

// ---------------- declare & friends ----------------

pub fn decl_quote(s: &str) -> String {
    if s.chars().any(|c| c.is_control()) {
        return shell_quote(s);
    }
    let mut o = String::from("\"");
    for c in s.chars() {
        if matches!(c, '"' | '\\' | '$' | '`') {
            o.push('\\');
        }
        o.push(c);
    }
    o.push('"');
    o
}

pub fn decl_key(k: &str) -> String {
    if k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !k.is_empty() {
        k.to_string()
    } else {
        decl_quote(k)
    }
}

fn attr_flags(attrs: u32, v: Option<&Value>) -> String {
    let mut s = String::new();
    if matches!(v, Some(Value::Arr(_))) || attrs & ARRAY != 0 && !matches!(v, Some(Value::Assoc(_))) {
        s.push('a');
    }
    if matches!(v, Some(Value::Assoc(_))) || attrs & ASSOC != 0 {
        s.push('A');
    }
    if attrs & INTEGER != 0 {
        s.push('i');
    }
    if attrs & LOWER != 0 {
        s.push('l');
    }
    if attrs & NAMEREF != 0 {
        s.push('n');
    }
    if attrs & READONLY != 0 {
        s.push('r');
    }
    if attrs & TRACE != 0 {
        s.push('t');
    }
    if attrs & UPPER != 0 {
        s.push('u');
    }
    if attrs & EXPORT != 0 {
        s.push('x');
    }
    if s.is_empty() {
        "--".into()
    } else {
        format!("-{}", s)
    }
}

pub fn declare_line(sh: &mut Shell, name: &str) -> Option<String> {
    let var = sh.vars.get(name).cloned();
    let (attrs, value) = match var {
        Some(v) => (v.attrs, v.value),
        None => {
            if Shell::is_special_dynamic(name) {
                (0, sh.get_value(name))
            } else {
                return None;
            }
        }
    };
    let flags = attr_flags(attrs, value.as_ref());
    Some(match value {
        None => format!("declare {} {}", flags, name),
        Some(Value::Str(s)) => format!("declare {} {}={}", flags, name, decl_quote(&s)),
        Some(Value::Arr(a)) => {
            let items: Vec<String> = a.iter().map(|(k, v)| format!("[{}]={}", k, decl_quote(v))).collect();
            format!("declare {} {}=({})", flags, name, items.join(" "))
        }
        Some(Value::Assoc(m)) => {
            let items: Vec<String> = m.iter().map(|(k, v)| format!("[{}]={} ", decl_key(k), decl_quote(v))).collect();
            format!("declare {} {}=({})", flags, name, items.concat())
        }
    })
}

fn parse_compound_value(sh: &mut Shell, s: &str) -> Option<Vec<(Option<String>, String)>> {
    let t = s.trim();
    if !(t.starts_with('(') && t.ends_with(')')) {
        return None;
    }
    let src = format!("__gash_tmp={}", t);
    let mut p = crate::parser::Parser::new(&src);
    let prog = p.parse_program().ok()?;
    let item = prog.items.first()?;
    if let crate::ast::Command::Simple(sc) = &item.and_or.first.cmds.first()?.0 {
        if let Some(a) = sc.assigns.first() {
            if let crate::ast::AssignValue::Array(elems) = &a.value {
                return sh.expand_array_elems(elems).ok();
            }
        }
    }
    None
}

pub fn declare(sh: &mut Shell, cmd: &str, args: &[String]) -> R {
    let mut set_attrs: u32 = 0;
    let mut unset_attrs: u32 = 0;
    let mut print = false;
    let mut funcs = false;
    let mut func_names = false;
    let mut global = false;
    let mut names: Vec<(usize, String)> = Vec::new();
    let mut opts_done = false;
    for (k, a) in args.iter().enumerate() {
        if !opts_done && (a.starts_with('-') || a.starts_with('+')) && a.len() > 1 && !a.contains('=') {
            if a == "--" {
                opts_done = true;
                continue;
            }
            let on = a.starts_with('-');
            for c in a[1..].chars() {
                let bit = match c {
                    'a' => ARRAY,
                    'A' => ASSOC,
                    'i' => INTEGER,
                    'l' => LOWER,
                    'u' => UPPER,
                    'n' => NAMEREF,
                    'r' => READONLY,
                    't' => TRACE,
                    'x' => EXPORT,
                    'c' => CAPITALIZE,
                    'p' => {
                        print = true;
                        0
                    }
                    'f' => {
                        funcs = true;
                        0
                    }
                    'F' => {
                        func_names = true;
                        0
                    }
                    'g' => {
                        global = true;
                        0
                    }
                    'I' => 0,
                    _ => {
                        sh.eprint(&format!("{}: -{}: invalid option", cmd, c));
                        return Ok(2);
                    }
                };
                if on {
                    set_attrs |= bit;
                } else {
                    unset_attrs |= bit;
                }
            }
        } else {
            opts_done = true;
            names.push((k + 1, a.clone()));
        }
    }
    let is_local = cmd == "local" || (sh.func_depth > 0 && !global && cmd != "export" && cmd != "readonly");
    if cmd == "local" && sh.func_depth == 0 {
        sh.eprint("local: can only be used in a function");
        return Ok(1);
    }
    // function listing
    if funcs || func_names {
        let mut out = String::new();
        let mut list: Vec<String> = if names.is_empty() { sh.funcs.keys().cloned().collect() } else { names.iter().map(|(_, n)| n.clone()).collect() };
        list.sort();
        let mut st = 0;
        for n in list {
            match sh.funcs.get(&n).cloned() {
                Some(f) => {
                    if func_names && !names.is_empty() {
                        out.push_str(&format!("{}\n", n));
                    } else if func_names {
                        out.push_str(&format!("declare -f {}\n", n));
                    } else {
                        out.push_str(&format!("{}\n", format_function(&f)));
                    }
                    if set_attrs & EXPORT != 0 {
                        Arc::make_mut(&mut sh.exported_funcs).insert(n.clone());
                    }
                }
                None => st = 1,
            }
        }
        if set_attrs & EXPORT == 0 || names.is_empty() {
            sh.out(1, out.as_bytes())?;
        }
        return Ok(st);
    }
    if names.is_empty() {
        let mut out = String::new();
        for (n, v) in sh.vars.visible() {
            if set_attrs != 0 && v.attrs & set_attrs == 0 && !(set_attrs & ARRAY != 0 && matches!(v.value, Some(Value::Arr(_))) || set_attrs & ASSOC != 0 && matches!(v.value, Some(Value::Assoc(_)))) {
                continue;
            }
            if print || set_attrs != 0 {
                if let Some(l) = declare_line(sh, &n) {
                    out.push_str(&l);
                    out.push('\n');
                }
            } else if let Some(Value::Str(s)) = &v.value {
                out.push_str(&format!("{}={}\n", n, shell_quote(s)));
            } else if v.value.is_some() {
                if let Some(l) = declare_line(sh, &n) {
                    let l = l.splitn(3, ' ').nth(2).unwrap_or("").to_string();
                    out.push_str(&l);
                    out.push('\n');
                }
            }
        }
        if !print && set_attrs == 0 {
            let mut fl: Vec<_> = sh.funcs.values().cloned().collect();
            fl.sort_by(|a, b| a.name.cmp(&b.name));
            for f in fl {
                out.push_str(&format!("{}\n", format_function(&f)));
            }
        }
        sh.out(1, out.as_bytes())?;
        return Ok(0);
    }
    if print {
        let mut out = String::new();
        let mut st = 0;
        for (_, n) in &names {
            match declare_line(sh, n) {
                Some(l) => {
                    out.push_str(&l);
                    out.push('\n');
                }
                None => {
                    sh.eprint(&format!("{}: {}: not found", cmd, n));
                    st = 1;
                }
            }
        }
        sh.out(1, out.as_bytes())?;
        return Ok(st);
    }
    let mut st = 0;
    for (argi, spec) in names {
        let (lhs, value, append) = match spec.find('=') {
            Some(i) => {
                let (l, v) = spec.split_at(i);
                let v = &v[1..];
                if let Some(l2) = l.strip_suffix('+') {
                    (l2.to_string(), Some(v.to_string()), true)
                } else {
                    (l.to_string(), Some(v.to_string()), false)
                }
            }
            None => (spec.clone(), None, false),
        };
        let (name, sub) = match (lhs.find('['), lhs.ends_with(']')) {
            (Some(b), true) => (lhs[..b].to_string(), Some(lhs[b + 1..lhs.len() - 1].to_string())),
            _ => (lhs.clone(), None),
        };
        if !crate::parser::is_valid_name(&name) {
            sh.eprint(&format!("{}: `{}': not a valid identifier", cmd, spec));
            st = 1;
            continue;
        }
        let existing = sh.vars.get(&name).map(|v| v.attrs).unwrap_or(0);
        if is_local && existing & READONLY != 0 && !sh.vars.is_local_here(&name) {
            sh.eprint(&format!("{}: {}: readonly variable", cmd, name));
            st = 1;
            continue;
        }
        if existing & READONLY != 0 && (value.is_some() || unset_attrs & READONLY != 0) && !(is_local && !sh.vars.is_local_here(&name)) {
            sh.eprint(&format!("{}: {}: readonly variable", cmd, name));
            st = 1;
            continue;
        }
        // create the variable in the right scope
        if is_local && !sh.vars.is_local_here(&name) {
            let inherit = sh.vars.get(&name).cloned();
            let v = sh.vars.local_mut(&name);
            v.value = None;
            v.attrs = 0;
            if let Some(iv) = inherit {
                // local inherits nothing except when localvar_inherit; but keep export attr
                if iv.attrs & EXPORT != 0 {
                    v.attrs |= 0;
                }
            }
        } else if !is_local && global && sh.vars.get(&name).is_none() {
            sh.vars.global_mut(&name);
        } else if sh.vars.get(&name).is_none() {
            sh.vars.get_mut_or_create(&name);
        }
        // attributes first (except readonly, applied after assignment)
        {
            let v = if is_local { sh.vars.local_mut(&name) } else { sh.vars.get_mut_or_create(&name) };
            v.attrs |= set_attrs & !READONLY;
            v.attrs &= !unset_attrs;
            if set_attrs & UPPER != 0 {
                v.attrs &= !LOWER;
            }
            if set_attrs & LOWER != 0 {
                v.attrs &= !UPPER;
            }
            if set_attrs & ASSOC != 0 && !matches!(v.value, Some(Value::Assoc(_))) {
                if value.is_none() {
                    v.value = Some(Value::Assoc(AssocMap::default()));
                }
            } else if set_attrs & ARRAY != 0 && v.value.is_none() && value.is_none() {
                v.value = Some(Value::Arr(BTreeMap::new()));
            } else if set_attrs & ARRAY != 0 {
                if let Some(Value::Str(s)) = &v.value {
                    let mut m = BTreeMap::new();
                    m.insert(0, s.clone());
                    v.value = Some(Value::Arr(m));
                }
            }
        }
        if let Some(val) = value {
            let r = if let Some(elems) = sh.decl_arrays.get(&argi).cloned() {
                if sub.is_some() {
                    Err(format!("{}: cannot assign list to array member", lhs))
                } else {
                    sh.set_array(&name, elems, append)
                }
            } else if let Some(k) = &sub {
                sh.set_elem(&name, k, &val, append)
            } else if (set_attrs & (ARRAY | ASSOC) != 0 || matches!(sh.vars.get(&name).and_then(|v| v.value.as_ref()), Some(Value::Arr(_)) | Some(Value::Assoc(_))))
                && val.starts_with('(')
            {
                match parse_compound_value(sh, &val) {
                    Some(elems) => sh.set_array(&name, elems, append),
                    None => sh.set_var(&name, &val, append),
                }
            } else if set_attrs & NAMEREF != 0 {
                let v = sh.vars.get_mut(&name).unwrap();
                v.value = Some(Value::Str(val.clone()));
                Ok(())
            } else {
                // bypass nameref resolution when declaring
                let attrs = sh.vars.get(&name).map(|v| v.attrs).unwrap_or(0);
                if attrs & NAMEREF != 0 && set_attrs & NAMEREF == 0 {
                    sh.set_var(&name, &val, append)
                } else {
                    sh.set_var(&name, &val, append)
                }
            };
            if let Err(e) = r {
                sh.eprint(&format!("{}: {}", cmd, e));
                st = 1;
                continue;
            }
        } else if set_attrs & INTEGER != 0 {
            // re-evaluate existing value
            if let Some(Value::Str(s)) = sh.vars.get(&name).and_then(|v| v.value.clone()) {
                let _ = sh.set_var(&name, &s, false);
            }
        }
        if set_attrs & READONLY != 0 {
            if let Some(v) = sh.vars.get_mut(&name) {
                v.attrs |= READONLY;
            }
        }
        if name == "PATH" {
            sh.hash.clear();
        }
    }
    Ok(st)
}

pub fn export(sh: &mut Shell, args: &[String]) -> R {
    let mut unexport = false;
    let mut funcs = false;
    let mut print = false;
    let mut names = Vec::new();
    for a in args {
        if a.starts_with('-') && a.len() > 1 && names.is_empty() && !a.contains('=') {
            for c in a[1..].chars() {
                match c {
                    'n' => unexport = true,
                    'f' => funcs = true,
                    'p' => print = true,
                    '-' => {}
                    _ => {
                        sh.eprint(&format!("export: -{}: invalid option", c));
                        return Ok(2);
                    }
                }
            }
        } else {
            names.push(a.clone());
        }
    }
    if names.is_empty() || print {
        let mut out = String::new();
        for (n, v) in sh.vars.visible() {
            if v.has(EXPORT) {
                match &v.value {
                    Some(Value::Str(s)) => out.push_str(&format!("declare -x {}={}\n", n, decl_quote(s))),
                    None => out.push_str(&format!("declare -x {}\n", n)),
                    _ => {}
                }
            }
        }
        sh.out(1, out.as_bytes())?;
        return Ok(0);
    }
    let mut st = 0;
    for (k, spec) in names.iter().enumerate() {
        if funcs {
            let set = Arc::make_mut(&mut sh.exported_funcs);
            if unexport {
                set.remove(spec);
            } else {
                set.insert(spec.clone());
            }
            continue;
        }
        let (name, value, append) = match spec.find('=') {
            Some(i) => {
                let l = &spec[..i];
                match l.strip_suffix('+') {
                    Some(l2) => (l2.to_string(), Some(spec[i + 1..].to_string()), true),
                    None => (l.to_string(), Some(spec[i + 1..].to_string()), false),
                }
            }
            None => (spec.clone(), None, false),
        };
        if !crate::parser::is_valid_name(&name) {
            sh.eprint(&format!("export: `{}': not a valid identifier", spec));
            st = 1;
            continue;
        }
        if let Some(v) = value {
            let argi = args.iter().position(|a| a == spec).map(|p| p + 1).unwrap_or(k + 1);
            let r = if let Some(elems) = sh.decl_arrays.get(&argi).cloned() {
                sh.set_array(&name, elems, append)
            } else {
                sh.set_var(&name, &v, append)
            };
            if let Err(e) = r {
                sh.eprint(&format!("export: {}", e));
                st = 1;
                continue;
            }
        }
        let n = sh.resolve_name(&name);
        let var = sh.vars.get_mut_or_create(&n);
        if unexport {
            var.attrs &= !EXPORT;
        } else {
            var.attrs |= EXPORT;
        }
        if n == "PATH" {
            sh.hash.clear();
        }
    }
    Ok(st)
}

pub fn readonly(sh: &mut Shell, args: &[String]) -> R {
    let names: Vec<String> = args.iter().filter(|a| !(a.starts_with('-') && !a.contains('='))).cloned().collect();
    let flags: String = args.iter().filter(|a| a.starts_with('-') && !a.contains('=')).map(|a| a[1..].to_string()).collect();
    if names.is_empty() {
        let mut out = String::new();
        for (n, v) in sh.vars.visible() {
            if v.has(READONLY) {
                if let Some(l) = declare_line(sh, &n) {
                    out.push_str(&l);
                    out.push('\n');
                }
            }
        }
        sh.out(1, out.as_bytes())?;
        return Ok(0);
    }
    let mut a: Vec<String> = vec![format!("-r{}", flags.replace('p', ""))];
    a.extend(args.iter().filter(|x| !(x.starts_with('-') && !x.contains('='))).cloned());
    // decl_arrays indexes refer to original argv positions; remap
    let mut remap = std::collections::HashMap::new();
    let mut pos = 1;
    for (i, x) in args.iter().enumerate() {
        if !(x.starts_with('-') && !x.contains('=')) {
            pos += 1;
            if let Some(v) = sh.decl_arrays.get(&(i + 1)) {
                remap.insert(pos, v.clone());
            }
        }
    }
    let saved = std::mem::replace(&mut sh.decl_arrays, remap);
    let saved_depth = sh.func_depth;
    sh.func_depth = 0; // readonly is always global-ish
    let r = declare(sh, "readonly", &a);
    sh.func_depth = saved_depth;
    sh.decl_arrays = saved;
    r
}

pub fn unset(sh: &mut Shell, args: &[String]) -> R {
    let mut mode = 'b';
    let mut st = 0;
    for a in args {
        match a.as_str() {
            "-f" => {
                mode = 'f';
                continue;
            }
            "-v" => {
                mode = 'v';
                continue;
            }
            "-n" => {
                mode = 'n';
                continue;
            }
            "--" => continue,
            _ => {}
        }
        if mode == 'f' {
            Arc::make_mut(&mut sh.funcs).remove(a.as_str());
            continue;
        }
        if mode == 'n' {
            sh.vars.unset(a);
            continue;
        }
        if let (Some(b), true) = (a.find('['), a.ends_with(']')) {
            let n = &a[..b];
            let k = &a[b + 1..a.len() - 1];
            let n = sh.resolve_name(n);
            let is_assoc = matches!(sh.vars.get(&n).and_then(|v| v.value.as_ref()), Some(Value::Assoc(_)));
            if k == "@" || k == "*" {
                if !is_assoc {
                    let _ = sh.unset_var(&n);
                    continue;
                }
            }
            let idx = if is_assoc { None } else { crate::arith::eval(k, sh).ok() };
            if let Some(v) = sh.vars.get_mut(&n) {
                if v.has(READONLY) {
                    sh.eprint(&format!("unset: {}: cannot unset: readonly variable", n));
                    st = 1;
                    continue;
                }
                match &mut v.value {
                    Some(Value::Assoc(m)) => m.remove(k),
                    Some(Value::Arr(arr)) => {
                        if let Some(mut i) = idx {
                            if i < 0 {
                                i += arr.keys().next_back().map(|x| x + 1).unwrap_or(0);
                            }
                            arr.remove(&i);
                        }
                    }
                    Some(Value::Str(_)) => {
                        if idx == Some(0) {
                            v.value = None;
                        }
                    }
                    None => {}
                }
            }
            continue;
        }
        if !crate::parser::is_valid_name(a) {
            sh.eprint(&format!("unset: `{}': not a valid identifier", a));
            st = 1;
            continue;
        }
        if mode == 'b' && sh.vars.get(a).is_none() && sh.funcs.contains_key(a.as_str()) {
            Arc::make_mut(&mut sh.funcs).remove(a.as_str());
            continue;
        }
        if let Err(e) = sh.unset_var(a) {
            sh.eprint(&format!("unset: {}", e));
            st = 1;
        }
    }
    Ok(st)
}

// ---------------- getopts / let ----------------

pub fn getopts(sh: &mut Shell, args: &[String]) -> R {
    if args.len() < 2 {
        sh.eprint("getopts: usage: getopts optstring name [arg ...]");
        return Ok(2);
    }
    let optstring = args[0].clone();
    let var = args[1].clone();
    let params: Vec<String> = if args.len() > 2 { args[2..].to_vec() } else { sh.positional.clone() };
    let silent = optstring.starts_with(':') || sh.get_str("OPTERR").map(|v| v == "0").unwrap_or(false);
    let ostr = optstring.trim_start_matches(':');
    let mut optind: usize = sh.get_str("OPTIND").and_then(|s| s.parse().ok()).unwrap_or(1).max(1);
    let saved_pos = sh.getopts_pos;
    let set_ind = |sh: &mut Shell, n: usize, pos: usize| {
        let _ = sh.set_var("OPTIND", &n.to_string(), false);
        sh.getopts_pos = pos;
    };
    let mut pos = saved_pos;
    if optind > params.len() {
        set_ind(sh, optind, 0);
        let _ = sh.set_var(&var, "?", false);
        return Ok(1);
    }
    let arg = params[optind - 1].clone();
    if pos == 0 {
        if arg == "--" {
            set_ind(sh, optind + 1, 0);
            let _ = sh.set_var(&var, "?", false);
            return Ok(1);
        }
        if !arg.starts_with('-') || arg == "-" {
            set_ind(sh, optind, 0);
            let _ = sh.set_var(&var, "?", false);
            return Ok(1);
        }
        pos = 1;
    }
    let chars: Vec<char> = arg.chars().collect();
    if pos >= chars.len() {
        set_ind(sh, optind + 1, 0);
        let _ = sh.set_var(&var, "?", false);
        return Ok(1);
    }
    let c = chars[pos];
    pos += 1;
    let at_end = pos >= chars.len();
    let spec_idx = if c == ':' { None } else { ostr.find(c) };
    match spec_idx {
        None => {
            if at_end {
                optind += 1;
                pos = 0;
            }
            set_ind(sh, optind, pos);
            if silent {
                let _ = sh.set_var("OPTARG", &c.to_string(), false);
            } else {
                let _ = sh.unset_var("OPTARG");
                let m = format!("{}: illegal option -- {}\n", sh.arg0, c);
                sh.write_fd(2, m.as_bytes());
            }
            let _ = sh.set_var(&var, "?", false);
            Ok(0)
        }
        Some(i) => {
            let needs_arg = ostr[i + c.len_utf8()..].starts_with(':');
            if needs_arg {
                if !at_end {
                    let rest: String = chars[pos..].iter().collect();
                    let _ = sh.set_var("OPTARG", &rest, false);
                    set_ind(sh, optind + 1, 0);
                } else if optind < params.len() {
                    let v = params[optind].clone();
                    let _ = sh.set_var("OPTARG", &v, false);
                    set_ind(sh, optind + 2, 0);
                } else {
                    set_ind(sh, optind + 1, 0);
                    if silent {
                        let _ = sh.set_var("OPTARG", &c.to_string(), false);
                        let _ = sh.set_var(&var, ":", false);
                    } else {
                        let _ = sh.unset_var("OPTARG");
                        let m = format!("{}: option requires an argument -- {}\n", sh.arg0, c);
                        sh.write_fd(2, m.as_bytes());
                        let _ = sh.set_var(&var, "?", false);
                    }
                    return Ok(0);
                }
            } else {
                let _ = sh.unset_var("OPTARG");
                if at_end {
                    set_ind(sh, optind + 1, 0);
                } else {
                    set_ind(sh, optind, pos);
                }
            }
            let _ = sh.set_var(&var, &c.to_string(), false);
            Ok(0)
        }
    }
}

pub fn let_(sh: &mut Shell, args: &[String]) -> R {
    if args.is_empty() {
        sh.eprint("let: expression expected");
        return Ok(1);
    }
    let mut v = 0;
    for a in args {
        match crate::arith::eval(a, sh) {
            Ok(x) => v = x,
            Err(e) => {
                sh.eprint(&format!("let: {}: {}", a, e));
                return Ok(1);
            }
        }
    }
    Ok(if v != 0 { 0 } else { 1 })
}

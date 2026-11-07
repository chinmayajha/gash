//! Cygwin's command-line tokenizer (`build_argv`, `quoted` and `globify` in winsup/cygwin/dcrt0.cc), as a test oracle for `msys_quote`.

use crate::exec::msys_quote;

const GLOB: &[u8] = b"<GLOB>";

fn is_sep(c: u8) -> bool {
    c == 0 || b" \t\n\r".contains(&c)
}

fn is_quote(c: u8) -> bool {
    c == b'"' || c == b'\''
}

fn is_dos_path(s: &[u8]) -> bool {
    let drive = s.len() >= 2 && s[0].is_ascii_alphabetic() && s[1] == b':';
    let unc = s.len() >= 3 && s[0] == b'\\' && s[1] == b'\\' && s[2].is_ascii_alphabetic() && s[3..].contains(&b'\\');
    drive || unc
}

fn is_dos_spec(word: &[u8]) -> bool {
    is_dos_path(word) || (is_quote(word[0]) && word.len() >= 3 && is_dos_path(&word[1..]))
}

fn quoted_plain(b: &mut Vec<u8>, cmd: usize) -> usize {
    let quote = b[cmd];
    b.remove(cmd);
    let mut p = cmd;
    while b[p] != 0 && b[p] != quote {
        p += 1;
    }
    if b[p] != 0 {
        b.remove(p);
    }
    p
}

fn quoted_winshell(b: &mut [u8], mut cmd: usize) -> usize {
    let quote = b[cmd];
    let stops: &[u8] = if quote == b'\'' { b"'" } else { b"\\\"" };
    while b[cmd] != 0 {
        cmd += 1;
        if b[cmd] == 0 {
            break;
        }
        let Some(off) = b[cmd..].iter().position(|c| stops.contains(c)) else {
            while b[cmd] != 0 {
                cmd += 1;
            }
            break;
        };
        let p = cmd + off;
        if b[p] == b'\\' {
            cmd = p + 1;
        } else if quote == b'"' && b[p + 1] == b'"' {
            b[p] = b'\\';
            cmd = p + 1;
        } else {
            return p + 1;
        }
    }
    cmd
}

fn quoted(b: &mut Vec<u8>, cmd: usize, plain: bool) -> usize {
    if plain {
        quoted_plain(b, cmd)
    } else {
        quoted_winshell(b, cmd)
    }
}

fn is_escape(word: &[u8], s: usize, quote: u8) -> bool {
    word[s] == b'\\' && matches!(word.get(s + 1), Some(&n) if n == quote || n == b'\\')
}

fn escape_quoted(word: &[u8], mut s: usize, dos: bool, pattern: &mut Vec<u8>) -> usize {
    let quote = word[s - 1];
    while s < word.len() && word[s] != quote {
        if !dos && is_escape(word, s, quote) {
            s += 1;
        }
        pattern.push(b'\\');
        pattern.push(word[s]);
        s += 1;
    }
    s + 1
}

fn glob_pattern(word: &[u8], dos: bool) -> Vec<u8> {
    let mut pattern = Vec::new();
    let mut s = 0;
    while s < word.len() {
        let c = word[s];
        s += 1;
        if is_quote(c) {
            s = escape_quoted(word, s, dos, &mut pattern);
        } else {
            if dos && c == b'\\' {
                pattern.push(b'\\');
            }
            pattern.push(c);
        }
    }
    pattern
}

fn glob_nomatch(pattern: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pattern.len() {
        let c = pattern[i];
        if c == b'\\' && i + 1 < pattern.len() {
            i += 1;
            out.push(pattern[i]);
        } else if b"*?[{".contains(&c) || (i == 0 && c == b'~') {
            return GLOB.to_vec();
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

fn globify(word: &[u8]) -> Option<Vec<u8>> {
    if word.first() != Some(&b'~') && !word.iter().any(|c| b"?*[\"'(){}".contains(c)) {
        return None;
    }
    Some(glob_nomatch(&glob_pattern(word, is_dos_spec(word))))
}

fn build_argv(cmdline: &str) -> Vec<Vec<u8>> {
    let mut b = cmdline.as_bytes().to_vec();
    b.push(0);
    let mut argv: Vec<Vec<u8>> = Vec::new();
    let mut cmd = 0;
    while b[cmd] != 0 {
        if is_sep(b[cmd]) {
            cmd += 1;
            continue;
        }
        let word = cmd;
        while b[cmd] != 0 {
            if is_quote(b[cmd]) {
                let plain = argv.is_empty() || b[word] == b'@';
                cmd = quoted(&mut b, cmd, plain);
            } else {
                cmd += 1;
            }
            if is_sep(b[cmd]) {
                break;
            }
        }
        let text = b[word..cmd].to_vec();
        if b[cmd] != 0 {
            cmd += 1;
        }
        let globbed = if argv.is_empty() { None } else { globify(&text) };
        argv.push(globbed.unwrap_or(text));
    }
    argv
}

fn assert_roundtrip(arg: &str) {
    let quoted = msys_quote(arg);
    let want = vec![b"prog".to_vec(), arg.as_bytes().to_vec(), b"next".to_vec()];
    assert_eq!(build_argv(&format!("prog {quoted} next")), want, "argument {arg:?} quoted as {quoted:?}");
    assert_eq!(build_argv(&format!("prog {quoted}")), want[..2], "last argument {arg:?} quoted as {quoted:?}");
}

fn assert_all_roundtrip(args: &[&str]) {
    args.iter().for_each(|a| assert_roundtrip(a));
}

#[test]
fn quote_inside_drive_path_stays_one_word() {
    assert_all_roundtrip(&["C:\\x\\\" -la", "C:\\dir\\file\"name"]);
}

#[test]
fn newline_inside_drive_path_stays_one_word() {
    assert_all_roundtrip(&["C:\\file\n-la", "C:\\file\r-la"]);
}

#[test]
fn trailing_backslash_does_not_swallow_next_word() {
    assert_all_roundtrip(&["C:\\Program Files\\", "C:\\dir\\", "a b\\"]);
}

#[test]
fn rooted_path_is_not_read_as_unc() {
    assert_all_roundtrip(&["\\Windows\\System32", "\\dir\\sub dir"]);
}

#[test]
fn non_ascii_survives() {
    assert_all_roundtrip(&["C:\\café dir\\ü", "naïve file"]);
}

#[test]
fn simple_arguments_keep_their_quoting() {
    for (arg, quoted) in [
        ("plain", "plain"),
        ("", "\"\""),
        ("a b", "\"a b\""),
        ("C:\\Windows\\System32", "C:\\Windows\\System32"),
        ("~x", "\"~x\""),
        ("@x", "\"@x\""),
    ] {
        assert_eq!(msys_quote(arg), quoted);
        assert_roundtrip(arg);
    }
}

#[test]
fn every_short_argument_roundtrips() {
    const ALPHABET: &[char] = &['C', ':', '\\', '"', '\'', ' ', '\n', '\t', '*', '~', '@', 'x'];
    let mut args = vec![String::new()];
    let mut start = 0;
    for _ in 0..5 {
        let end = args.len();
        for i in start..end {
            for c in ALPHABET {
                args.push(format!("{}{c}", args[i]));
            }
        }
        start = end;
    }
    for a in &args {
        assert_roundtrip(a);
    }
}

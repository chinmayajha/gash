//! Path helpers. The original gash translated POSIX paths to Windows/MSYS ones; on Unix the
//! shell's paths are already the native ones, so these are identities.

use std::path::{Path, PathBuf};

pub fn init() {}

pub fn is_windows_abs(_s: &str) -> bool {
    false
}

/// Resolve a possibly relative POSIX path against `cwd`.
pub fn to_win(s: &str, cwd: &Path) -> PathBuf {
    if s.is_empty() {
        return cwd.to_path_buf();
    }
    if s.starts_with('/') {
        PathBuf::from(s)
    } else {
        cwd.join(s)
    }
}

pub fn to_posix(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Normalize a POSIX path lexically: collapse `//`, remove `.`, resolve `..`.
pub fn normalize_posix(p: &str) -> String {
    let abs = p.starts_with('/');
    let unc = p.starts_with("//") && !p.starts_with("///");
    let mut out: Vec<&str> = Vec::new();
    for comp in p.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if out.last().map(|l| *l != "..").unwrap_or(false) {
                    out.pop();
                } else if !abs {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    let joined = out.join("/");
    if unc {
        format!("//{}", joined)
    } else if abs {
        format!("/{}", joined)
    } else if joined.is_empty() {
        ".".into()
    } else {
        joined
    }
}

pub fn split_posix_list(list: &str) -> Vec<String> {
    list.split(':').map(|s| s.to_string()).collect()
}

pub fn is_msys_binary(_p: &Path) -> bool {
    false
}

pub fn win_fwd(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity() {
        let cwd = Path::new("/Users/x");
        assert_eq!(to_win("/a/b", cwd), PathBuf::from("/a/b"));
        assert_eq!(to_win("foo/bar", cwd), PathBuf::from("/Users/x/foo/bar"));
        assert_eq!(to_win("../y", cwd), PathBuf::from("/Users/x/../y"));
        assert_eq!(to_posix(Path::new("/a/b")), "/a/b");
        assert_eq!(normalize_posix("/a/b/../c/./d//e"), "/a/c/d/e");
        assert_eq!(normalize_posix("/.."), "/");
        assert_eq!(split_posix_list("/a:/b:/c"), vec!["/a", "/b", "/c"]);
    }
}

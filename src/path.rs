//! Path helpers.

use std::path::{Path, PathBuf};

/// Resolve a possibly relative POSIX path against `cwd`.
pub fn resolve(s: &str, cwd: &Path) -> PathBuf {
    if s.is_empty() {
        return cwd.to_path_buf();
    }
    if s.starts_with('/') {
        PathBuf::from(s)
    } else {
        cwd.join(s)
    }
}

pub fn path_str(p: &Path) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_lexically() {
        assert_eq!(normalize_posix("/a/b/../c/./d//e"), "/a/c/d/e");
        assert_eq!(normalize_posix("/.."), "/");
        assert_eq!(normalize_posix("a/../.."), "..");
    }
}

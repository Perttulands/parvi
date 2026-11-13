//! Paths as a node writes them: `/` on Linux and WSL, `\` on Windows. Every
//! helper takes the separator from the path it is given.

use parvi_files_contract::Root;

pub fn separator(path: &str) -> char {
    if path.contains('\\') && !path.contains('/') {
        '\\'
    } else {
        '/'
    }
}

/// The line Send hands a file or folder over with (CHROTE's `path …`).
pub fn reference(path: &str) -> String {
    format!("path {path}")
}

/// The path of `name` inside `folder`.
pub fn join(folder: &str, name: &str) -> String {
    let sep = separator(folder);
    if folder.ends_with(sep) {
        format!("{folder}{name}")
    } else {
        format!("{folder}{sep}{name}")
    }
}

/// The last name in a path.
pub fn base_name(path: &str) -> &str {
    path.split(separator(path))
        .rfind(|p| !p.is_empty())
        .unwrap_or(path)
}

/// The parent of a path, keeping a Windows drive's root separator.
pub fn parent(path: &str) -> String {
    let sep = separator(path);
    let trimmed = if path.len() > 1 {
        path.trim_end_matches(sep)
    } else {
        path
    };
    match trimmed.rfind(sep) {
        Some(0) => sep.to_string(),
        Some(i) => {
            let part = &trimmed[..i];
            if is_drive(part) {
                format!("{part}{sep}")
            } else {
                part.to_string()
            }
        }
        None => trimmed.to_string(),
    }
}

fn is_drive(part: &str) -> bool {
    let p = part.strip_prefix("\\\\?\\").unwrap_or(part);
    p.len() == 2 && p.as_bytes()[0].is_ascii_alphabetic() && p.as_bytes()[1] == b':'
}

/// Whether `path` is `folder` or under it.
pub fn inside(folder: &str, path: &str) -> bool {
    path == folder || path.starts_with(&join(folder, ""))
}

/// The root a path is under: the longest that holds it.
pub fn root_of<'a>(roots: &'a [Root], path: &str) -> Option<&'a Root> {
    roots
        .iter()
        .filter(|r| inside(&r.path, path))
        .max_by_key(|r| r.path.len())
}

/// A file's name in at most `most` letters: the middle gives way, the
/// start and the extension stay (`screenshot-…-final.png`).
pub fn elide_name(name: &str, most: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= most || most < 5 {
        return name.to_string();
    }
    let ext = name
        .rfind('.')
        .filter(|&at| at > 0)
        .map_or(0, |at| name[at..].chars().count());
    let tail = ext.max((most - 1) / 2).min(most - 2);
    let head = most - 1 - tail;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// A path as the operator reads it: under its root's name (`~/kuvio`).
pub fn shown(roots: &[Root], path: &str) -> String {
    match root_of(roots, path) {
        Some(r) if r.path == path => r.name.clone(),
        Some(r) => {
            let rest = &path[join(&r.path, "").len()..];
            join(&r.name, rest)
        }
        None => path.to_string(),
    }
}

/// `path` relative to `folder`, for find results.
pub fn relative<'a>(folder: &str, path: &'a str) -> &'a str {
    path.strip_prefix(&join(folder, "")).unwrap_or(path)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crumb {
    pub name: String,
    pub path: String,
}

/// The way from the root to `path`: the root by its name, then each folder.
pub fn crumbs(root: &Root, path: &str) -> Vec<Crumb> {
    let mut trail = vec![Crumb {
        name: root.name.clone(),
        path: root.path.clone(),
    }];
    if !inside(&root.path, path) || path == root.path {
        return trail;
    }
    let sep = separator(&root.path);
    let mut at = root.path.clone();
    for name in path[join(&root.path, "").len()..]
        .split(sep)
        .filter(|n| !n.is_empty())
    {
        at = join(&at, name);
        trail.push(Crumb {
            name: name.to_string(),
            path: at.clone(),
        });
    }
    trail
}

/// Why a name can't be used for a new or renamed entry (CHROTE fileService
/// `sanitizeFilename`).
pub fn bad_name(name: &str) -> Option<&'static str> {
    let t = name.trim();
    if t.is_empty() {
        Some("A name is needed")
    } else if t.contains(['/', '\\']) {
        Some("A name has no / or \\")
    } else if t.chars().all(|c| c == '.') {
        Some("A name is more than dots")
    } else if t.chars().any(|c| c.is_control()) {
        Some("A name has no control characters")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_long_name_gives_way_in_the_middle_and_keeps_its_extension() {
        assert_eq!(elide_name("chart.png", 20), "chart.png");
        assert_eq!(
            elide_name("screenshot-2026-11-13-final.png", 16),
            "screensh…nal.png"
        );
        assert_eq!(elide_name("a-very-long-name-without-dot", 10), "a-ver…-dot");
        assert_eq!(
            elide_name("chart.png", 3),
            "chart.png",
            "too few to say anything"
        );
    }

    use super::*;

    #[test]
    fn a_file_is_handed_over_by_its_path() {
        assert_eq!(reference("/srv/x/y.rs"), "path /srv/x/y.rs");
        assert_eq!(reference(r"C:\a b\c.txt"), r"path C:\a b\c.txt");
    }

    fn root(name: &str, path: &str) -> Root {
        Root {
            name: name.into(),
            path: path.into(),
        }
    }

    #[test]
    fn unix_and_windows_paths() {
        assert_eq!(join("/home/p", "a"), "/home/p/a");
        assert_eq!(join("C:\\", "Users"), "C:\\Users");
        assert_eq!(parent("/home/p/a"), "/home/p");
        assert_eq!(parent("/home"), "/");
        assert_eq!(parent("C:\\Users"), "C:\\");
        assert_eq!(parent("C:\\Users\\p"), "C:\\Users");
        assert_eq!(base_name("C:\\Users\\p\\x.md"), "x.md");
        assert_eq!(base_name("/a/b/"), "b");
    }

    #[test]
    fn roots_name_the_way() {
        let roots = [root("~", "/home/p"), root("/srv", "/srv")];
        assert_eq!(shown(&roots, "/home/p/kuvio/models"), "~/kuvio/models");
        assert_eq!(shown(&roots, "/home/p"), "~");
        assert_eq!(shown(&roots, "/home/pq"), "/home/pq");
        let c = crumbs(&roots[0], "/home/p/kuvio/models");
        let names: Vec<_> = c.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["~", "kuvio", "models"]);
        assert_eq!(c[2].path, "/home/p/kuvio/models");
        let w = root("C:", "C:\\");
        let c = crumbs(&w, "C:\\Users\\p");
        assert_eq!(c.last().unwrap().path, "C:\\Users\\p");
        assert_eq!(relative("/srv", "/srv/a/b.md"), "a/b.md");
    }

    #[test]
    fn names_that_cannot_be_used() {
        assert!(bad_name("").is_some());
        assert!(bad_name("a/b").is_some());
        assert!(bad_name("..").is_some());
        assert!(bad_name("ok.md").is_none());
    }
}

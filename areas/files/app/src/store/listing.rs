//! What the tree shows of a folder: folders first, then files, each with
//! the hidden (dot) ones after the rest, in natural order (`file2` before
//! `file10`).

use parvi_files_contract::Entry;
use std::cmp::Ordering;

/// Compares names as people count: digit runs by value, the rest without case.
pub fn natural(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let n: String = std::iter::from_fn(|| x.next_if(|c| c.is_ascii_digit())).collect();
                let m: String = std::iter::from_fn(|| y.next_if(|c| c.is_ascii_digit())).collect();
                let (n, m) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let o = n.len().cmp(&m.len()).then_with(|| n.cmp(m));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// A copy's staging folder (docs/files.md, Bridge): the node keeps it
/// beside the copy, but it is the node's bookkeeping, not the operator's.
pub const STAGING: &str = ".parvi-transfer-";

/// The entries a folder shows, in order.
pub fn shown(entries: &[Entry]) -> Vec<&Entry> {
    let mut out: Vec<&Entry> = entries
        .iter()
        .filter(|e| !(e.folder && e.name.starts_with(STAGING)))
        .collect();
    out.sort_by(|a, b| {
        let hidden = |e: &Entry| e.name.starts_with('.');
        b.folder
            .cmp(&a.folder)
            .then(hidden(a).cmp(&hidden(b)))
            .then_with(|| natural(&a.name, &b.name))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, folder: bool) -> Entry {
        Entry {
            name: name.into(),
            folder,
            size: 1,
            modified: 1,
        }
    }

    #[test]
    fn numbers_sort_by_value() {
        let mut v = vec!["file10", "File2", "file1", "a", "file02b"];
        v.sort_by(|a, b| natural(a, b));
        assert_eq!(v, ["a", "file1", "File2", "file02b", "file10"]);
    }

    #[test]
    fn folders_first_then_hidden_last_by_name() {
        let list = [
            e("b10.txt", false),
            e("z", true),
            e("b9.txt", false),
            e("c", true),
            e(".cache", true),
            e(".bashrc", false),
            e(".parvi-transfer-0123456789abcdef0123456789abcdef", true),
        ];
        let names: Vec<&str> = shown(&list).iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["c", "z", ".cache", "b9.txt", "b10.txt", ".bashrc"]);
    }
}

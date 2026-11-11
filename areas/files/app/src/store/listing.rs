//! What a pane shows of a folder: folders first, then files, by the chosen
//! column in natural order (`file2` before `file10`), filtered by name.

use parvi_files_contract::Entry;
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Column {
    #[default]
    Name,
    Size,
    Modified,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Sort {
    pub by: Column,
    pub descending: bool,
}

impl Sort {
    /// A header click: the same column turns, another starts ascending
    /// (newest and largest first for those).
    pub fn toggled(self, by: Column) -> Self {
        if self.by == by {
            Self {
                by,
                descending: !self.descending,
            }
        } else {
            Self {
                by,
                descending: by != Column::Name,
            }
        }
    }
}

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

/// The entries a pane lists, in order.
/// A copy's staging folder (docs/files.md, Bridge): the node keeps it
/// beside the copy, but it is the node's bookkeeping, not the operator's.
pub const STAGING: &str = ".parvi-transfer-";

pub fn shown<'a>(entries: &'a [Entry], filter: &str, sort: Sort) -> Vec<&'a Entry> {
    let q = filter.trim().to_lowercase();
    let mut out: Vec<&Entry> = entries
        .iter()
        .filter(|e| !(e.folder && e.name.starts_with(STAGING)))
        .filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q))
        .collect();
    out.sort_by(|a, b| {
        b.folder.cmp(&a.folder).then_with(|| {
            let o = match sort.by {
                Column::Name => Ordering::Equal,
                Column::Size => a.size.cmp(&b.size),
                Column::Modified => a.modified.cmp(&b.modified),
            }
            .then_with(|| natural(&a.name, &b.name));
            if sort.descending { o.reverse() } else { o }
        })
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, folder: bool, size: u64, modified: u64) -> Entry {
        Entry {
            name: name.into(),
            folder,
            size,
            modified,
        }
    }

    #[test]
    fn numbers_sort_by_value() {
        let mut v = vec!["file10", "File2", "file1", "a", "file02b"];
        v.sort_by(|a, b| natural(a, b));
        assert_eq!(v, ["a", "file1", "File2", "file02b", "file10"]);
    }

    #[test]
    fn folders_first_then_the_column() {
        let list = [
            e("b.txt", false, 5, 3),
            e("z", true, 0, 1),
            e("a.txt", false, 50, 2),
            e("c", true, 0, 9),
            e(
                ".parvi-transfer-0123456789abcdef0123456789abcdef",
                true,
                0,
                9,
            ),
        ];
        let names = |s: Sort, f: &str| -> Vec<String> {
            shown(&list, f, s).iter().map(|e| e.name.clone()).collect()
        };
        assert_eq!(names(Sort::default(), ""), ["c", "z", "a.txt", "b.txt"]);
        let by_size = Sort::default().toggled(Column::Size);
        assert!(by_size.descending);
        assert_eq!(names(by_size, ""), ["z", "c", "a.txt", "b.txt"]);
        assert_eq!(
            names(by_size.toggled(Column::Size), ""),
            ["c", "z", "b.txt", "a.txt"]
        );
        assert_eq!(names(Sort::default(), "TXT"), ["a.txt", "b.txt"]);
    }
}

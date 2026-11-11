//! Draft changes after moving a file or folder.
use super::paths;
use std::collections::HashMap;

/// Every draft for `from` or a file under it, on `node`, moves to `to`.
pub fn move_drafts<D>(drafts: &mut HashMap<(String, String), D>, node: &str, from: &str, to: &str) {
    let under: Vec<_> = drafts
        .keys()
        .filter(|(n, p)| n == node && paths::inside(from, p))
        .cloned()
        .collect();
    for key in under {
        let draft = drafts.remove(&key).expect("just listed");
        drafts.insert((key.0, moved(&key.1, from, to)), draft);
    }
}

/// `path` after `from`, which it is or is under, became `to`.
fn moved(path: &str, from: &str, to: &str) -> String {
    format!("{to}{}", &path[from.len()..])
}

/// What to say when a change got no answer: it may have happened, so the
/// folder is listed again before anything is retried.
pub fn unknown(doing: &str) -> String {
    format!(
        "No answer {doing}; it may have happened. Check the folder, listed again, before trying again."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_rename_moves_the_drafts_of_every_file_under_it() {
        let key = |n: &str, p: &str| (n.to_string(), p.to_string());
        let mut drafts = HashMap::from([
            (key("a", "/w/dir/x.md"), 1),
            (key("a", "/w/dir/deep/y.md"), 2),
            (key("a", "/w/dir2/z.md"), 3),
            (key("b", "/w/dir/x.md"), 4),
        ]);
        move_drafts(&mut drafts, "a", "/w/dir", "/w/new");
        assert_eq!(
            drafts,
            HashMap::from([
                (key("a", "/w/new/x.md"), 1),
                (key("a", "/w/new/deep/y.md"), 2),
                (key("a", "/w/dir2/z.md"), 3),
                (key("b", "/w/dir/x.md"), 4),
            ])
        );
    }
}

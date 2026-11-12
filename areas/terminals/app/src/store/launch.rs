//! The launcher's rules, kept apart from its view so they can be tested.
//! `command_line` must build exactly what the node types into the new
//! session (`src/launch.rs`): the preview is a promise, but for the hooks
//! Notify adds, which its own switch stands for.

use parvi_terminals_contract::Session;

/// What a launch types into the new session's shell. Empty for the shell.
pub fn command_line(command: &str, flags: &str) -> String {
    let (c, f) = (command.trim(), flags.trim());
    match (c.is_empty(), f.is_empty()) {
        (true, _) => String::new(),
        (false, true) => c.into(),
        (false, false) => format!("{c} {f}"),
    }
}

/// What Notify promises for a harness's command, `None` if the node can't
/// hook it (docs/protocol.md, Agent events): it knows Claude Code and Codex
/// by the program's name.
pub fn notify_label(command: &str) -> Option<&'static str> {
    let program = command.trim().rsplit(['/', '\\']).next()?;
    let program = program.split('.').next()?;
    match program {
        "claude" => Some("Notify when it finishes or needs you"),
        "codex" => Some("Notify when it finishes"),
        _ => None,
    }
}

/// Why tmux would not take this name, or `None` if it would.
pub fn name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("Name the session.");
    }
    if name.chars().count() > 50 {
        return Some("At most 50 characters.");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Some("Letters, digits, _ and - only.");
    }
    None
}

/// The folder's last part; Windows paths (`~\work`, `C:\src`) split at
/// either slash, and a drive root is its letter.
fn folder_part(folder: &str) -> &str {
    let seps = ['/', '\\'];
    let last = folder
        .trim()
        .trim_end_matches(seps)
        .rsplit(seps)
        .next()
        .unwrap_or("")
        .trim_end_matches(':');
    if last.is_empty() || last == "~" {
        "home"
    } else {
        last
    }
}

/// The word a session name starts with for a harness.
pub fn short(harness: &str) -> &str {
    if harness == "claude-code" {
        "claude"
    } else {
        harness
    }
}

/// `<harness>-<folder>`, made safe, with `-2`, `-3`… if it is taken.
pub fn suggest_name(harness: &str, folder: &str, taken: &[&str]) -> String {
    let raw = format!("{}-{}", short(harness), folder_part(folder));
    let mut base = String::new();
    let mut dash = false;
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            base.push(c);
            dash = false;
        } else if !dash {
            base.push('-');
            dash = true;
        }
    }
    let base: String = base.chars().take(46).collect();
    if !taken.contains(&base.as_str()) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|n| !taken.contains(&n.as_str()))
        .unwrap_or(base)
}

/// The label a harness gets in the launcher.
pub fn harness_label(id: &str) -> String {
    match id {
        "claude-code" | "claude" => "Claude Code".into(),
        "codex" => "Codex".into(),
        "shell" => "Shell".into(),
        "pi" => "Pi".into(),
        other => {
            let mut c = other.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    }
}

/// The node a launcher launches on: the one picked, while the app still
/// knows it, else the serving node.
pub fn place(
    picked: Option<&str>,
    here: Option<&str>,
    known: impl Fn(&str) -> bool,
) -> Option<String> {
    picked.filter(|n| known(n)).or(here).map(str::to_string)
}

/// What the Folder field's list holds, as CHROTE's FolderField decides it:
/// the offered folders until something is typed; for a path, the folders
/// under the folder typed so far; for a fragment, the offered folders it
/// matches. (CHROTE ranks the host's workspaces there; a node lists no
/// workspaces, so the fragment searches what the launcher offers.)
#[derive(Debug, PartialEq)]
pub enum FolderList {
    Offered,
    Matching,
    /// `dir` ends in its separator; `segment` is what follows it.
    Inside {
        dir: String,
        segment: String,
    },
}

/// A path the node can list: `/…` or a drive (`C:\…`, `C:/…`).
fn absolute(value: &str) -> bool {
    let b = value.as_bytes();
    value.starts_with('/')
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'/' | b'\\'))
}

pub fn folder_list(value: &str, editing: bool) -> FolderList {
    if !editing || value.is_empty() {
        FolderList::Offered
    } else if absolute(value) {
        let cut = value.rfind(['/', '\\']).map_or(0, |i| i + 1);
        FolderList::Inside {
            dir: value[..cut].to_string(),
            segment: value[cut..].to_string(),
        }
    } else {
        FolderList::Matching
    }
}

/// A row of the Folder field's list: the folder, and what Tab writes, which
/// for a listed folder ends in its separator so the next Tab lists inside.
#[derive(Debug, PartialEq)]
pub struct Suggestion {
    pub path: String,
    pub complete: String,
}

/// The folders in `dir` whose name starts with `segment`, case aside, in
/// the listing's order with dot-folders last.
pub fn inside(dir: &str, segment: &str, folders: &[String]) -> Vec<Suggestion> {
    let wanted = segment.to_lowercase();
    let sep = if dir.ends_with('\\') { '\\' } else { '/' };
    let names = folders
        .iter()
        .filter(|n| n.to_lowercase().starts_with(&wanted));
    let (dotted, plain): (Vec<&String>, Vec<&String>) = names.partition(|n| n.starts_with('.'));
    plain
        .into_iter()
        .chain(dotted)
        .map(|n| Suggestion {
            path: format!("{dir}{n}"),
            complete: format!("{dir}{n}{sep}"),
        })
        .collect()
}

/// CHROTE's fuzzy score for a word. Scores are doubled to keep the gap
/// penalty (1.5 per skipped character) integral.
fn word_score(word: &str, candidate: &str) -> Option<i64> {
    let text: Vec<char> = candidate.chars().collect();
    let mut previous = vec![None; text.len()];
    for (letter_index, letter) in word.chars().enumerate() {
        let mut next = vec![None; text.len()];
        for (i, ch) in text.iter().enumerate() {
            if ch.to_lowercase().to_string() != letter.to_lowercase().to_string() {
                continue;
            }
            let start = i == 0
                || !text[i - 1].is_ascii_alphanumeric()
                || (text[i - 1].is_ascii_lowercase() && ch.is_ascii_uppercase());
            let base = 2 + if start { 6 } else { 0 };
            next[i] = if letter_index == 0 {
                Some(base)
            } else {
                (0..i)
                    .filter_map(|j| {
                        previous[j].map(|score| {
                            score
                                + base
                                + if j + 1 == i {
                                    6
                                } else {
                                    -3 * (i - j - 1) as i64
                                }
                        })
                    })
                    .max()
            };
        }
        previous = next;
    }
    previous
        .into_iter()
        .flatten()
        .max()
        .map(|score| (score * 5).max(1))
}

/// Rank the last two path segments as CHROTE does, keeping input order on ties.
pub fn matching(fragment: &str, offered: &[String]) -> Vec<Suggestion> {
    let words: Vec<&str> = fragment.split_whitespace().collect();
    if words.is_empty() {
        return vec![];
    }
    let mut found: Vec<(&String, i64)> = offered
        .iter()
        .filter_map(|path| {
            let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
            let tail = parts[parts.len().saturating_sub(2)..].join("/");
            let score: Option<i64> = words.iter().map(|w| word_score(w, &tail)).sum();
            score.map(|score| (path, score))
        })
        .collect();
    found.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    found
        .into_iter()
        .map(|(f, _)| Suggestion {
            path: f.clone(),
            complete: f.clone(),
        })
        .collect()
}

/// The row lit before the arrows move: the best match for a fragment, and
/// none otherwise, so Enter on a typed path means that path.
pub fn first_lit(list: &FolderList, rows: usize) -> Option<usize> {
    (*list == FolderList::Matching && rows > 0).then_some(0)
}

/// Where an arrow moves the lit row among `rows`: down stops at the last;
/// up stops at the first for a fragment, and above it, at none, otherwise.
pub fn step(lit: Option<usize>, rows: usize, down: bool, list: &FolderList) -> Option<usize> {
    if rows == 0 {
        return lit;
    }
    match (lit, down) {
        (None, true) => Some(0),
        (Some(i), true) => Some((i + 1).min(rows - 1)),
        (Some(0), false) if *list != FolderList::Matching => None,
        (Some(i), false) => Some(i.saturating_sub(1)),
        (None, false) => None,
    }
}

/// How many folders of live sessions the launcher offers.
const RECENT: usize = 5;

/// The folders the launcher offers: those launched in on this device, the
/// latest first, so the first is where the next launch goes; then the
/// node's own; then the folders `user`'s live sessions sit in, the newest
/// first, at most five of them.
pub fn folders(
    launched: &[String],
    pinned: &[String],
    sessions: &[Session],
    user: &str,
) -> Vec<String> {
    let mut mine: Vec<&Session> = sessions
        .iter()
        .filter(|s| s.user.as_deref().unwrap_or("") == user)
        .collect();
    mine.sort_by_key(|s| std::cmp::Reverse(s.last.unwrap_or(0)));
    let mut out: Vec<String> = Vec::new();
    for folder in launched.iter().chain(pinned) {
        if !out.contains(folder) {
            out.push(folder.clone());
        }
    }
    let mut recent = 0;
    for cwd in mine.iter().filter_map(|s| s.cwd.as_deref().map(str::trim)) {
        if recent == RECENT {
            break;
        }
        if !cwd.is_empty() && !out.iter().any(|f| f == cwd) {
            out.push(cwd.to_string());
            recent += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_suggested_from_harness_and_folder() {
        assert_eq!(
            suggest_name("claude-code", "/srv/chrote", &[]),
            "claude-chrote"
        );
        assert_eq!(suggest_name("codex", "~", &[]), "codex-home");
        assert_eq!(suggest_name("shell", "~\\", &[]), "shell-home");
        assert_eq!(suggest_name("shell", r"C:\Users\me\api\", &[]), "shell-api");
        assert_eq!(suggest_name("shell", r"D:\", &[]), "shell-D");
        assert_eq!(
            suggest_name("shell", "/srv/my app/", &["shell-my-app"]),
            "shell-my-app-2"
        );
    }

    #[test]
    fn tmux_name_rules() {
        assert_eq!(name_problem(""), Some("Name the session."));
        assert_eq!(name_problem("a b"), Some("Letters, digits, _ and - only."));
        assert_eq!(
            name_problem(&"x".repeat(51)),
            Some("At most 50 characters.")
        );
        assert_eq!(name_problem("claude-srv_2"), None);
    }

    #[test]
    fn the_preview_is_what_the_node_types() {
        assert_eq!(command_line("claude", " --x "), "claude --x");
        assert_eq!(command_line("", "--x"), "");
        assert_eq!(harness_label("claude-code"), "Claude Code");
    }

    #[test]
    fn notify_is_offered_for_the_harnesses_the_node_hooks() {
        assert!(notify_label("/usr/local/bin/claude").is_some());
        assert!(notify_label(r"C:\tools\codex.exe").is_some());
        assert_eq!(notify_label("aider"), None);
        assert_eq!(notify_label(""), None);
    }

    #[test]
    fn a_launch_goes_where_it_was_pointed_while_that_node_is_known() {
        let known = |n: &str| n == "rig-win" || n == "rig-wsl";
        assert_eq!(
            place(Some("rig-win"), Some("rig-wsl"), known).as_deref(),
            Some("rig-win")
        );
        assert_eq!(
            place(Some("gone"), Some("rig-wsl"), known).as_deref(),
            Some("rig-wsl"),
            "a node the fleet no longer lists gives way to the serving one"
        );
        assert_eq!(
            place(None, Some("rig-wsl"), known).as_deref(),
            Some("rig-wsl")
        );
        assert_eq!(place(None, None, known), None);
    }

    // CHROTE's FolderField tests, on the rules under the Parvi field.

    #[test]
    fn the_offered_folders_stand_until_something_is_typed() {
        assert_eq!(folder_list("/srv/chrote", false), FolderList::Offered);
        assert_eq!(folder_list("", true), FolderList::Offered);
        assert_eq!(folder_list("rep VSK", true), FolderList::Matching);
        assert_eq!(first_lit(&FolderList::Offered, 2), None);
    }

    #[test]
    fn a_fragment_ranks_the_offered_folders_with_the_best_lit() {
        let offered = [
            "/srv/chrote".to_string(),
            "/home/operator/repos/VSK-Zone".to_string(),
            "/srv/context-citadel".to_string(),
        ];
        let found = matching("rep VSK", &offered);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "/home/operator/repos/VSK-Zone");
        assert_eq!(first_lit(&FolderList::Matching, found.len()), Some(0));
        let offered = ["/srv/music".to_string(), "/srv/chrote".to_string()];
        let paths: Vec<String> = matching("c", &offered)
            .into_iter()
            .map(|s| s.path)
            .collect();
        assert_eq!(
            paths,
            ["/srv/chrote", "/srv/music"],
            "a last part starting with the word comes first"
        );
    }

    #[test]
    fn fuzzy_folder_ranking_matches_chrote() {
        let paths = [
            "/srv/chroma-tester",
            "/srv/chrote",
            "/home/operator/repos/VSK-Zone",
            "/home/operator/repos/vsk-notes",
            "/VSK/elsewhere/project",
        ]
        .map(str::to_string);
        let names = |query| {
            matching(query, &paths)
                .into_iter()
                .map(|s| s.path)
                .collect::<Vec<_>>()
        };
        assert_eq!(names("chrote"), ["/srv/chrote", "/srv/chroma-tester"]);
        assert_eq!(
            names("rep VSK"),
            [
                "/home/operator/repos/VSK-Zone",
                "/home/operator/repos/vsk-notes"
            ]
        );
        assert!(names("xyz").is_empty());
        let ties = [r"C:\repos\same", r"D:\repos\same"].map(str::to_string);
        assert_eq!(
            matching("same", &ties)
                .into_iter()
                .map(|s| s.path)
                .collect::<Vec<_>>(),
            ties
        );
    }

    #[test]
    fn a_typed_path_lists_the_folders_under_it_dot_folders_last() {
        let list = folder_list("/home/operator/re", true);
        assert_eq!(
            list,
            FolderList::Inside {
                dir: "/home/operator/".into(),
                segment: "re".into()
            }
        );
        let names = [".cache", "research", "repos"].map(String::from);
        let all: Vec<String> = inside("/home/operator/", "", &names)
            .into_iter()
            .map(|s| s.path)
            .collect();
        assert_eq!(
            all,
            [
                "/home/operator/research",
                "/home/operator/repos",
                "/home/operator/.cache"
            ]
        );
        let some = inside("/home/operator/", "RE", &names);
        assert_eq!(some.len(), 2);
        assert_eq!(
            first_lit(&list, some.len()),
            None,
            "Enter means the typed path"
        );
        let lit = step(step(None, 2, true, &list), 2, true, &list);
        assert_eq!(lit, Some(1));
        assert_eq!(
            some[1].complete, "/home/operator/repos/",
            "Tab lists inside next"
        );
        assert_eq!(step(Some(0), 2, false, &list), None);
        assert_eq!(step(Some(0), 2, false, &FolderList::Matching), Some(0));
    }

    #[test]
    fn a_windows_path_completes_with_its_own_separator() {
        assert_eq!(
            folder_list(r"C:\Users\me\sr", true),
            FolderList::Inside {
                dir: r"C:\Users\me\".into(),
                segment: "sr".into()
            }
        );
        let s = inside(r"C:\Users\me\", "sr", &["src".to_string()]);
        assert_eq!(s[0].complete, r"C:\Users\me\src\");
        assert_eq!(folder_list("~/work", true), FolderList::Matching);
    }

    #[test]
    fn folders_are_the_nodes_then_the_users_live_sessions_newest_first() {
        let at = |cwd: &str, user: Option<&str>, last: u64| Session {
            cwd: Some(cwd.into()),
            user: user.map(Into::into),
            last: Some(last),
            ..Default::default()
        };
        let live = [
            at("/srv/a", None, 1),
            at("/srv/b", None, 3),
            at("~", None, 2),
            at("/home/alice", Some("alice"), 9),
        ];
        assert_eq!(
            folders(&[], &["~".into()], &live, ""),
            ["~", "/srv/b", "/srv/a"]
        );
        assert_eq!(folders(&[], &[], &live, "alice"), ["/home/alice"]);
        // Where this device launched last comes first, so it is where the
        // next launch goes, then the rest in the order they were used.
        let launched = ["/srv/a".to_string(), "/tmp/proj".to_string()];
        assert_eq!(
            folders(&launched, &["~".into()], &live, ""),
            ["/srv/a", "/tmp/proj", "~", "/srv/b"]
        );
    }
}

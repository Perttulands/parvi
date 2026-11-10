//! Finding and describing sessions: the rail's groups and search, the marks
//! a row carries, and Send's ranking. Pure, so it is tested on its own.

use parvi_terminals_contract::{AgentEvent, EventKind, Session};
use serde::{Deserialize, Serialize};

/// Which node and session, across tmux restarts: tmux's ids start over when
/// its server does, names don't. `user` is whose server, "" for the service
/// user's own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Key {
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub user: String,
    pub name: String,
}

impl Key {
    pub fn of(s: &Session) -> Self {
        Key {
            node: s.node.clone(),
            user: s.user.clone().unwrap_or_default(),
            name: s.name.clone(),
        }
    }

    /// One string for sets and storage: node ids, tmux names and Unix user
    /// names cannot hold a `:`.
    pub fn text(&self) -> String {
        format!("{}:{}:{}", self.node, self.user, self.name)
    }
}

pub fn is(s: &Session, k: &Key) -> bool {
    s.node == k.node && s.name == k.name && s.user.as_deref().unwrap_or("") == k.user
}

pub fn find<'a>(sessions: &'a [Session], k: &Key) -> Option<&'a Session> {
    sessions.iter().find(|s| is(s, k))
}

/// CHROTE's group for a name: `main` and `shell` are main; otherwise the
/// part before the first `-`, or the name without trailing digits, or other.
pub fn group(name: &str) -> String {
    if name == "main" || name == "shell" {
        return "main".into();
    }
    if let Some((prefix, _)) = name.split_once('-')
        && !prefix.is_empty()
    {
        return prefix.into();
    }
    let trimmed = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if !trimmed.is_empty() && trimmed != name {
        return trimmed.into();
    }
    "other".into()
}

fn group_rank(g: &str) -> u8 {
    match g {
        "main" => 1,
        "other" => 100,
        _ => 4,
    }
}

/// Names in natural order: a run of digits compares as its number, so
/// `agent-2` comes before `agent-10`; letters compare case aside, then as
/// written.
pub fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    fn parts(s: &str) -> Vec<(bool, &str)> {
        let mut out = vec![];
        let mut start = 0;
        let bytes = s.as_bytes();
        for i in 1..=bytes.len() {
            if i == bytes.len() || bytes[i].is_ascii_digit() != bytes[start].is_ascii_digit() {
                out.push((bytes[start].is_ascii_digit(), &s[start..i]));
                start = i;
            }
        }
        out
    }
    let key = |s: &str| {
        parts(s)
            .into_iter()
            .map(|(digits, p)| match digits {
                true => (
                    0u8,
                    p.trim_start_matches('0').len(),
                    p.trim_start_matches('0').to_string(),
                ),
                false => (1u8, 0, p.to_lowercase()),
            })
            .collect::<Vec<_>>()
    };
    key(a).cmp(&key(b)).then_with(|| a.cmp(b))
}

/// Sessions in groups: main first, other last, the rest by name; within a
/// group by name, in natural order.
pub fn groups<'a>(sessions: &[&'a Session]) -> Vec<(String, Vec<&'a Session>)> {
    let mut list: Vec<&Session> = sessions.to_vec();
    list.sort_by(|a, b| {
        let (ga, gb) = (group(&a.name), group(&b.name));
        (group_rank(&ga), ga)
            .cmp(&(group_rank(&gb), gb))
            .then_with(|| natural(&a.name, &b.name))
            .then_with(|| a.user.cmp(&b.user))
    });
    let mut out: Vec<(String, Vec<&Session>)> = Vec::new();
    for s in list {
        let g = group(&s.name);
        match out.last_mut() {
            Some((name, v)) if *name == g => v.push(s),
            _ => out.push((g, vec![s])),
        }
    }
    out
}

/// Sessions whose name holds every word typed, case aside. Names that start
/// with the first word come first; otherwise the list's order stands.
pub fn search<'a>(sessions: &'a [Session], query: &str) -> Vec<&'a Session> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return sessions.iter().collect();
    }
    let found: Vec<&Session> = sessions
        .iter()
        .filter(|s| {
            let n = s.name.to_lowercase();
            words.iter().all(|w| n.contains(w.as_str()))
        })
        .collect();
    let starts = |s: &&Session| s.name.to_lowercase().starts_with(&words[0]);
    let (mut a, b): (Vec<_>, Vec<_>) = found.into_iter().partition(starts);
    a.extend(b);
    a
}

pub struct Badge {
    /// CHROTE's one-glyph marker, for the rail row.
    pub mark: &'static str,
    pub text: String,
    pub why: String,
}

/// Shells tmux names as a pane's command: a row shows no mark for them.
const SHELLS: [&str; 6] = ["sh", "bash", "dash", "fish", "ksh", "zsh"];

/// What a row shows for the program a session runs, as CHROTE does: a
/// harness mark for `claude` and `codex`, nothing for a shell, else the
/// command's own name.
#[derive(Debug, PartialEq, Eq)]
pub enum Running<'a> {
    Harness(&'static str),
    Command(&'a str),
    Nothing,
}

pub fn running(s: &Session) -> Running<'_> {
    match s.command.as_deref().map(program) {
        None | Some("") => Running::Nothing,
        Some(c) if is_shell(c) => Running::Nothing,
        Some("claude") => Running::Harness("claude-code"),
        Some("codex") => Running::Harness("codex"),
        Some(c) => Running::Command(c),
    }
}

/// A shell, the resting state: a Unix shell, or a Windows node's.
pub fn is_shell(program: &str) -> bool {
    SHELLS.contains(&program) || ["pwsh", "powershell", "cmd"].contains(&program)
}

/// A command's program name: a Windows node reports `powershell.exe` or a
/// whole path, which is `powershell` here.
pub fn program(command: &str) -> &str {
    let name = command.trim().rsplit(['/', '\\']).next().unwrap_or("");
    match name.len().checked_sub(4) {
        Some(i)
            if name
                .get(i..)
                .is_some_and(|x| x.eq_ignore_ascii_case(".exe")) =>
        {
            &name[..i]
        }
        _ => name,
    }
}

/// What makes a session not what one terminal shows (ADR-0017 decision 7),
/// and nothing else: the set must not grow into decoration. The user is
/// drawn as its square, not here.
pub fn badges(s: &Session) -> Vec<Badge> {
    let mut list = Vec::new();
    let mut add =
        |mark: &'static str, text: String, why: String| list.push(Badge { mark, text, why });
    let panes = s.panes.len();
    if panes > 1 {
        add(
            "⊞",
            format!("{panes} panes"),
            format!("{panes} panes; a terminal shows one at a time."),
        );
    }
    let windows = s.windows.unwrap_or(1);
    if windows > 1 {
        add(
            "⊞",
            format!("{windows} windows"),
            format!("{windows} tmux windows; Parvi shows the current one."),
        );
    }
    let foreign = s.foreign.unwrap_or(0);
    if foreign > 0 {
        let text = if foreign == 1 {
            "attached".to_string()
        } else {
            format!("{foreign} attached")
        };
        let clients = if foreign == 1 { "client" } else { "clients" };
        add(
            "◈",
            text,
            format!("{foreign} other {clients} attached, such as an SSH session."),
        );
    }
    if s.pinned == Some(true) {
        add(
            "⊡",
            "pinned".into(),
            "window-size is manual: the window keeps its size whoever views it.".into(),
        );
    }
    if s.mouse == Some(false) {
        add(
            "⊗",
            "mouse off".into(),
            "tmux mouse mode is off: the wheel and clicks go to the program.".into(),
        );
    }
    list
}

/// How many of a node's sessions report tmux's mouse on, of those that
/// report it at all.
pub fn mouse_on(sessions: &[Session]) -> (usize, usize) {
    let told: Vec<bool> = sessions.iter().filter_map(|s| s.mouse).collect();
    (told.iter().filter(|on| **on).count(), told.len())
}

/// What the session's screens ask: the first waiting pane's question, ""
/// if it isn't on screen, or `None` if nothing waits.
pub fn waiting(s: &Session) -> Option<&str> {
    s.panes.iter().find_map(|p| p.waiting.as_deref())
}

/// The sessions in `after` that wait and didn't in `before`.
pub fn started_waiting<'a>(before: &[Session], after: &'a [Session]) -> Vec<&'a Session> {
    let was: std::collections::HashSet<String> = before
        .iter()
        .filter(|s| waiting(s).is_some())
        .map(|s| Key::of(s).text())
        .collect();
    after
        .iter()
        .filter(|s| waiting(s).is_some() && !was.contains(&Key::of(s).text()))
        .collect()
}

/// The session's agent event while it is news: nobody has looked since.
pub fn news(s: &Session) -> Option<&AgentEvent> {
    s.event.as_ref().filter(|e| !e.seen)
}

/// The sessions in `after` whose agent reported since `before`: an unseen
/// event `before` didn't have for that session.
pub fn reported<'a>(before: &[Session], after: &'a [Session]) -> Vec<&'a Session> {
    after
        .iter()
        .filter(|s| {
            news(s).is_some_and(|e| {
                find(before, &Key::of(s))
                    .and_then(|b| b.event.as_ref())
                    .is_none_or(|was| was.at != e.at)
            })
        })
        .collect()
}

/// What a report says, `build finished` or `build needs input`.
pub fn report_title(s: &Session, e: &AgentEvent) -> String {
    match e.kind {
        EventKind::Finished => format!("{} finished", s.name),
        EventKind::NeedsInput => format!("{} needs input", s.name),
    }
}

/// How long ago, compact: `40s`, `5m`, `3h`, `2d`.
pub fn ago(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// A row's activity: `active` while it writes, then how long since.
pub fn activity(s: &Session, now_ms: u64) -> String {
    if s.active == Some(true) {
        return "active".into();
    }
    s.last
        .map(|l| ago(now_ms.saturating_sub(l)))
        .unwrap_or_default()
}

/// Send's targets: with nothing typed, recent targets first (most recent
/// first), then the rest in the list's order; with a query, the search.
pub fn targets<'a>(sessions: &'a [Session], recent: &[String], query: &str) -> Vec<&'a Session> {
    if !query.trim().is_empty() {
        return search(sessions, query);
    }
    let rank = |s: &Session| {
        recent
            .iter()
            .position(|r| *r == Key::of(s).text())
            .unwrap_or(recent.len())
    };
    let mut list: Vec<(usize, &Session)> = sessions.iter().enumerate().collect();
    list.sort_by_key(|(i, s)| (rank(s), *i));
    list.into_iter().map(|(_, s)| s).collect()
}

/// `recent` with `id` moved to the front, at most `limit` long.
pub fn remember(recent: &mut Vec<String>, id: &str, limit: usize) {
    recent.retain(|r| r != id);
    recent.insert(0, id.to_string());
    recent.truncate(limit);
}

/// A name for a narrow row: the head and the tail kept, the middle elided,
/// so `service-controller-agent` reads `service-contro…agent`.
pub fn elide_middle(name: &str, max: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max || max < 4 {
        return name.to_string();
    }
    let tail = max / 4;
    let head = max - 1 - tail;
    let mut s: String = chars[..head].iter().collect();
    s.push('…');
    s.extend(&chars[chars.len() - tail..]);
    s
}

/// A name split for tail-preserving truncation, as CHROTE's
/// `splitSessionName`: the head is everything up to and including the last
/// hyphen, the tail the rest. Names share prefixes (`claude-chrote-…`), so
/// the head is what may be clipped and the tail tells sessions apart.
pub fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('-') {
        Some(i) => name.split_at(i + 1),
        None => ("", name),
    }
}

/// A tag's name for a narrow window: the part after the last hyphen kept
/// whole and the head clipped, as CHROTE's tag does, because names share
/// their prefixes and the tail tells them apart (`forge-existing` reads
/// `fo…existing` in eleven characters).
pub fn elide_head(name: &str, max: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max || max < 2 {
        return name.to_string();
    }
    let cut = chars.iter().rposition(|c| *c == '-').map_or(0, |i| i + 1);
    let tail = chars.len() - cut;
    if tail + 1 >= max {
        let mut s = String::from("…");
        s.extend(&chars[chars.len() - (max - 1)..]);
        return s;
    }
    let mut s: String = chars[..max - 1 - tail].iter().collect();
    s.push('…');
    s.extend(&chars[cut..]);
    s
}

/// The last `n` lines of a screen's text that hold something: a screen
/// whose output sits at the top shows it, not its empty rows.
pub fn screen_tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(0, |i| i + 1);
    lines[end.saturating_sub(n)..end].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvi_terminals_contract::Pane;

    fn s(name: &str) -> Session {
        Session {
            node: "n".into(),
            name: name.into(),
            panes: vec![Pane {
                id: 1,
                cols: 80,
                rows: 24,
                waiting: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn splits_a_name_at_its_last_hyphen() {
        assert_eq!(
            split_name("claude-chrote-architect"),
            ("claude-chrote-", "architect")
        );
        assert_eq!(split_name("build"), ("", "build"));
        assert_eq!(split_name("ends-"), ("ends-", ""));
    }

    #[test]
    fn counts_the_sessions_that_report_the_mouse_on() {
        let with = |mouse| Session { mouse, ..s("x") };
        assert_eq!(mouse_on(&[]), (0, 0));
        assert_eq!(
            mouse_on(&[with(Some(true)), with(None), with(Some(false))]),
            (1, 2)
        );
    }

    #[test]
    fn groups_follow_chrote() {
        assert_eq!(group("claude-srv"), "claude");
        assert_eq!(group("main"), "main");
        assert_eq!(group("build12"), "build");
        assert_eq!(group("solo"), "other");
        let list = [
            s("solo"),
            s("claude-b"),
            s("main"),
            s("claude-a"),
            s("api-x"),
        ];
        let refs: Vec<&Session> = list.iter().collect();
        let g: Vec<(String, Vec<String>)> = groups(&refs)
            .into_iter()
            .map(|(g, v)| (g, v.iter().map(|s| s.name.clone()).collect()))
            .collect();
        let names: Vec<&str> = g.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["main", "api", "claude", "other"]);
        assert_eq!(g[2].1, ["claude-a", "claude-b"]);
    }

    #[test]
    fn names_sort_naturally() {
        let mut names = [
            "agent-10",
            "agent-2",
            "Agent-3",
            "agent-02b",
            "agent",
            "agent-1",
        ];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(
            names,
            [
                "agent",
                "agent-1",
                "agent-2",
                "agent-02b",
                "Agent-3",
                "agent-10"
            ]
        );
        let list = [s("build10"), s("build9")];
        let refs: Vec<&Session> = list.iter().collect();
        let g = groups(&refs);
        let names: Vec<&str> = g[0].1.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["build9", "build10"]);
    }

    #[test]
    fn search_puts_prefix_matches_first() {
        let list = [s("my-build"), s("build-log"), s("clock")];
        let found: Vec<&str> = search(&list, "BUILD")
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(found, ["build-log", "my-build"]);
        assert_eq!(search(&list, "  ").len(), 3);
    }

    #[test]
    fn badges_only_say_what_one_terminal_does_not_show() {
        let mut x = s("a");
        assert!(badges(&x).is_empty());
        x.foreign = Some(1);
        x.mouse = Some(false);
        x.windows = Some(2);
        let b: Vec<String> = badges(&x).into_iter().map(|b| b.text).collect();
        assert_eq!(b, ["2 windows", "attached", "mouse off"]);
    }

    #[test]
    fn a_row_marks_a_harness_names_a_program_and_leaves_a_shell_bare() {
        let mut x = s("a");
        assert_eq!(running(&x), Running::Nothing);
        for (command, shown) in [
            ("claude", Running::Harness("claude-code")),
            ("codex", Running::Harness("codex")),
            ("bash", Running::Nothing),
            ("sleep", Running::Command("sleep")),
            ("powershell.exe", Running::Nothing),
            (r"C:\Tools\htop.EXE", Running::Command("htop")),
        ] {
            x.command = Some(command.into());
            assert_eq!(running(&x), shown);
        }
    }

    #[test]
    fn waiting_is_noticed_once() {
        let before = [s("a"), s("b")];
        let mut after = before.clone();
        after[1].panes[0].waiting = Some("Proceed?".into());
        let started: Vec<&str> = started_waiting(&before, &after)
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(started, ["b"]);
        assert!(started_waiting(&after, &after).is_empty());
    }

    #[test]
    fn a_report_is_told_once_and_not_once_seen() {
        let event = |at, seen| {
            Some(AgentEvent {
                kind: EventKind::Finished,
                at,
                seen,
                summary: String::new(),
            })
        };
        let before = [s("a"), s("b")];
        let mut after = before.clone();
        after[0].event = event(5, false);
        after[1].event = event(6, true);
        let told: Vec<&str> = reported(&before, &after)
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(told, ["a"], "a seen report is not news");
        assert!(reported(&after, &after).is_empty(), "told once");
        let mut again = after.clone();
        again[0].event = event(9, false);
        assert_eq!(reported(&after, &again).len(), 1, "a new report is news");
        assert_eq!(
            report_title(&again[0], again[0].event.as_ref().unwrap()),
            "a finished"
        );
    }

    #[test]
    fn send_ranks_recent_first_and_remembers_five() {
        let list = [s("a"), s("b"), s("c")];
        let recent = vec![Key::of(&list[2]).text()];
        let t: Vec<&str> = targets(&list, &recent, "")
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(t, ["c", "a", "b"]);
        let mut r = vec![];
        for i in 0..7 {
            remember(&mut r, &i.to_string(), 5);
        }
        remember(&mut r, "4", 5);
        assert_eq!(r, ["4", "6", "5", "3", "2"]);
    }

    #[test]
    fn durations_and_elision() {
        assert_eq!(ago(40_000), "40s");
        assert_eq!(ago(300_000), "5m");
        assert_eq!(ago(3 * 3_600_000), "3h");
        assert_eq!(ago(2 * 86_400_000), "2d");
        assert_eq!(
            elide_middle("service-controller-agent", 20),
            "service-contro…agent"
        );
        assert_eq!(elide_middle("short", 20), "short");
    }

    #[test]
    fn a_tag_keeps_the_tail_of_its_name_and_clips_the_head() {
        assert_eq!(elide_head("forge-existing", 20), "forge-existing");
        assert_eq!(elide_head("forge-existing", 11), "fo…existing");
        assert_eq!(elide_head("service-controller-agent", 12), "servic…agent");
        assert_eq!(elide_head("averyveryverylongname", 8), "…ongname");
        assert_eq!(elide_head("forge-existing", 6), "…sting");
    }

    #[test]
    fn a_screen_tail_skips_empty_rows() {
        assert_eq!(screen_tail("$ ls\na\n$\n\n\n", 2), "a\n$");
        assert_eq!(screen_tail("one\ntwo", 5), "one\ntwo");
        assert_eq!(screen_tail("\n \n", 3), "");
    }
}

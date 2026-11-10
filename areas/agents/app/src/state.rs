use parvi_agents_contract::*;
use std::collections::{BTreeMap, HashMap};

/// Gold needs the operator, error is broken, plain is worth knowing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Behind,
    Unlinked,
    Error,
    Plain,
    Ok,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drift {
    /// Where, after the place: `["skills", "ship-issue"]`.
    pub path: Vec<String>,
    pub what: String,
    pub tone: Tone,
}

fn commits(n: u32) -> String {
    format!("{n} commit{}", if n == 1 { "" } else { "s" })
}

fn link<'a>(s: &'a Skill, folder: &str) -> Option<&'a str> {
    s.links.get(folder).map(String::as_str)
}

fn location_behind(s: &Skill, folder: &str) -> u32 {
    s.locations.get(folder).map_or(0, |l| l.behind)
}

/// The skills a folder has, and those it lacks from the checkouts it links
/// into.
pub fn folder_skills<'a>(r: &'a Report, folder: &str) -> Vec<&'a Skill> {
    let present: Vec<&Skill> = r
        .skills
        .iter()
        .filter(|s| link(s, folder).is_some_and(|l| l != "missing"))
        .collect();
    let from: Vec<Option<&str>> = present
        .iter()
        .filter(|s| link(s, folder) == Some("linked"))
        .map(|s| s.locations.get(folder).and_then(|l| l.checkout.as_deref()))
        .collect();
    r.skills
        .iter()
        .filter(|s| {
            present.iter().any(|p| std::ptr::eq(*p, *s))
                || (s.checkout.is_some() && from.contains(&s.checkout.as_deref()))
        })
        .collect()
}

pub fn tool_skills<'a>(r: &'a Report, t: &Tool) -> Vec<&'a Skill> {
    t.skills.as_deref().map_or(vec![], |f| folder_skills(r, f))
}

/// The checkouts a tool's skills come from.
pub fn tool_checkouts<'a>(r: &'a Report, t: &Tool) -> Vec<&'a Checkout> {
    let Some(folder) = t.skills.as_deref() else {
        return vec![];
    };
    let paths: Vec<&str> = tool_skills(r, t)
        .iter()
        .filter_map(|s| s.locations.get(folder).and_then(|l| l.checkout.as_deref()))
        .collect();
    r.checkouts
        .iter()
        .filter(|c| paths.contains(&c.path.as_str()))
        .collect()
}

/// One skill in one folder, as a table cell says it.
pub fn skill_cell(s: &Skill, folder: &str) -> (&'static str, Tone) {
    match link(s, folder) {
        Some("linked") if location_behind(s, folder) > 0 => ("behind", Tone::Behind),
        Some("linked") => ("✓", Tone::Ok),
        Some("broken") => ("broken", Tone::Error),
        Some("copy") => ("copy", Tone::Plain),
        _ => ("—", Tone::Plain),
    }
}

fn upstream_drift(c: &Checkout) -> Vec<Drift> {
    let mut out = vec![];
    let at = vec![c.name.clone()];
    if c.behind > 0 {
        out.push(Drift {
            path: at.clone(),
            what: format!(
                "{} behind {}",
                commits(c.behind),
                c.upstream.as_deref().unwrap_or("upstream")
            ),
            tone: Tone::Behind,
        });
    }
    if c.fetch == "failed" {
        out.push(Drift {
            path: at.clone(),
            what: format!("fetch failed: {}", c.fetch_error.as_deref().unwrap_or(""))
                .trim()
                .to_string(),
            tone: Tone::Error,
        });
    }
    if c.upstream.is_none() {
        out.push(Drift {
            path: at,
            what: "tracks no upstream".into(),
            tone: Tone::Plain,
        });
    }
    out
}

fn skill_drift(r: &Report, skills: &[&Skill], folder: &str) -> Vec<Drift> {
    let name_of = |path: Option<&str>| {
        r.checkouts
            .iter()
            .find(|c| Some(c.path.as_str()) == path)
            .map_or("upstream".to_string(), |c| c.name.clone())
    };
    skills
        .iter()
        .filter_map(|s| {
            let path = vec!["skills".to_string(), s.name.clone()];
            match link(s, folder) {
                Some("linked") => {
                    let behind = location_behind(s, folder);
                    (behind > 0).then(|| Drift {
                        path,
                        what: format!(
                            "{} behind {}",
                            commits(behind),
                            name_of(s.locations.get(folder).and_then(|l| l.checkout.as_deref()))
                        ),
                        tone: Tone::Behind,
                    })
                }
                Some("broken") => Some(Drift {
                    path,
                    what: "link leads nowhere".into(),
                    tone: Tone::Error,
                }),
                Some("copy") => None,
                _ => Some(Drift {
                    path,
                    what: "not linked".into(),
                    tone: Tone::Unlinked,
                }),
            }
        })
        .collect()
}

/// Everything about a tool that needs the operator.
pub fn tool_drift(r: &Report, t: &Tool) -> Vec<Drift> {
    if t.path.is_none() {
        return vec![Drift {
            path: vec!["cli".into()],
            what: "not installed".into(),
            tone: Tone::Plain,
        }];
    }
    let mut out = vec![];
    if t.signed_in == Some(false) {
        out.push(Drift {
            path: vec![t.command.clone()],
            what: "not signed in".into(),
            tone: Tone::Behind,
        });
    }
    for c in tool_checkouts(r, t) {
        out.extend(upstream_drift(c));
    }
    if let Some(folder) = &t.skills {
        out.extend(skill_drift(r, &tool_skills(r, t), folder));
    }
    for s in t.settings.iter().filter(|s| !s.present) {
        out.push(Drift {
            path: vec![s.path.clone()],
            what: "missing".into(),
            tone: Tone::Plain,
        });
    }
    out
}

/// The skills a checkout gives, in any folder.
pub fn checkout_skills<'a>(r: &'a Report, c: &Checkout) -> Vec<&'a Skill> {
    r.skills
        .iter()
        .filter(|s| {
            s.checkout.as_deref() == Some(c.path.as_str())
                || s.locations
                    .values()
                    .any(|l| l.checkout.as_deref() == Some(c.path.as_str()))
        })
        .collect()
}

/// Everything about a checkout that needs the operator, in every folder.
pub fn checkout_drift(r: &Report, c: &Checkout) -> Vec<Drift> {
    let skills = checkout_skills(r, c);
    let mut out = upstream_drift(c);
    for f in &r.folders {
        for mut d in skill_drift(r, &skills, &f.id) {
            d.path[0] = f.path.clone();
            out.push(d);
        }
    }
    out
}

/// A tool's one-word state, as the rail and the versions table say it.
pub fn tool_state(r: &Report, t: &Tool) -> (String, Tone) {
    if t.path.is_none() {
        return ("not installed".into(), Tone::Plain);
    }
    if t.signed_in == Some(false) {
        return ("not signed in".into(), Tone::Behind);
    }
    let drift = tool_drift(r, t);
    if drift.iter().any(|d| d.tone == Tone::Error) {
        return ("broken".into(), Tone::Error);
    }
    let unlinked = drift.iter().filter(|d| d.tone == Tone::Unlinked).count();
    if unlinked > 0 {
        return (format!("{unlinked} not linked"), Tone::Unlinked);
    }
    let behind = drift.iter().filter(|d| d.tone == Tone::Behind).count();
    if behind > 0 {
        return (format!("{behind} behind"), Tone::Behind);
    }
    match t.signed_in {
        None => ("sign-in unknown".into(), Tone::Plain),
        Some(_) => ("current".into(), Tone::Ok),
    }
}

/// The checkout's distance from upstream, in words.
pub fn upstream_words(c: &Checkout) -> String {
    let Some(up) = &c.upstream else {
        return "no upstream".into();
    };
    let mut counts = vec![];
    if c.behind > 0 {
        counts.push(format!("{} behind", c.behind));
    }
    if c.ahead > 0 {
        counts.push(format!("{} ahead", c.ahead));
    }
    let at = if counts.is_empty() {
        "level with".to_string()
    } else {
        counts.join(", ")
    };
    let when = match c.fetch.as_str() {
        "fetching" => "fetching…",
        "failed" => "fetch failed",
        _ => "just fetched",
    };
    format!("{at} {up} · {when}")
}

/// Two capitals, as Archon's rail draws agents.
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter(|w| !w.is_empty())
        .collect();
    let letters: String = if words.len() > 1 {
        words[..2].iter().filter_map(|w| w.chars().next()).collect()
    } else {
        name.chars().take(2).collect()
    };
    letters.to_uppercase()
}

/// What the rail has chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chosen {
    Tool(String),
    /// A checkout by name: the same checkout on every device.
    Checkout(String),
}

/// What a sync frame answered.
#[derive(Clone, Debug, PartialEq)]
pub enum Synced {
    Started { checkout: String, session: String },
    Refused { checkout: String, why: String },
}

pub struct Agents {
    /// Each node's last report; it stays as last known when the node goes.
    pub reports: BTreeMap<String, Report>,
    pub chosen: Chosen,
    /// Asked syncs by id: (node, checkout path).
    syncs: HashMap<u32, (String, String)>,
    next: u32,
}

impl Default for Agents {
    fn default() -> Self {
        Self {
            reports: BTreeMap::new(),
            chosen: Chosen::Tool("claude".into()),
            syncs: HashMap::new(),
            next: 1,
        }
    }
}

impl Agents {
    /// A frame of the channel from `node`. Returns a sync's outcome to say.
    pub fn frame(&mut self, node: &str, kind: u8, body: &[u8]) -> Option<Option<Synced>> {
        match kind {
            REPORT => {
                let r = serde_json::from_slice::<Report>(body).ok()?;
                self.reports.insert(node.to_string(), r);
                Some(None)
            }
            SYNCED if body.len() >= 5 => {
                let id = u32::from_le_bytes(body[..4].try_into().ok()?);
                let ok = body[4] == 1;
                let text = String::from_utf8_lossy(&body[5..]).to_string();
                let (_, checkout) = self.syncs.remove(&id)?;
                Some(Some(if ok {
                    Synced::Started {
                        checkout,
                        session: text,
                    }
                } else {
                    Synced::Refused {
                        checkout,
                        why: serde_json::from_str::<parvi_core::Error>(&text)
                            .ok()?
                            .message,
                    }
                }))
            }
            _ => None,
        }
    }

    /// A sync of `checkout` on `node`: the frame to send.
    pub fn sync(&mut self, node: &str, checkout: &str) -> Vec<u8> {
        let id = self.next;
        self.next += 1;
        self.syncs
            .insert(id, (node.to_string(), checkout.to_string()));
        let mut f = vec![SYNC];
        f.extend_from_slice(&id.to_le_bytes());
        f.extend_from_slice(checkout.as_bytes());
        f
    }

    /// Is a sync of this checkout on this node on its way?
    pub fn starting(&self, node: &str, checkout: &str) -> bool {
        self.syncs.values().any(|(n, c)| n == node && c == checkout)
    }

    /// A node's link dropped: its asked syncs' outcomes are unknown.
    pub fn dropped(&mut self, node: &str) -> Vec<String> {
        let gone: Vec<u32> = self
            .syncs
            .iter()
            .filter(|(_, (n, _))| n == node)
            .map(|(id, _)| *id)
            .collect();
        gone.into_iter()
            .filter_map(|id| self.syncs.remove(&id).map(|(_, c)| c))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Report {
        serde_json::from_str(
            r#"{"home":"~",
 "folders":[{"id":"claude","path":"~/.claude/skills"},{"id":"agents","path":"~/.agents/skills"}],
 "tools":[{"id":"claude","name":"Claude Code","command":"claude","path":"~/.local/bin/claude",
           "version":"2.1.318","skills":"claude","signedIn":true,"credentialsPresent":true,
           "settings":[{"path":"~/.claude/settings.json","present":true}]},
          {"id":"gh","name":"GitHub CLI","command":"gh","signedIn":null,"settings":[]}],
 "checkouts":[{"name":"agent-kit","path":"~/agent-kit","branch":"main","commit":"1a2b3c4",
               "subject":"s","upstream":"origin/main","setup":"install.sh",
               "fetch":"fetched","ahead":0,"behind":2,"dirty":false}],
 "skills":[{"name":"handoff","source":"~/agent-kit/skills/handoff","checkout":"~/agent-kit","behind":1,
            "links":{"claude":"linked","agents":"missing"},
            "locations":{"claude":{"state":"linked","checkout":"~/agent-kit","behind":1}}},
           {"name":"lost","checkout":"~/agent-kit","behind":0,
            "links":{"claude":"broken"},"locations":{"claude":{"state":"broken","behind":0}}}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn reads_a_tools_drift_as_the_node_ui_does() {
        let r = report();
        let claude = &r.tools[0];
        let drift: Vec<String> = tool_drift(&r, claude)
            .iter()
            .map(|d| format!("{} {}", d.path.join(" › "), d.what))
            .collect();
        assert_eq!(
            drift,
            vec![
                "agent-kit 2 commits behind origin/main",
                "skills › handoff 1 commit behind agent-kit",
                "skills › lost link leads nowhere",
            ]
        );
        assert_eq!(tool_state(&r, claude), ("broken".into(), Tone::Error));
        assert_eq!(
            tool_state(&r, &r.tools[1]),
            ("not installed".into(), Tone::Plain)
        );
    }

    #[test]
    fn reads_a_checkouts_drift_in_every_folder() {
        let r = report();
        let words: Vec<String> = checkout_drift(&r, &r.checkouts[0])
            .iter()
            .map(|d| format!("{} {}", d.path.join(" › "), d.what))
            .collect();
        assert_eq!(
            words,
            vec![
                "agent-kit 2 commits behind origin/main",
                "~/.claude/skills › handoff 1 commit behind agent-kit",
                "~/.claude/skills › lost link leads nowhere",
                "~/.agents/skills › handoff not linked",
                "~/.agents/skills › lost not linked",
            ]
        );
        assert_eq!(
            upstream_words(&r.checkouts[0]),
            "2 behind origin/main · just fetched"
        );
        assert_eq!(skill_cell(&r.skills[0], "claude"), ("behind", Tone::Behind));
        assert_eq!(initials("Claude Code"), "CC");
    }

    #[test]
    fn missing_links_are_not_reported_as_behind() {
        let mut r = report();
        r.checkouts[0].behind = 0;
        r.skills.truncate(1);
        r.skills[0].links.insert("claude".into(), "missing".into());
        // Another linked skill establishes which checkout this folder uses.
        let mut linked = r.skills[0].clone();
        linked.name = "other".into();
        linked.links.insert("claude".into(), "linked".into());
        linked.locations.get_mut("claude").unwrap().behind = 0;
        r.skills.push(linked);
        assert_eq!(
            tool_state(&r, &r.tools[0]),
            ("1 not linked".into(), Tone::Unlinked)
        );
        assert_eq!(
            checkout_drift(&r, &r.checkouts[0])
                .iter()
                .filter(|d| d.tone == Tone::Unlinked)
                .count(),
            3
        );
    }

    #[test]
    fn a_sync_answer_names_its_checkout_and_a_drop_settles_it() {
        let mut a = Agents::default();
        let f = a.sync("n1", "~/agent-kit");
        assert_eq!(f[0], SYNC);
        assert!(a.starting("n1", "~/agent-kit"));
        let mut body = f[1..5].to_vec();
        body.push(1);
        body.extend_from_slice(b"sync-agent-kit");
        assert_eq!(
            a.frame("n1", SYNCED, &body),
            Some(Some(Synced::Started {
                checkout: "~/agent-kit".into(),
                session: "sync-agent-kit".into()
            }))
        );
        a.sync("n1", "~/x");
        assert_eq!(a.dropped("n1"), vec!["~/x".to_string()]);
        assert!(!a.starting("n1", "~/x"));
    }
}

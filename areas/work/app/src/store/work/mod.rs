//! The Work tab's rules (CHROTE's Beads view, `beadsSlice.ts`, applied to
//! both trackers): one table of every item across the node's projects,
//! sliced by search, project, status and days, sorted by any column, with
//! the links between items across stores and the selected item's family as
//! a graph. Plain data, tested without a window; the snapshot comes from
//! `proto::work`.

pub mod column;
pub mod days;
pub mod graph;
pub mod links;
pub mod prose;
mod slice;
pub mod when;

#[cfg(test)]
pub mod tests;

pub use days::Days;
pub use links::Links;
pub use slice::{COLUMNS, Column, Slice, Sliced};

use parvi_work_contract::{Family, ProjectView as Project, Snapshot};
use std::collections::HashMap;

/// The status partition, in the order the rail lists it and a sort ranks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    Open,
    InProgress,
    Blocked,
    Deferred,
    Closed,
}

pub const STATUSES: [Status; 5] = [
    Status::Open,
    Status::InProgress,
    Status::Blocked,
    Status::Deferred,
    Status::Closed,
];

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::InProgress => "in progress",
            Status::Blocked => "blocked",
            Status::Deferred => "deferred",
            Status::Closed => "closed",
        }
    }

    /// CHROTE's status glyphs (`beadStatus.ts`).
    pub fn glyph(self) -> &'static str {
        match self {
            Status::Open => "○",
            Status::InProgress => "◐",
            Status::Blocked => "●",
            Status::Deferred => "❄",
            Status::Closed => "✓",
        }
    }

    /// A tracker's own status, partitioned. A custom Beads status reads as
    /// open; GitHub's draft as open and merged as closed.
    fn of(raw: &str) -> Status {
        match raw {
            "in_progress" => Status::InProgress,
            "blocked" => Status::Blocked,
            "deferred" => Status::Deferred,
            "closed" | "merged" => Status::Closed,
            _ => Status::Open,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tracker {
    Beads,
    Github,
}

/// One configured project as last read, and how that went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectInfo {
    pub tracker: Tracker,
    /// What the column and the rail show: a store's prefix (with its folder
    /// when two share it), a repository's `owner/name`.
    pub label: String,
    /// A store's folder; a repository's name.
    pub path: String,
    pub checkout: Option<String>,
    pub url: Option<String>,
    /// Milliseconds since the epoch of the last read that worked.
    pub synced: Option<u64>,
    pub error: Option<String>,
    /// Why the node cannot see the store change.
    pub unwatched: Option<String>,
    /// A read has worked: its rows are in the table.
    pub read: bool,
    pub total: usize,
}

/// A pull request's own facts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullFacts {
    pub head: String,
    pub base: String,
    pub additions: u32,
    pub deletions: u32,
    pub closes: Vec<u32>,
    pub checks: Option<parvi_work_contract::Checks>,
}

/// One row of the table: a Bead, an issue or a pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub project: usize,
    /// The canonical reference (docs/work.md, References).
    pub reference: String,
    /// `ctx-12`, or `#12` for GitHub.
    pub id: String,
    pub number: Option<u32>,
    /// A Bead's type; `issue` or `pull` for GitHub.
    pub kind: String,
    pub title: String,
    /// The tracker's own words.
    pub raw_status: String,
    pub status: Status,
    pub priority: Option<u8>,
    pub url: Option<String>,
    pub author: Option<String>,
    pub assignees: Vec<String>,
    pub labels: Vec<String>,
    pub created: Option<u64>,
    pub updated: Option<u64>,
    pub closed: Option<u64>,
    pub body: String,
    /// Characters of the body: how much there is to read (CHROTE's Size).
    pub size: usize,
    pub family: Family,
    /// An issue's pull requests, by number.
    pub pulls: Vec<u32>,
    pub pull: Option<PullFacts>,
    haystack: String,
}

impl Row {
    /// The id as written to be found again: a Bead's own, an issue's with
    /// its repository (`o/r#12`), as Copy id puts it on the clipboard.
    pub fn written_id(&self, board: &Board) -> String {
        match board.projects[self.project].tracker {
            Tracker::Beads => self.id.clone(),
            Tracker::Github => format!("{}{}", board.projects[self.project].path, self.id),
        }
    }

    /// The line that hands the item to an agent (docs/work.md, Name).
    pub fn line(&self, board: &Board) -> String {
        let p = &board.projects[self.project];
        match p.tracker {
            Tracker::Beads => {
                // The receiver runs bd in the folder; the id alone would not
                // say which store when another has its prefix or holds it.
                let elsewhere = board.holders(&self.id).len() > 1;
                if board.shared_prefix(self.project) || elsewhere {
                    format!("bead {} ({}): {}", self.id, p.path, self.title)
                } else {
                    format!("bead {}: {}", self.id, self.title)
                }
            }
            Tracker::Github => {
                let what = if self.pull.is_some() { "pull" } else { "issue" };
                format!("{what} {}{}: {}", p.path, self.id, self.title)
            }
        }
    }
}

/// Every project and every row, as the node last said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Board {
    pub projects: Vec<ProjectInfo>,
    pub rows: Vec<Row>,
    /// The links between rows, across stores.
    pub links: Links,
    /// Each row's days, for the strip and the Idle column.
    pub days: Days,
    /// Row index by reference, and by bare Bead id where it is unique.
    by_ref: HashMap<String, usize>,
    /// Bead rows by (project, id), and every Bead row holding an id.
    by_store: HashMap<(usize, String), usize>,
    by_id: HashMap<String, Vec<usize>>,
}

fn prefix(id: &str) -> &str {
    id.rsplit_once('-').map_or(id, |(p, _)| p)
}

fn base_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}

/// The prefix most of a store's ids carry: a Bead moved in from another
/// store keeps its own.
fn common_prefix(items: &[parvi_work_contract::Item]) -> Option<String> {
    let mut n: HashMap<&str, usize> = HashMap::new();
    for i in items {
        *n.entry(prefix(&i.id)).or_default() += 1;
    }
    n.into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0)))
        .map(|(p, _)| p.to_string())
}

fn haystack(parts: &[&str]) -> String {
    parts.join("\n").to_lowercase()
}

impl Board {
    /// The board of a snapshot, its days counted at `now` (ms) in the zone
    /// `offset` gives (seconds east of UTC at an instant).
    pub fn from_snapshot(s: &Snapshot, now: u64, offset: &dyn Fn(u64) -> i64) -> Board {
        let mut b = Board::default();
        for (pi, p) in s.projects.iter().enumerate() {
            match p {
                Project::Beads(st) => {
                    let items = st.items.clone().unwrap_or_default();
                    let label =
                        common_prefix(&items).unwrap_or_else(|| base_name(&st.folder).to_string());
                    b.projects.push(ProjectInfo {
                        tracker: Tracker::Beads,
                        label,
                        path: st.folder.clone(),
                        checkout: Some(st.folder.clone()),
                        url: None,
                        synced: st.synced,
                        error: st.error.clone(),
                        unwatched: st.unwatched.clone(),
                        read: st.items.is_some(),
                        total: items.len(),
                    });
                    for i in items {
                        b.rows.push(Row {
                            project: pi,
                            size: 0,
                            haystack: haystack(&[&i.id, &i.title, &i.body, &i.labels.join(" ")]),
                            reference: i.reference,
                            id: i.id,
                            number: None,
                            kind: i.kind.unwrap_or_else(|| "task".into()),
                            title: i.title,
                            status: Status::of(&i.status),
                            raw_status: i.status,
                            priority: i.priority,
                            url: i.url,
                            author: i.author,
                            assignees: i.assignees,
                            labels: i.labels,
                            created: when::parse(&i.created),
                            updated: when::parse(&i.updated),
                            closed: i.closed.as_deref().and_then(when::parse),
                            body: i.body,
                            family: i.family,
                            pulls: vec![],
                            pull: None,
                        });
                    }
                }
                Project::Github(r) => {
                    let issues = r
                        .work
                        .as_ref()
                        .map(|w| w.issues.clone())
                        .unwrap_or_default();
                    let pulls = r.work.as_ref().map(|w| w.pulls.clone()).unwrap_or_default();
                    b.projects.push(ProjectInfo {
                        tracker: Tracker::Github,
                        label: r.name.clone(),
                        path: r.name.clone(),
                        checkout: r.checkout.clone(),
                        url: r.work.as_ref().map(|w| w.url.clone()),
                        synced: r.synced,
                        error: r.error.clone(),
                        unwatched: None,
                        read: r.work.is_some(),
                        total: issues.len() + pulls.len(),
                    });
                    let reference = |n: u32| format!("gh:{}#{n}", r.name);
                    for i in issues {
                        let n = i.number;
                        b.rows.push(Row {
                            project: pi,
                            size: 0,
                            haystack: haystack(&[
                                &format!("#{n}"),
                                &i.title,
                                &i.body,
                                &i.labels.join(" "),
                            ]),
                            reference: reference(n),
                            id: format!("#{n}"),
                            number: Some(n),
                            kind: "issue".into(),
                            status: Status::of(match i.state {
                                parvi_work_contract::IssueState::Open => "open",
                                parvi_work_contract::IssueState::Closed => "closed",
                            }),
                            raw_status: match i.state {
                                parvi_work_contract::IssueState::Open => "open",
                                parvi_work_contract::IssueState::Closed => "closed",
                            }
                            .into(),
                            title: i.title,
                            priority: None,
                            url: Some(i.url),
                            author: i.author,
                            assignees: i.assignees,
                            labels: i.labels,
                            created: when::parse(&i.created),
                            updated: when::parse(&i.updated),
                            closed: None,
                            body: i.body,
                            family: Family {
                                pulls: i.pulls.iter().map(|p| reference(*p)).collect(),
                                ..Family::default()
                            },
                            pulls: i.pulls,
                            pull: None,
                        });
                    }
                    for p in pulls {
                        let n = p.number;
                        b.rows.push(Row {
                            project: pi,
                            size: 0,
                            haystack: haystack(&[
                                &format!("#{n}"),
                                &p.title,
                                &p.body,
                                &p.labels.join(" "),
                            ]),
                            reference: reference(n),
                            id: format!("#{n}"),
                            number: Some(n),
                            kind: "pull".into(),
                            status: Status::of(match p.state {
                                parvi_work_contract::PullState::Open => "open",
                                parvi_work_contract::PullState::Draft => "draft",
                                parvi_work_contract::PullState::Merged => "merged",
                                parvi_work_contract::PullState::Closed => "closed",
                            }),
                            raw_status: match p.state {
                                parvi_work_contract::PullState::Open => "open",
                                parvi_work_contract::PullState::Draft => "draft",
                                parvi_work_contract::PullState::Merged => "merged",
                                parvi_work_contract::PullState::Closed => "closed",
                            }
                            .into(),
                            title: p.title,
                            priority: None,
                            url: Some(p.url),
                            author: p.author,
                            assignees: p.assignees,
                            labels: p.labels,
                            created: when::parse(&p.created),
                            updated: when::parse(&p.updated),
                            closed: None,
                            body: p.body,
                            family: Family {
                                blocking: p.closes.iter().map(|c| reference(*c)).collect(),
                                ..Family::default()
                            },
                            pulls: vec![],
                            pull: Some(PullFacts {
                                head: p.head,
                                base: p.base,
                                additions: p.additions,
                                deletions: p.deletions,
                                closes: p.closes,
                                checks: p.checks,
                            }),
                        });
                    }
                }
            }
        }
        for r in &mut b.rows {
            r.size = r.body.chars().count();
        }
        b.name_shared_prefixes();
        b.index();
        b.links = Links::of(&b);
        b.block();
        b.days = Days::of(&b, now, offset);
        b
    }

    /// Build a card from the on-demand item and the node's project list.
    /// The on-demand item wins over a possibly older list, while the list
    /// supplies the other rows that its family references name.
    pub fn from_item(item: parvi_work_contract::Item, projects: &[Project], now: u64) -> Board {
        let reference = item.reference.clone();
        let github = reference
            .strip_prefix("gh:")
            .and_then(|r| r.rsplit_once('#'))
            .map(|(r, _)| r.to_owned());
        let folder = reference
            .strip_prefix("bead:")
            .and_then(|r| r.split_once(':'))
            .and_then(|(_, r)| r.rsplit_once('#'))
            .map(|(f, _)| f.to_owned())
            .unwrap_or_default();
        let snapshot = Snapshot {
            projects: vec![Project::Beads(parvi_work_contract::StoreView {
                folder,
                items: Some(vec![item]),
                ..Default::default()
            })],
        };
        let mut board = Board::from_snapshot(&snapshot, now, &when::local_offset);
        if let Some(repo) = github {
            let project = &mut board.projects[0];
            project.tracker = Tracker::Github;
            project.label = repo.clone();
            project.path = repo.clone();
            project.checkout = projects.iter().find_map(|p| match p {
                Project::Github(r) if r.name.eq_ignore_ascii_case(&repo) => r.checkout.clone(),
                _ => None,
            });
            project.url = Some(format!("https://github.com/{repo}"));
            let row = &mut board.rows[0];
            row.id = format!("#{}", row.id.trim_start_matches('#'));
            row.kind = "issue".into();
            board.index();
        }
        let listed = Board::from_snapshot(
            &Snapshot {
                projects: projects.to_vec(),
            },
            now,
            &when::local_offset,
        );
        let mut project_map = Vec::with_capacity(listed.projects.len());
        for project in listed.projects {
            if project.tracker == board.projects[0].tracker
                && project.path == board.projects[0].path
            {
                project_map.push(0);
            } else {
                project_map.push(board.projects.len());
                board.projects.push(project);
            }
        }
        for mut row in listed.rows {
            if row.reference != reference || row.pull.is_some() {
                row.project = project_map[row.project];
                board.rows.push(row);
            }
        }
        board.name_shared_prefixes();
        board.index();
        board.links = Links::of(&board);
        // The one-item board provisionally marked unknown blockers as
        // unfinished. Recompute from the trackers once their rows arrive.
        for row in &mut board.rows {
            row.status = Status::of(&row.raw_status);
        }
        board.block();
        board.days = Days::of(&board, now, &when::local_offset);
        board
    }

    /// Two stores with one prefix read as `prefix (folder)`.
    fn name_shared_prefixes(&mut self) {
        let mut seen: HashMap<String, usize> = HashMap::new();
        for p in &self.projects {
            if p.tracker == Tracker::Beads {
                *seen.entry(p.label.clone()).or_default() += 1;
            }
        }
        for p in &mut self.projects {
            if p.tracker == Tracker::Beads && seen[&p.label] > 1 {
                p.label = format!("{} ({})", p.label, base_name(&p.path));
            }
        }
    }

    fn shared_prefix(&self, project: usize) -> bool {
        self.projects[project].label.ends_with(')')
    }

    fn index(&mut self) {
        let mut bare: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, r) in self.rows.iter().enumerate() {
            // An issue and a pull request share GitHub's numbers; the issue
            // keeps the reference, the pull is found by its own.
            self.by_ref.entry(r.reference.clone()).or_insert(i);
            if self.projects[r.project].tracker == Tracker::Beads {
                bare.entry(r.id.clone()).or_default().push(i);
                self.by_store.insert((r.project, r.id.clone()), i);
            }
        }
        self.by_id = bare.clone();
        for (id, rows) in bare {
            if rows.len() == 1 {
                self.by_ref.entry(id).or_insert(rows[0]);
            }
        }
    }

    /// A Bead not finished or under way is blocked while any blocker, in
    /// any store, is unfinished; a blocker no store holds is unfinished
    /// (CHROTE's status partition). An issue is never blocked by its pull
    /// requests.
    fn block(&mut self) {
        let blocked: Vec<usize> = (0..self.rows.len())
            .filter(|&i| {
                let r = &self.rows[i];
                self.projects[r.project].tracker == Tracker::Beads
                    && matches!(r.status, Status::Open | Status::Deferred)
                    && (!self.links.unresolved[i].is_empty()
                        || self.links.blockers[i]
                            .iter()
                            .any(|&j| self.rows[j].status != Status::Closed))
            })
            .collect();
        for i in blocked {
            self.rows[i].status = Status::Blocked;
        }
    }

    /// The row a Bead id names from `project`: its own store's first, else
    /// the one row anywhere that holds it.
    pub fn find_from(&self, id: &str, project: usize) -> Option<usize> {
        if let Some(&i) = self.by_store.get(&(project, id.to_string())) {
            return Some(i);
        }
        match self.holders(id) {
            [one] => Some(*one),
            _ => None,
        }
    }

    /// Every Bead row holding `id`, in any store.
    pub fn holders(&self, id: &str) -> &[usize] {
        self.by_id.get(id).map_or(&[], Vec::as_slice)
    }

    /// The row a family reference names, if the node read it.
    pub fn find(&self, reference: &str) -> Option<usize> {
        self.by_ref.get(reference).copied()
    }

    /// A pull request of `project` by its number.
    pub fn pull(&self, project: usize, number: u32) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.project == project && r.number == Some(number) && r.pull.is_some())
    }

    /// The row with this key (reference, and pull or not).
    pub fn position(&self, key: &RowKey) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.reference == key.reference && r.pull.is_some() == key.pull)
    }
}

/// A size as the table reads it: `940`, `6.4k`, `12k`.
pub fn size_label(n: usize) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..9950 => format!("{:.1}k", n as f64 / 1000.),
        _ => format!("{:.0}k", n as f64 / 1000.),
    }
}

/// What a selection holds on to across snapshots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowKey {
    pub reference: String,
    pub pull: bool,
}

impl RowKey {
    pub fn of(r: &Row) -> RowKey {
        RowKey {
            reference: r.reference.clone(),
            pull: r.pull.is_some(),
        }
    }
}

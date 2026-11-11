//! One environment's agent setup, gathered and written as the report frame's
//! JSON (`docs/protocol.md`, Agents).

use std::path::PathBuf;

use super::checkout::{self, Checkout, Fetch};
use super::skills::{self, FOLDERS, Skill};
use super::tools::{self, Found, TOOLS};
use super::{Home, run};
use parvi_agents_contract as wire;

pub struct Report {
    pub tools: Vec<Found>,
    pub skills: Vec<Skill>,
    /// The checkouts the skills link into.
    pub checkouts: Vec<Checkout>,
}

/// Gathers the report as of each checkout's last fetch.
pub async fn gather(home: &Home, fetch: Fetch) -> Report {
    let tools = all(TOOLS.iter().map(|tool| {
        let home = home.clone();
        async move { tools::find(&home, tool).await }
    }))
    .await;
    let mut report = Report {
        tools,
        skills: skills::scan(home),
        checkouts: Vec::new(),
    };
    report.read_checkouts(home, |_| fetch.clone()).await;
    report
}

/// The distinct checkouts the skills come from.
pub fn checkout_paths(skills: &[Skill]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = skills
        .iter()
        .flat_map(|s| s.checkouts.iter().flatten().cloned())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

impl Report {
    pub fn checkout_paths(&self) -> Vec<PathBuf> {
        checkout_paths(&self.skills)
    }

    /// Reads every checkout and how far behind each skill is, again.
    pub async fn read_checkouts(&mut self, home: &Home, fetch: impl Fn(&PathBuf) -> Fetch) {
        let paths = self.checkout_paths();
        self.checkouts = all(paths.iter().map(|path| {
            let (home, path, fetch) = (home.clone(), path.clone(), fetch(path));
            async move { checkout::read(&home, &path, fetch).await }
        }))
        .await;
        let behind: Vec<&Checkout> = self.checkouts.iter().filter(|c| c.behind > 0).collect();
        for skill in &mut self.skills {
            for i in 0..skill.links.len() {
                skill.behind_by_folder[i] = 0;
                if let (Some(at), Some(source)) = (&skill.checkouts[i], &skill.sources[i])
                    && let Some(checkout) = behind.iter().find(|c| &c.path == at)
                    && let Ok(relative) = source.strip_prefix(&checkout.path)
                {
                    skill.behind_by_folder[i] =
                        checkout::behind_in(home, &checkout.path, relative).await;
                }
            }
            skill.behind = 0;
            let (Some(at), Some(source)) = (&skill.checkout, &skill.source) else {
                continue;
            };
            if let Some(checkout) = behind.iter().find(|c| &c.path == at)
                && let Ok(relative) = source.strip_prefix(&checkout.path)
            {
                skill.behind = checkout::behind_in(home, &checkout.path, relative).await;
            }
        }
    }

    /// Fetches every checkout, then reads them again.
    pub async fn fetch(&mut self, home: &Home) {
        let paths = self.checkout_paths();
        let results = all(paths.iter().map(|path| {
            let (home, path) = (home.clone(), path.clone());
            async move { run::fetch(&home, &path).await }
        }))
        .await;
        self.read_checkouts(home, |path| {
            let at = paths.iter().position(|p| p == path);
            match at.map(|i| &results[i]) {
                Some(Err(why)) => Fetch::Failed(why.clone()),
                _ => Fetch::Fetched,
            }
        })
        .await;
    }

    /// Converts local paths and probe results into the shared wire contract.
    pub fn wire(&self, home: &Home) -> wire::Report {
        wire::Report {
            home: home.show(&home.dir),
            folders: FOLDERS
                .iter()
                .map(|(id, path)| wire::Folder {
                    id: (*id).into(),
                    path: format!("~/{path}"),
                })
                .collect(),
            tools: self
                .tools
                .iter()
                .map(|found| wire::Tool {
                    id: found.tool.id.into(),
                    name: found.tool.name.into(),
                    command: found.tool.command.into(),
                    path: found.path.as_deref().map(|p| home.show(p)),
                    version: found.version.clone(),
                    skills: found.tool.skills.map(str::to_owned),
                    signed_in: None,
                    credentials_present: found.credentials_present,
                    settings: found
                        .settings
                        .iter()
                        .map(|(path, present)| wire::Setting {
                            path: format!("~/{path}"),
                            present: *present,
                        })
                        .collect(),
                })
                .collect(),
            checkouts: self
                .checkouts
                .iter()
                .map(|c| {
                    let (fetch, fetch_error) = match &c.fetch {
                        Fetch::Fetching => ("fetching", None),
                        Fetch::Fetched => ("fetched", None),
                        Fetch::Failed(why) => ("failed", Some(why.clone())),
                    };
                    wire::Checkout {
                        name: c.name(),
                        path: home.show(&c.path),
                        branch: c.branch.clone(),
                        commit: c.commit.clone(),
                        subject: c.subject.clone(),
                        upstream: c.upstream.clone(),
                        setup: c.setup.map(str::to_owned),
                        fetch: fetch.into(),
                        fetch_error,
                        ahead: c.ahead,
                        behind: c.behind,
                        dirty: c.dirty,
                    }
                })
                .collect(),
            skills: self
                .skills
                .iter()
                .map(|s| wire::Skill {
                    name: s.name.clone(),
                    description: (!s.description.is_empty()).then(|| s.description.clone()),
                    source: s.source.as_deref().map(|p| home.show(p)),
                    checkout: s.checkout.as_deref().map(|p| home.show(p)),
                    behind: s.behind,
                    links: FOLDERS
                        .iter()
                        .zip(&s.links)
                        .map(|((id, _), link)| ((*id).into(), link.word().into()))
                        .collect(),
                    locations: FOLDERS
                        .iter()
                        .enumerate()
                        .map(|(i, (id, _))| {
                            (
                                (*id).into(),
                                wire::Location {
                                    state: s.links[i].word().into(),
                                    source: s.sources[i].as_deref().map(|p| home.show(p)),
                                    checkout: s.checkouts[i].as_deref().map(|p| home.show(p)),
                                    behind: s.behind_by_folder[i],
                                },
                            )
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    pub fn json(&self, home: &Home) -> String {
        serde_json::to_string(&self.wire(home)).expect("report serializes")
    }
}

/// Runs the futures side by side and returns their outputs in order.
async fn all<T: Send + 'static>(
    futures: impl IntoIterator<Item = impl Future<Output = T> + Send + 'static>,
) -> Vec<T> {
    let mut set = tokio::task::JoinSet::new();
    let mut count = 0;
    for (i, future) in futures.into_iter().enumerate() {
        set.spawn(async move { (i, future.await) });
        count += 1;
    }
    let mut out: Vec<Option<T>> = (0..count).map(|_| None).collect();
    while let Some(joined) = set.join_next().await {
        let (i, value) = joined.expect("a probe panicked");
        out[i] = Some(value);
    }
    out.into_iter()
        .map(|v| v.expect("every probe ran"))
        .collect()
}

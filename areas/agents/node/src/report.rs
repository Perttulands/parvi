//! One environment's agent setup, gathered and written as the report frame's
//! JSON (`docs/protocol.md`, Agents).

use std::path::PathBuf;

use super::checkout::{self, Checkout, Fetch};
use super::skills::{self, FOLDERS, Skill};
use super::tools::{self, Found, TOOLS};
use super::{Home, run};
fn push_json_string(out: &mut String, value: &str) {
    out.push_str(&serde_json::to_string(value).expect("string serializes"));
}

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

    pub fn json(&self, home: &Home) -> String {
        let mut json = String::from("{\"home\":");
        push_json_string(&mut json, &home.show(&home.dir));
        json.push_str(",\"folders\":[");
        for (i, (id, relative)) in FOLDERS.iter().enumerate() {
            comma(&mut json, i);
            json.push_str("{\"id\":");
            push_json_string(&mut json, id);
            json.push_str(",\"path\":");
            push_json_string(&mut json, &format!("~/{relative}"));
            json.push('}');
        }
        json.push_str("],\"tools\":[");
        for (i, found) in self.tools.iter().enumerate() {
            comma(&mut json, i);
            push_tool(&mut json, home, found);
        }
        json.push_str("],\"checkouts\":[");
        for (i, checkout) in self.checkouts.iter().enumerate() {
            comma(&mut json, i);
            push_checkout(&mut json, home, checkout);
        }
        json.push_str("],\"skills\":[");
        for (i, skill) in self.skills.iter().enumerate() {
            comma(&mut json, i);
            push_skill(&mut json, home, skill);
        }
        json.push_str("]}");
        json
    }
}

fn comma(json: &mut String, i: usize) {
    if i > 0 {
        json.push(',');
    }
}

fn push_field(json: &mut String, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        json.push_str(&format!(",\"{name}\":"));
        push_json_string(json, value);
    }
}

fn push_tool(json: &mut String, home: &Home, found: &Found) {
    let tool = found.tool;
    json.push_str("{\"id\":");
    push_json_string(json, tool.id);
    push_field(json, "name", Some(tool.name));
    push_field(json, "command", Some(tool.command));
    push_field(
        json,
        "path",
        found.path.as_deref().map(|p| home.show(p)).as_deref(),
    );
    push_field(json, "version", found.version.as_deref());
    push_field(json, "skills", tool.skills);
    json.push_str(&format!(
        ",\"signedIn\":null,\"credentialsPresent\":{},\"settings\":[",
        found.credentials_present
    ));
    for (i, (file, present)) in found.settings.iter().enumerate() {
        comma(json, i);
        json.push_str("{\"path\":");
        push_json_string(json, &format!("~/{file}"));
        json.push_str(&format!(",\"present\":{present}}}"));
    }
    json.push_str("]}");
}

fn push_checkout(json: &mut String, home: &Home, checkout: &Checkout) {
    json.push_str("{\"name\":");
    push_json_string(json, &checkout.name());
    push_field(json, "path", Some(&home.show(&checkout.path)));
    push_field(json, "branch", checkout.branch.as_deref());
    push_field(json, "commit", checkout.commit.as_deref());
    push_field(json, "subject", checkout.subject.as_deref());
    push_field(json, "upstream", checkout.upstream.as_deref());
    push_field(json, "setup", checkout.setup);
    let (fetch, error) = match &checkout.fetch {
        Fetch::Fetching => ("fetching", None),
        Fetch::Fetched => ("fetched", None),
        Fetch::Failed(why) => ("failed", Some(why.as_str())),
    };
    push_field(json, "fetch", Some(fetch));
    push_field(json, "fetchError", error);
    json.push_str(&format!(
        ",\"ahead\":{},\"behind\":{},\"dirty\":{}}}",
        checkout.ahead, checkout.behind, checkout.dirty
    ));
}

fn push_skill(json: &mut String, home: &Home, skill: &Skill) {
    json.push_str("{\"name\":");
    push_json_string(json, &skill.name);
    push_field(
        json,
        "description",
        Some(&skill.description)
            .filter(|d| !d.is_empty())
            .map(String::as_str),
    );
    push_field(
        json,
        "source",
        skill.source.as_deref().map(|p| home.show(p)).as_deref(),
    );
    push_field(
        json,
        "checkout",
        skill.checkout.as_deref().map(|p| home.show(p)).as_deref(),
    );
    json.push_str(&format!(",\"behind\":{},\"links\":{{", skill.behind));
    for (i, ((id, _), link)) in FOLDERS.iter().zip(&skill.links).enumerate() {
        comma(json, i);
        push_json_string(json, id);
        json.push(':');
        push_json_string(json, link.word());
    }
    json.push_str("},\"locations\":{");
    for (i, (id, _)) in FOLDERS.iter().enumerate() {
        comma(json, i);
        push_json_string(json, id);
        json.push_str(":{");
        json.push_str("\"state\":");
        push_json_string(json, skill.links[i].word());
        push_field(
            json,
            "source",
            skill.sources[i].as_deref().map(|p| home.show(p)).as_deref(),
        );
        push_field(
            json,
            "checkout",
            skill.checkouts[i]
                .as_deref()
                .map(|p| home.show(p))
                .as_deref(),
        );
        json.push_str(&format!(",\"behind\":{}", skill.behind_by_folder[i]));
        json.push('}');
    }
    json.push_str("}}");
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

//! The Brain's routes against a scratch corpus, with a stand-in for Context
//! Citadel that answers its search contract (`POST /v1/library/search`). The
//! browser test (`ui/e2e/brain.e2e.ts`) runs the real Citadel.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::State;
use axum::routing::post;
use parvi_brain::brain::{self, Brain};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A scratch corpus, removed when the test ends.
struct Corpus(PathBuf);

impl Corpus {
    fn new(name: &str) -> Corpus {
        let root = std::env::temp_dir().join(format!("parvi-80-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Corpus(root)
    }

    fn write(&self, path: &str, text: &str) {
        let file = self.0.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
}

impl Drop for Corpus {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Serves `router` on a free loopback port.
async fn serve(router: axum::Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    addr
}

async fn serve_brain(corpus: Option<&Path>, citadel: Option<&str>) -> SocketAddr {
    serve(brain::routes(std::sync::Arc::new(
        Brain::new(corpus, citadel).unwrap(),
    )))
    .await
}

/// GETs `path` and returns the status and the JSON body.
async fn get(addr: SocketAddr, path: &str) -> (u16, Value) {
    request(addr, "GET", path, None).await
}

/// PUTs `body` as JSON at `path`.
async fn put(addr: SocketAddr, path: &str, body: Value) -> (u16, Value) {
    request(addr, "PUT", path, Some(body)).await
}

async fn request(addr: SocketAddr, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.unwrap();
    let (head, body) = answer.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

fn paths(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|item| item["path"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn reads_a_page_with_its_links_and_backlinks() {
    let corpus = Corpus::new("read");
    corpus.write(
        "knowledge/upscaling/upscaling.md",
        "---\nkind: topic\nsummary: \"How example upscales big images\"\n---\n\
         # Tiled upscaling\n\nTiles overlap; see [[seams]], [[Bench results|the bench]] \
         and [the study](../../evidence/study/README.md#method).\n\
         Not yet written: [[vram-budget]].\n\n```toml\n[[steps]]\n```\n",
    );
    corpus.write(
        "knowledge/upscaling/seams.md",
        "---\nkind: note\nsummary: Blend seams\n---\n# Seam blending\n\nPart of [[upscaling]].\n",
    );
    corpus.write(
        "evidence/study/README.md",
        "---\nkind: research\n---\n# A study\n\nFor [[upscaling]] and again [[upscaling#tiles]].\n",
    );
    corpus.write("evidence/bench.md", "# Bench results\n\n41.2 img/s.\n");
    corpus.write(
        "knowledge/unrelated.md",
        "# Unrelated\n\n`[[upscaling]]` in code.\n",
    );
    corpus.write(".git/stray.md", "# Stray\n\n[[upscaling]]\n");
    let addr = serve_brain(Some(&corpus.0), None).await;

    let (status, page) = get(
        addr,
        "/api/brain/page?path=knowledge/upscaling/upscaling.md",
    )
    .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["title"], "Tiled upscaling");
    assert_eq!(page["kind"], "topic");
    assert_eq!(page["summary"], "How example upscales big images");
    assert!(
        page["body"]
            .as_str()
            .unwrap()
            .starts_with("# Tiled upscaling")
    );
    let links: Vec<(&str, Option<&str>)> = page["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["target"].as_str().unwrap(), l["page"]["path"].as_str()))
        .collect();
    assert_eq!(
        links,
        [
            ("seams", Some("knowledge/upscaling/seams.md")),
            ("Bench results", Some("evidence/bench.md")),
            (
                "../../evidence/study/README.md",
                Some("evidence/study/README.md")
            ),
            ("vram-budget", None),
        ]
    );
    assert_eq!(page["links"][1]["page"]["title"], "Bench results");
    // By title, each once; code and dot-folders do not link.
    assert_eq!(
        paths(&page["backlinks"]),
        ["evidence/study/README.md", "knowledge/upscaling/seams.md"]
    );

    // Following a link reads the page it names.
    let (status, seams) = get(addr, "/api/brain/page?path=knowledge/upscaling/seams.md").await;
    assert_eq!(status, 200);
    assert_eq!(seams["title"], "Seam blending");
    assert_eq!(
        paths(&seams["backlinks"]),
        ["knowledge/upscaling/upscaling.md"]
    );
}

#[tokio::test]
async fn related_pages_include_links_and_selective_shared_tags() {
    let corpus = Corpus::new("related");
    corpus.write(
        "notes/a.md",
        "---\ntags: [Shared, common]\n---\n# A\n\n[[b]]\n",
    );
    corpus.write("notes/b.md", "---\ntags: [common]\n---\n# B\n");
    corpus.write(
        "notes/c.md",
        "---\ntags:\n  - shared\n  - common\n---\n# C\n",
    );
    corpus.write("notes/d.md", "# D\n\n[[a]]\n");
    for i in 0..4 {
        corpus.write(&format!("notes/filler{i}.md"), "# Filler\n");
    }
    let addr = serve_brain(Some(&corpus.0), None).await;
    let (_, page) = get(addr, "/api/brain/page?path=notes/a.md").await;
    assert_eq!(
        paths(&page["related"]),
        ["notes/b.md", "notes/c.md", "notes/d.md"]
    );
    let (_, unrelated) = get(addr, "/api/brain/page?path=notes/b.md").await;
    assert_eq!(paths(&unrelated["related"]), ["notes/a.md"]);
}

#[tokio::test]
async fn refuses_paths_outside_the_corpus() {
    let corpus = Corpus::new("refuse");
    corpus.write("knowledge/a.md", "# A\n");
    corpus.write(".git/b.md", "# B\n");
    let outside = Corpus::new("outside");
    outside.write("secret.md", "# Secret\n");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        outside.0.join("secret.md"),
        corpus.0.join("knowledge/link.md"),
    )
    .unwrap();
    let addr = serve_brain(Some(&corpus.0), None).await;

    let secret = outside.0.join("secret.md");
    let cases = [
        ("knowledge/a.md", 200),
        ("knowledge/missing.md", 404),
        ("knowledge/../knowledge/a.md", 403),
        ("../outside.md", 403),
        (secret.to_str().unwrap(), 403),
        (".git/b.md", 403),
        ("knowledge", 400),
        ("", 403),
        #[cfg(unix)]
        ("knowledge/link.md", 403),
    ];
    for (path, expected) in cases {
        let (status, body) = get(addr, &format!("/api/brain/page?path={path}")).await;
        assert_eq!(status, expected, "{path}: {body}");
        if expected != 200 {
            assert!(body["error"]["message"].is_string(), "{path}: {body}");
        }
    }
}

/// What the stand-in Citadel was asked.
type Asked = Arc<Mutex<Vec<Value>>>;

async fn citadel(asked: Asked) -> SocketAddr {
    async fn search(State(asked): State<Asked>, Json(body): Json<Value>) -> Json<Value> {
        asked.lock().unwrap().push(body);
        // Citadel's order is its ranking; it carries more fields than Parvi reads.
        Json(json!({ "results": [
            { "id": "upscaling", "slug": "upscaling", "path": "knowledge/upscaling/upscaling.md",
              "title": "Tiled upscaling", "kind": "topic", "collection": "knowledge",
              "summary": "How example upscales", "snippet": "»Tiles« overlap" },
            { "id": "bench", "slug": "bench", "path": "evidence/bench.md",
              "title": "Bench results", "kind": "", "collection": "evidence",
              "summary": "", "snippet": "»tiles« per second" },
        ], "truncated": false }))
    }
    let router = axum::Router::new()
        .route("/v1/library/search", post(search))
        .with_state(asked);
    serve(router).await
}

#[tokio::test]
async fn searches_through_citadel_keeping_its_ranking() {
    let asked = Asked::default();
    let url = format!("http://{}", citadel(asked.clone()).await);
    let addr = serve_brain(None, Some(&url)).await;

    let (status, answer) = get(addr, "/api/brain/search?q=tiles%20overlap").await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        paths(&answer["results"]),
        ["knowledge/upscaling/upscaling.md", "evidence/bench.md"]
    );
    assert_eq!(answer["results"][0]["snippet"], "»Tiles« overlap");
    assert_eq!(answer["results"][1]["collection"], "evidence");
    assert_eq!(
        *asked.lock().unwrap(),
        [json!({ "query": "tiles overlap", "limit": 50 })]
    );

    // An empty search asks nothing.
    let (status, answer) = get(addr, "/api/brain/search?q=%20").await;
    assert_eq!((status, answer), (200, json!({ "results": [] })));
    assert_eq!(asked.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn says_what_is_missing() {
    let unconfigured = serve_brain(None, None).await;
    let (status, body) = get(unconfigured, "/api/brain/search?q=x").await;
    assert_eq!(status, 503);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("[brain].citadel")
    );
    let (status, body) = get(unconfigured, "/api/brain/page?path=a.md").await;
    assert_eq!(status, 503);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("[brain].corpus")
    );

    // A Citadel that is not there is a bad gateway, not an empty answer.
    let gone = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", gone.local_addr().unwrap());
    drop(gone);
    let addr = serve_brain(None, Some(&url)).await;
    let (status, body) = get(addr, "/api/brain/search?q=x").await;
    assert_eq!(status, 502);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Citadel")
    );

    assert!(Brain::new(Some(Path::new("/nonexistent/parvi-80")), None).is_err());
}

/// Runs git in `dir` as the Librarian, who writes the corpus between edits.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=Librarian",
            "-c",
            "user.email=librarian@example.org",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// A corpus in git: two commits by the Librarian.
fn shelved(name: &str) -> Corpus {
    let corpus = Corpus::new(name);
    git(&corpus.0, &["init", "-q"]);
    corpus.write(
        "knowledge/upscaling.md",
        "---\nkind: topic\n---\n# Tiled upscaling\n\nTiles.\n",
    );
    corpus.write("self/pace.md", "# Pace\n\nSlow.\n");
    corpus.write("knowledge/notes.txt", "not a page\n");
    git(&corpus.0, &["add", "-A"]);
    git(&corpus.0, &["commit", "-q", "-m", "Shelve the first pages"]);
    corpus.write("knowledge/seams.md", "# Seam blending\n");
    corpus.write(
        "knowledge/upscaling.md",
        "---\nkind: topic\n---\n# Tiled upscaling\n\nTiles overlap.\n",
    );
    git(&corpus.0, &["add", "-A"]);
    git(&corpus.0, &["commit", "-q", "-m", "File seams"]);
    corpus
}

#[tokio::test]
async fn lists_collections_arrivals_and_a_pages_history() {
    let corpus = shelved("shelves");
    corpus.write(".hidden/x.md", "# Hidden\n");
    corpus.write("evidence/.keep", "");
    let addr = serve_brain(Some(&corpus.0), None).await;

    let (status, body) = get(addr, "/api/brain/collections").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["collections"],
        json!([
            { "name": "evidence", "pages": 0 },
            { "name": "knowledge", "pages": 2 },
            { "name": "self", "pages": 1 },
        ])
    );

    // Newest first, each with the pages it touched, by title.
    let (status, body) = get(addr, "/api/brain/changes").await;
    assert_eq!(status, 200, "{body}");
    let changes = body["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0]["message"], "File seams");
    assert_eq!(changes[0]["author"], "Librarian");
    assert_eq!(
        paths(&changes[0]["pages"]),
        ["knowledge/seams.md", "knowledge/upscaling.md"]
    );
    assert_eq!(changes[0]["pages"][0]["title"], "Seam blending");
    // The text file was in the commit, and is no page.
    assert_eq!(
        paths(&changes[1]["pages"]),
        ["knowledge/upscaling.md", "self/pace.md"]
    );
    assert!(body["error"].is_null());
    let (_, one) = get(addr, "/api/brain/changes?limit=1").await;
    assert_eq!(one["changes"].as_array().unwrap().len(), 1);
    let (status, _) = get(addr, "/api/brain/changes?limit=0").await;
    assert_eq!(status, 400);

    let (status, body) = get(addr, "/api/brain/collection?name=knowledge").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        paths(&body["pages"]),
        ["knowledge/seams.md", "knowledge/upscaling.md"]
    );
    assert_eq!(body["pages"][1]["title"], "Tiled upscaling");
    assert_eq!(body["pages"][1]["kind"], "topic");
    assert_eq!(body["pages"][1]["author"], "Librarian");
    assert!(
        body["pages"][1]["updated"]
            .as_str()
            .unwrap()
            .starts_with("20")
    );
    for (name, status) in [("missing", 404), (".hidden", 403), ("../x", 403), ("", 403)] {
        let (got, body) = get(addr, &format!("/api/brain/collection?name={name}")).await;
        assert_eq!(got, status, "{name}: {body}");
    }

    let (status, page) = get(addr, "/api/brain/page?path=knowledge/upscaling.md").await;
    assert_eq!(status, 200, "{page}");
    let messages: Vec<&str> = page["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["message"].as_str().unwrap())
        .collect();
    assert_eq!(messages, ["File seams", "Shelve the first pages"]);
    assert_eq!(page["author"], "Librarian");
    assert_eq!(page["updated"], page["history"][0]["time"]);
    assert_eq!(page["front"], "---\nkind: topic\n---\n");
    assert!(page.get("history_error").is_none());
}

#[tokio::test]
async fn an_edit_is_one_commit_signed_as_the_operator() {
    let corpus = shelved("save");
    let brain = Brain::new(Some(&corpus.0), None)
        .unwrap()
        .with_author(Some("Ada Operator <ada@example.org>"))
        .unwrap();
    let addr = serve(brain::routes(Arc::new(brain))).await;
    // Somebody else's change elsewhere stays theirs.
    corpus.write("self/pace.md", "# Pace\n\nFast now.\n");

    let text = "---\nkind: topic\n---\n# Tiled upscaling\n\nTiles overlap by 64 px.\n";
    let save =
        json!({ "path": "knowledge/upscaling.md", "content": text, "summary": "Say the overlap" });
    let (status, commit) = put(addr, "/api/brain/page", save.clone()).await;
    assert_eq!(status, 200, "{commit}");
    assert_eq!(commit["message"], "Say the overlap");
    assert_eq!(commit["author"], "Ada Operator");
    let log = git(
        &corpus.0,
        &["log", "-1", "--format=%an <%ae>|%s", "--name-only"],
    );
    assert_eq!(
        log.trim(),
        "Ada Operator <ada@example.org>|Say the overlap\n\nknowledge/upscaling.md"
    );
    let (_, page) = get(addr, "/api/brain/page?path=knowledge/upscaling.md").await;
    assert_eq!(
        page["body"],
        "# Tiled upscaling\n\nTiles overlap by 64 px.\n"
    );
    assert_eq!(page["history"][0]["hash"], commit["hash"]);
    assert_eq!(page["author"], "Ada Operator");
    assert_eq!(
        git(&corpus.0, &["status", "--porcelain"]).trim(),
        "M self/pace.md",
        "only the edited page is committed"
    );

    // The same text again commits nothing and answers the last commit.
    let (status, again) = put(addr, "/api/brain/page", save).await;
    assert_eq!((status, &again["hash"]), (200, &commit["hash"]));
    // No summary: Edit and the path.
    let plain = json!({ "path": "knowledge/seams.md", "content": "# Seam blending\n\nFeather.\n" });
    let (_, commit) = put(addr, "/api/brain/page", plain).await;
    assert_eq!(commit["message"], "Edit knowledge/seams.md");

    // A page already changed in the corpus is refused, and left as it is.
    let dirty = json!({ "path": "self/pace.md", "content": "# Pace\n" });
    let (status, body) = put(addr, "/api/brain/page", dirty).await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("uncommitted")
    );
    let pace = std::fs::read_to_string(corpus.0.join("self/pace.md")).unwrap();
    assert_eq!(pace, "# Pace\n\nFast now.\n");

    for (path, expected) in [
        ("../x.md", 403),
        ("knowledge/new.md", 404),
        (".git/config", 403),
    ] {
        let (status, body) = put(
            addr,
            "/api/brain/page",
            json!({ "path": path, "content": "x" }),
        )
        .await;
        assert_eq!(status, expected, "{path}: {body}");
    }
    let (status, _) = put(addr, "/api/brain/page", json!({ "content": "x" })).await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn a_commit_git_refuses_puts_the_page_back() {
    let corpus = shelved("refused");
    // A hook that refuses every commit, set locally so no global hook path wins.
    corpus.write(
        "hooks/pre-commit",
        "#!/bin/sh\necho 'refused by the hook' >&2\nexit 1\n",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hook = corpus.0.join("hooks/pre-commit");
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git(&corpus.0, &["config", "core.hooksPath", "hooks"]);
    // No author is configured: the checkout's own identity signs.
    git(&corpus.0, &["config", "user.name", "Corpus Owner"]);
    git(&corpus.0, &["config", "user.email", "owner@example.org"]);
    let addr = serve_brain(Some(&corpus.0), None).await;
    let before = std::fs::read_to_string(corpus.0.join("self/pace.md")).unwrap();

    let save = json!({ "path": "self/pace.md", "content": "# Pace\n\nChanged.\n" });
    let (status, body) = put(addr, "/api/brain/page", save).await;
    assert_eq!(status, 500, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("refused by the hook"),
        "{body}"
    );
    let after = std::fs::read_to_string(corpus.0.join("self/pace.md")).unwrap();
    assert_eq!(after, before);
    let status = git(&corpus.0, &["status", "--porcelain", "--", "self/pace.md"]);
    assert_eq!(status, "", "the page is clean again");

    git(&corpus.0, &["config", "--unset", "core.hooksPath"]);
    let save = json!({ "path": "self/pace.md", "content": "# Pace\n\nChanged.\n" });
    let (status, commit) = put(addr, "/api/brain/page", save).await;
    assert_eq!(
        (status, commit["author"].as_str()),
        (200, Some("Corpus Owner"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_disconnected_editor_does_not_cancel_a_commit_halfway() {
    use axum::body::Body;
    use axum::http::Request;
    use std::os::unix::fs::PermissionsExt;
    use tower::ServiceExt;

    let corpus = shelved("disconnected");
    git(&corpus.0, &["config", "user.name", "Corpus Owner"]);
    git(&corpus.0, &["config", "user.email", "owner@example.org"]);
    corpus.write(
        "hooks/pre-commit",
        "#!/bin/sh\ntouch ready\nwhile [ ! -e release ]; do sleep 0.05; done\n",
    );
    std::fs::set_permissions(
        corpus.0.join("hooks/pre-commit"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    git(&corpus.0, &["config", "core.hooksPath", "hooks"]);
    let app = brain::routes::<()>(Arc::new(Brain::new(Some(&corpus.0), None).unwrap()));
    let content = "# Pace\n\nSaved despite the disconnect.\n";
    let body = json!({"path":"self/pace.md", "content":content, "summary":"Keep pace"}).to_string();
    let request = |body: String| {
        Request::builder()
            .method("PUT")
            .uri("/api/brain/page")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    };
    let first = tokio::spawn(app.clone().oneshot(request(body.clone())));
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !corpus.0.join("ready").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("commit hook did not start");
    first.abort();
    let _ = first.await;
    corpus.write("release", "go\n");
    // A second save waits for the first lock, then sees the committed text.
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        app.oneshot(request(body)),
    )
    .await
    .expect("save did not finish")
    .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(corpus.0.join("self/pace.md")).unwrap(),
        content
    );
    assert_eq!(
        git(
            &corpus.0,
            &["log", "-1", "--format=%s", "--", "self/pace.md"]
        )
        .trim(),
        "Keep pace"
    );
    assert!(git(&corpus.0, &["status", "--porcelain", "--", "self/pace.md"]).is_empty());
}

#[tokio::test]
async fn a_corpus_git_will_not_read_says_why() {
    let corpus = Corpus::new("nogit");
    corpus.write("knowledge/a.md", "# A\n");
    let addr = serve_brain(Some(&corpus.0), None).await;
    let (status, page) = get(addr, "/api/brain/page?path=knowledge/a.md").await;
    assert_eq!(status, 200);
    assert_eq!(page["history"], json!([]));
    assert!(
        page["history_error"].as_str().unwrap().contains("git"),
        "{page}"
    );
    let (status, body) = get(addr, "/api/brain/changes").await;
    assert_eq!(status, 200);
    assert_eq!(body["changes"], json!([]));
    assert!(body["error"].is_string());
    let (status, _) = put(
        addr,
        "/api/brain/page",
        json!({ "path": "knowledge/a.md", "content": "# B\n" }),
    )
    .await;
    assert_eq!(status, 500);
    assert_eq!(
        std::fs::read_to_string(corpus.0.join("knowledge/a.md")).unwrap(),
        "# A\n"
    );
}

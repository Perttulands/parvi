//! Files' HTTP interface, as a browser or another node calls it.

use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use parvi_files::{Files, MAX_BYTES};
use tower::ServiceExt;

const NODE: &str = "test-node";

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path =
            std::env::temp_dir().join(format!("parvi-files-http-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Scratch(path.canonicalize().unwrap())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn app(root: &Path) -> Router {
    parvi_files::routes(
        std::sync::Arc::new(Files::new(NODE.to_owned(), &[root.to_path_buf()], None).unwrap()),
        std::sync::Arc::new(parvi_files::drops::Drops::new(root.join("drops"), vec![])),
    )
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn at(route: &str, path: &Path) -> String {
    format!(
        "/api/files/{route}?node={NODE}&path={}",
        encode(path.to_str().unwrap())
    )
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: impl Into<Body>,
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(body.into())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, body, headers)
}

/// The `base` an editor sends: the SHA-256 of what it read, in hex.
fn version(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[tokio::test]
async fn roots_name_the_node_and_its_roots() {
    let scratch = Scratch::new("roots");
    let (status, body, _) = call(&app(&scratch.0), "GET", "/api/files/roots", Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    let path = scratch.0.to_str().unwrap();
    // The name drops Windows' verbatim prefix; the path keeps it.
    let name = path.strip_prefix(r"\\?\").unwrap_or(path);
    let expected = serde_json::json!({"node": NODE, "roots": [{"name": name, "path": path}]});
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
        expected
    );
}

#[tokio::test]
async fn a_request_for_another_node_is_not_answered_here() {
    let scratch = Scratch::new("other");
    let uri = format!(
        "/api/files/folder?node=elsewhere&path={}",
        encode(scratch.0.to_str().unwrap())
    );
    let (status, body, _) = call(&app(&scratch.0), "GET", &uri, Body::empty()).await;
    assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
    assert!(text(&body).contains("elsewhere"), "{}", text(&body));
}

#[tokio::test]
async fn create_save_list_read_rename_delete() {
    let scratch = Scratch::new("cycle");
    let app = app(&scratch.0);
    let folder = scratch.0.join("models");
    let file = folder.join("notes.md");

    assert_eq!(
        call(&app, "POST", &at("folder", &folder), Body::empty())
            .await
            .0,
        StatusCode::CREATED
    );
    assert_eq!(
        call(&app, "POST", &at("file", &file), Body::empty())
            .await
            .0,
        StatusCode::CREATED
    );
    let (status, body, _) = call(&app, "POST", &at("file", &file), Body::empty()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(text(&body).contains("error"));

    assert_eq!(
        call(&app, "PUT", &at("file", &file), "# kuvio").await.0,
        StatusCode::NO_CONTENT
    );
    let (status, body, _) = call(&app, "GET", &at("folder", &folder), Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    let listing = text(&body);
    assert!(
        listing.contains(r#""name":"notes.md","folder":false,"size":7"#),
        "{listing}"
    );

    let (status, body, headers) = call(&app, "GET", &at("file", &file), Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"# kuvio");
    assert_eq!(headers["content-type"], "application/octet-stream");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["content-security-policy"], "sandbox");

    let moved = folder.join("README.md");
    let uri = format!(
        "{}&to={}",
        at("rename", &file),
        encode(moved.to_str().unwrap())
    );
    assert_eq!(
        call(&app, "POST", &uri, Body::empty()).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(std::fs::read(&moved).unwrap(), b"# kuvio");

    assert_eq!(
        call(&app, "DELETE", &at("file", &folder), Body::empty())
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert!(!folder.exists());
}

#[tokio::test]
async fn pictures_are_served_as_pictures() {
    let scratch = Scratch::new("picture");
    let picture = scratch.0.join("Shot.PNG");
    std::fs::write(&picture, b"\x89PNG").unwrap();
    let (status, _, headers) = call(
        &app(&scratch.0),
        "GET",
        &at("file", &picture),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "image/png");
}

#[tokio::test]
async fn viewer_reads_original_bytes_and_missing_files_report_why() {
    let scratch = Scratch::new("viewer");
    let app = app(&scratch.0);
    for (name, contents) in [
        ("notes.md", &b"# Notes\n[other](other.md)\n"[..]),
        ("data.json", &b"{\"a\":1}"[..]),
        ("binary.dat", &b"\0\x01\xff"[..]),
    ] {
        let file = scratch.0.join(name);
        std::fs::write(&file, contents).unwrap();
        let (status, body, _) = call(&app, "GET", &at("file", &file), Body::empty()).await;
        assert_eq!(status, StatusCode::OK, "{name}");
        assert_eq!(body, contents, "{name}");
    }
    let (status, body, _) = call(
        &app,
        "GET",
        &at("file", &scratch.0.join("missing.png")),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!body.is_empty(), "the viewer has an error to show");
}

#[tokio::test]
async fn refusals_carry_their_status() {
    let scratch = Scratch::new("refusals");
    let root = scratch.0.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(scratch.0.join("outside.txt"), "x").unwrap();
    std::fs::write(root.join("a.txt"), "a").unwrap();
    std::fs::write(root.join("b.txt"), "b").unwrap();
    let app = app(&root);

    let outside = scratch.0.join("outside.txt");
    assert_eq!(
        call(&app, "GET", &at("file", &outside), Body::empty())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "DELETE", &at("file", &root), Body::empty())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "GET", &at("file", &root.join("gone")), Body::empty())
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &at("folder", &root.join("a.txt")),
            Body::empty()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let uri = format!(
        "{}&to={}",
        at("rename", &root.join("a.txt")),
        encode(root.join("b.txt").to_str().unwrap())
    );
    assert_eq!(
        call(&app, "POST", &uri, Body::empty()).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(std::fs::read(root.join("b.txt")).unwrap(), b"b");
}

#[tokio::test]
async fn a_save_from_a_stale_base_and_an_upload_over_a_file_are_refused() {
    let scratch = Scratch::new("stale");
    let app = app(&scratch.0);
    let file = scratch.0.join("plan.txt");

    let (status, _, _) = call(&app, "POST", &at("file", &file), "first").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(std::fs::read(&file).unwrap(), b"first");
    let (status, _, _) = call(&app, "POST", &at("file", &file), "upload").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(std::fs::read(&file).unwrap(), b"first");

    let read = version(b"first");
    std::fs::write(&file, "an agent's edit").unwrap();
    let stale = format!("{}&base={read}", at("file", &file));
    let (status, body, _) = call(&app, "PUT", &stale, "operator's draft").await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    assert!(text(&body).contains("Changed"), "{}", text(&body));
    assert_eq!(std::fs::read(&file).unwrap(), b"an agent's edit");

    let current = version(b"an agent's edit");
    let fresh = format!("{}&base={current}", at("file", &file));
    assert_eq!(
        call(&app, "PUT", &fresh, "merged").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"merged");
}

#[tokio::test]
async fn an_oversized_save_is_refused_and_writes_nothing() {
    let scratch = Scratch::new("oversized");
    let file = scratch.0.join("big.bin");
    let body = vec![0u8; MAX_BYTES as usize + 1];
    let (status, _, _) = call(&app(&scratch.0), "PUT", &at("file", &file), body).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(!file.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn find_and_content_search_report_an_unreadable_chosen_folder() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("find-permission");
    let locked = scratch.0.join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::write(locked.join("note.txt"), "needle").unwrap();
    let app = app(&scratch.0);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let mut responses = Vec::new();
    for route in ["find", "search"] {
        let uri = format!("{}&q=needle", at(route, &locked));
        responses.push(call(&app, "GET", &uri, Body::empty()).await);
    }
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    for (status, body, _) in responses {
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(text(&body).contains("Permission denied"), "{}", text(&body));
    }
}

#[tokio::test]
async fn diff_reports_head_changes_no_repository_and_git_errors() {
    use std::process::Command;
    let scratch = Scratch::new("diff");
    let repo = scratch.0.join("repo");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["init", "-q"]);
    let file = repo.join("[notes].txt");
    std::fs::write(&file, "old\n").unwrap();
    let app = app(&scratch.0);
    let get = |path: &Path| at("diff", path);
    let (_, body, _) = call(&app, "GET", &get(&file), Body::empty()).await;
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(result["error"].as_str().unwrap().contains("HEAD"));
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "-qm",
        "initial",
    ]);
    std::fs::write(&file, "new\n").unwrap();
    let (status, body, _) = call(&app, "GET", &get(&file), Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(result["repository"], repo.to_str().unwrap());
    assert!(result["diff"].as_str().unwrap().contains("-old\n+new"));
    assert_eq!(result["truncated"], false);
    std::fs::write(&file, format!("{}\n", "x".repeat(1_100_000))).unwrap();
    let (_, body, _) = call(&app, "GET", &get(&file), Body::empty()).await;
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(result["truncated"], true);
    assert!(result["diff"].as_str().unwrap().len() <= 1 << 20);
    let plain = scratch.0.join("plain");
    std::fs::write(&plain, "plain").unwrap();
    let (_, body, _) = call(&app, "GET", &get(&plain), Body::empty()).await;
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(result["repository"], "");
    assert_eq!(result["diff"], "");
    assert_eq!(
        call(&app, "GET", &get(&scratch.0), Body::empty()).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "GET", &get(&scratch.0.join("missing")), Body::empty())
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

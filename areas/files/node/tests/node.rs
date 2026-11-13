use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use parvi_node_kit::{NodeClient, cli};
use parvi_testkit::node::Node;
use serde_json::{Value, json};

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn at(route: &str, path: &std::path::Path) -> String {
    format!("/api/files/{route}?path={}", enc(path.to_str().unwrap()))
}
async fn request(
    node: &Node,
    method: &str,
    uri: &str,
    body: impl Into<Body>,
    json: bool,
) -> (StatusCode, Vec<u8>) {
    let socket = tokio::net::TcpStream::connect(node.address())
        .await
        .unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(socket))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", node.address().to_string());
    if json {
        req = req.header("content-type", "application/json");
    }
    let response = sender
        .send_request(req.body(body.into()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
}

#[tokio::test]
async fn node_routes_and_local_copy_stream() {
    let root = parvi_testkit::fresh_dir("files-node");
    let conf = format!(
        "[files]\nroots = [{}]\n",
        serde_json::to_string(root.to_str().unwrap()).unwrap()
    );
    let node = Node::start(&[parvi_files::LISTING], &conf).await.unwrap();
    let (status, bytes) = request(&node, "GET", "/api/files/roots", Body::empty(), false).await;
    assert_eq!(status, StatusCode::OK);
    let roots: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(roots["node"], node.id());
    let folder = root.join("folder");
    let dest = root.join("dest");
    let file = folder.join("note.txt");
    for dir in [&folder, &dest] {
        assert_eq!(
            request(&node, "POST", &at("folder", dir), Body::empty(), false)
                .await
                .0,
            StatusCode::CREATED
        );
    }
    assert_eq!(
        request(&node, "POST", &at("file", &file), "hello", false)
            .await
            .0,
        StatusCode::CREATED
    );
    assert_eq!(
        request(&node, "GET", &at("file", &file), Body::empty(), false)
            .await
            .1,
        b"hello"
    );
    assert_eq!(
        request(&node, "PUT", &at("file", &file), "hello again", false)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    for route in ["folder", "find", "search", "diff"] {
        let path = if route == "diff" { &file } else { &folder };
        let q = if route == "find" || route == "search" {
            "&q=hello"
        } else {
            ""
        };
        assert_eq!(
            request(
                &node,
                "GET",
                &format!("{}{q}", at(route, path)),
                Body::empty(),
                false
            )
            .await
            .0,
            StatusCode::OK,
            "{route}"
        );
    }
    let id = "0123456789abcdef0123456789abcdef";
    let start = json!({"node":node.id(),"path":file,"target_node":node.id(),"folder":dest,"id":id});
    assert_eq!(
        request(
            &node,
            "POST",
            "/api/files/bridge/start",
            start.to_string(),
            true
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let (status, stream) = request(
        &node,
        "GET",
        &format!("/api/files/bridge/events?id={id}"),
        Body::empty(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&stream).contains("\"state\":\"done\""));
    assert_eq!(
        std::fs::read(dest.join("note.txt")).unwrap(),
        b"hello again"
    );
    let moved = folder.join("renamed.txt");
    assert_eq!(
        request(
            &node,
            "POST",
            &format!(
                "{}&to={}",
                at("rename", &file),
                enc(moved.to_str().unwrap())
            ),
            Body::empty(),
            false
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&node, "DELETE", &at("file", &moved), Body::empty(), false)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let (status, body) = request(&node, "POST", "/api/drops?name=note.txt", "drop", false).await;
    assert_eq!(status, StatusCode::CREATED);
    let drop: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        std::fs::read(drop["path"].as_str().unwrap()).unwrap(),
        b"drop"
    );
    let (status, body) = request(
        &node,
        "GET",
        "/api/files/folder?node=elsewhere&path=%2F",
        Body::empty(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["error"]["code"], "unknown_node");
    let (status, body) = request(&node, "GET", "/api/files/folder", Body::empty(), false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["error"]["code"], "bad_request");
    node.stop().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn exact_commands_are_exposed_to_mcp_and_cli_and_operate_on_files() {
    let root = parvi_testkit::fresh_dir("files-commands");
    let config = format!(
        "[files]\nroots = [{}]\n",
        serde_json::to_string(root.to_str().unwrap()).unwrap()
    );
    let node = Node::start(&[parvi_files::LISTING], &config).await.unwrap();
    let client = NodeClient::new(Some(&node.url())).unwrap();
    let (answer, _) = client
        .request(
            Method::POST,
            "/mcp",
            Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})),
            None,
        )
        .await
        .unwrap();
    let mut actual: Vec<_> = answer["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .filter(|name| !parvi_node_kit::own::COMMANDS.contains(name))
        .collect();
    actual.sort();
    let mut expected = vec![
        "file_list",
        "file_read",
        "file_diff",
        "file_recent",
        "file_download",
        "file_upload",
        "file_write",
        "file_mkdir",
        "file_move",
        "file_delete",
        "file_find",
        "file_locate",
        "file_search",
        "file_copy",
        "file_resume",
        "file_transfer",
    ];
    expected.sort();
    assert_eq!(actual, expected);
    let mut out = Vec::new();
    let args = vec!["--node".into(), node.url()];
    cli::run("file", &args, &mut out).await.unwrap();
    let verbs = String::from_utf8(out).unwrap();
    for name in &expected {
        assert!(
            verbs.contains(&format!(
                "parvi file {}",
                name.strip_prefix("file_").unwrap()
            )),
            "{name}: {verbs}"
        );
    }
    let file = root.join("note.txt");
    let dest = root.join("dest");
    std::fs::create_dir(&dest).unwrap();
    node.call("file_write", json!({"path":file,"text":"hello"}))
        .await
        .unwrap();
    let read = node.call("file_read", json!({"path":file})).await.unwrap();
    assert_eq!(read["text"], "hello");
    let found = node
        .call("file_find", json!({"path":root,"query":"note"}))
        .await
        .unwrap();
    assert_eq!(found["matches"][0]["name"], "note.txt");
    let copy = node
        .call("file_copy", json!({"path":file,"to":dest}))
        .await
        .unwrap();
    assert_eq!(copy["transfer"]["state"], "done");
    let err = node
        .call(
            "file_copy",
            json!({"path":file,"to":dest,"to_node":"elsewhere"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "unknown_node");
    node.stop().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn file_diff_command_matches_the_route_and_prints_unified_lines() {
    let root = parvi_testkit::fresh_dir("files-diff-command");
    let repo = root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q"]);
    let file = repo.join("note.txt");
    std::fs::write(&file, "before\n").unwrap();
    git(&["add", "note.txt"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "-qm",
        "initial",
    ]);
    std::fs::write(&file, "after\n").unwrap();
    let plain = root.join("plain.txt");
    std::fs::write(&plain, "outside a repository").unwrap();
    let config = format!(
        "[files]\nroots = [{}]\n",
        serde_json::to_string(root.to_str().unwrap()).unwrap()
    );
    let node = Node::start(&[parvi_files::LISTING], &config).await.unwrap();
    let (status, body) = request(&node, "GET", &at("diff", &file), Body::empty(), false).await;
    assert_eq!(status, StatusCode::OK);
    let route: Value = serde_json::from_slice(&body).unwrap();
    let mut out = Vec::new();
    let args = vec![
        "diff".into(),
        file.to_string_lossy().into_owned(),
        "--node".into(),
        node.url(),
    ];
    assert_eq!(cli::run("file", &args, &mut out).await.unwrap(), 0);
    let human = String::from_utf8(out).unwrap();
    assert!(human.lines().any(|line| line == "-before"), "{human}");
    assert!(human.lines().any(|line| line == "+after"), "{human}");
    let mut args_json = args.clone();
    args_json.push("--json".into());
    let mut out = Vec::new();
    assert_eq!(cli::run("file", &args_json, &mut out).await.unwrap(), 0);
    assert_eq!(serde_json::from_slice::<Value>(&out).unwrap(), route);
    assert_eq!(
        node.call("file_diff", json!({"path": file})).await.unwrap(),
        route
    );

    let (status, body) = request(&node, "GET", &at("diff", &plain), Body::empty(), false).await;
    assert_eq!(status, StatusCode::OK);
    let no_repo: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(no_repo["repository"], "");
    assert_eq!(no_repo["diff"], "");
    assert_eq!(
        node.call("file_diff", json!({"path": plain}))
            .await
            .unwrap(),
        no_repo
    );
    let missing = root.join("missing");
    let (status, body) = request(&node, "GET", &at("diff", &missing), Body::empty(), false).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        node.call("file_diff", json!({"path": missing}))
            .await
            .unwrap_err()
            .code,
        error["error"]["code"].as_str().unwrap()
    );
    node.stop().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn files_cli_browses_finds_edits_and_deletes() {
    async fn run(node: &Node, args: &[&str]) -> Value {
        let mut args: Vec<String> = args.iter().map(|s| (*s).into()).collect();
        args.extend(["--json".into(), "--node".into(), node.url()]);
        let mut out = Vec::new();
        assert_eq!(
            cli::run("file", &args, &mut out).await.unwrap(),
            0,
            "{args:?}"
        );
        serde_json::from_slice(&out).unwrap()
    }
    let root = parvi_testkit::fresh_dir("files-cli-journey");
    let config = format!(
        "[files]\nroots = [{}]\n",
        serde_json::to_string(root.to_str().unwrap()).unwrap()
    );
    let node = Node::start(&[parvi_files::LISTING], &config).await.unwrap();
    let folder = root.join("folder");
    let file = folder.join("note.txt");
    let renamed = folder.join("renamed.txt");
    let root_text = root.to_str().unwrap();
    let folder_text = folder.to_str().unwrap();
    let file_text = file.to_str().unwrap();
    let renamed_text = renamed.to_str().unwrap();
    run(&node, &["mkdir", folder_text]).await;
    run(&node, &["write", file_text, "hello needle"]).await;
    assert_eq!(
        run(&node, &["read", file_text]).await["text"],
        "hello needle"
    );
    assert_eq!(
        run(&node, &["list", "--path", folder_text]).await["entries"][0]["name"],
        "note.txt"
    );
    assert_eq!(
        run(&node, &["find", root_text, "note"]).await["matches"][0]["name"],
        "note.txt"
    );
    assert_eq!(
        run(&node, &["search", root_text, "needle"]).await["matches"][0]["line"],
        1
    );
    let destination = root.join("copy-destination");
    run(&node, &["mkdir", destination.to_str().unwrap()]).await;
    let copied = run(&node, &["copy", file_text, destination.to_str().unwrap()]).await;
    assert_eq!(copied["transfer"]["state"], "done");
    assert_eq!(
        std::fs::read(destination.join("note.txt")).unwrap(),
        b"hello needle"
    );
    run(&node, &["move", file_text, renamed_text]).await;
    assert!(renamed.exists());
    run(&node, &["delete", renamed_text]).await;
    assert!(!renamed.exists());
    node.stop().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn files_cli_finds_recent_changes_and_names_and_round_trips_binary() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let root = parvi_testkit::fresh_dir("files-cli-binary");
    let project = root.join("repo");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::write(project.join("README.md"), "readme").unwrap();
    let config = format!(
        "[files]\nroots = [{}]\n",
        serde_json::to_string(root.to_str().unwrap()).unwrap()
    );
    let node = Node::start(&[parvi_files::LISTING], &config).await.unwrap();
    let mut out = Vec::new();
    assert_eq!(
        cli::run(
            "file",
            &[
                "recent".into(),
                "--json".into(),
                "--node".into(),
                node.url()
            ],
            &mut out
        )
        .await
        .unwrap(),
        0
    );
    let recent: Value = serde_json::from_slice(&out).unwrap();
    assert!(
        recent["files"][0]["path"]
            .as_str()
            .unwrap()
            .ends_with("repo/README.md"),
        "{recent}"
    );
    out.clear();
    assert_eq!(
        cli::run(
            "file",
            &[
                "locate".into(),
                "read".into(),
                "--json".into(),
                "--node".into(),
                node.url()
            ],
            &mut out
        )
        .await
        .unwrap(),
        0
    );
    let found: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(found["matches"][0]["name"], "README.md", "{found}");
    out.clear();
    let dest = project.join("data.bin");
    let bytes = [0, 0xff, 0x80, 42];
    let data = STANDARD.encode(bytes);
    let args = vec![
        "upload".into(),
        dest.to_string_lossy().into_owned(),
        data.clone(),
        "--node".into(),
        node.url(),
    ];
    out.clear();
    assert_eq!(cli::run("file", &args, &mut out).await.unwrap(), 0);
    assert_eq!(std::fs::read(&dest).unwrap(), bytes);
    assert_eq!(
        node.call("file_upload", json!({"path":dest,"data":data}))
            .await
            .unwrap_err()
            .code,
        "already_exists"
    );
    out.clear();
    assert_eq!(
        cli::run(
            "file",
            &[
                "download".into(),
                dest.to_string_lossy().into_owned(),
                "--json".into(),
                "--node".into(),
                node.url()
            ],
            &mut out
        )
        .await
        .unwrap(),
        0
    );
    let downloaded: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        STANDARD
            .decode(downloaded["data"].as_str().unwrap())
            .unwrap(),
        bytes
    );
    let outside = root.parent().unwrap().join("outside-parvi-file");
    assert_eq!(
        node.call("file_upload", json!({"path":outside,"data":data}))
            .await
            .unwrap_err()
            .code,
        "outside_roots"
    );
    node.stop().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn files_work_as_the_files_view_does_under_the_roots() {
    let scratch = parvi_testkit::fresh_dir("files-mcp");
    let folder = scratch.join("root");
    let other = scratch.join("other");
    std::fs::create_dir(&folder).unwrap();
    std::fs::create_dir(&other).unwrap();
    let config = format!(
        "[files]\nroots = [{}]\n",
        serde_json::to_string(folder.to_str().unwrap()).unwrap()
    );
    let node = Node::start(&[parvi_files::LISTING], &config).await.unwrap();
    std::fs::create_dir(folder.join("sub")).unwrap();
    std::fs::write(folder.join("notes.md"), "# Notes\nhello\n").unwrap();
    std::fs::write(folder.join("blob"), [0xff, 0xfe, 0x00]).unwrap();
    std::fs::write(other.join("secret"), "no").unwrap();

    let roots = node.call("file_list", json!({})).await.unwrap();
    assert_eq!(roots["roots"][0]["path"], json!(folder), "{roots}");

    let listing = node
        .call("file_list", json!({ "path": folder }))
        .await
        .unwrap();
    let names: Vec<&str> = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["blob", "notes.md", "sub"], "{listing}");

    let notes = folder.join("notes.md");
    let read = node
        .call("file_read", json!({ "path": notes }))
        .await
        .unwrap();
    assert_eq!(read["text"], "# Notes\nhello\n");
    assert_eq!(read["truncated"], false);
    let version = read["version"].as_str().unwrap().to_owned();

    // A write from the version read lands; a second from the same stale
    // version is refused rather than overwriting the change.
    node.call(
        "file_write",
        json!({ "path": notes, "text": "edited\n", "base": version }),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "edited\n");
    let stale = node
        .call(
            "file_write",
            json!({ "path": notes, "text": "lost\n", "base": version }),
        )
        .await;
    assert!(stale.unwrap_err().message.contains("Changed"));
    let taken = node
        .call(
            "file_write",
            json!({ "path": notes, "text": "x", "new": true }),
        )
        .await;
    assert!(taken.unwrap_err().message.contains("already"));

    node.call("file_mkdir", json!({ "path": folder.join("made") }))
        .await
        .unwrap();
    node.call(
        "file_move",
        json!({ "path": notes, "to": folder.join("made/notes.md") }),
    )
    .await
    .unwrap();
    assert!(!notes.exists() && folder.join("made/notes.md").exists());

    let found = node
        .call("file_find", json!({ "path": folder, "query": "NOTES" }))
        .await
        .unwrap();
    assert_eq!(found["matches"][0]["name"], "notes.md", "{found}");
    let lines = node
        .call("file_search", json!({ "path": folder, "query": "edited" }))
        .await
        .unwrap();
    assert_eq!(lines["matches"][0]["line"], 1, "{lines}");

    node.call("file_delete", json!({ "path": folder.join("made") }))
        .await
        .unwrap();
    assert!(!folder.join("made").exists());

    // The Files view's one rule: outside the roots is refused.
    let outside = node
        .call("file_read", json!({ "path": other.join("secret") }))
        .await;
    assert!(outside.unwrap_err().message.contains("Outside"));
    let binary = node
        .call("file_read", json!({ "path": folder.join("blob") }))
        .await;
    assert!(binary.unwrap_err().message.contains("is not text"));

    // Copy to another folder on this node, through the bridge.
    std::fs::create_dir_all(folder.join("into")).unwrap();
    std::fs::write(folder.join("carried.txt"), "carried").unwrap();
    let copied = node
        .call(
            "file_copy",
            json!({ "path": folder.join("carried.txt"), "to": folder.join("into") }),
        )
        .await
        .unwrap();
    assert_eq!(copied["transfer"]["state"], "done", "{copied}");
    let carried = std::fs::read_to_string(folder.join("into/carried.txt")).unwrap();
    assert_eq!(carried, "carried");
    let followed = node
        .call("file_transfer", json!({ "id": copied["id"] }))
        .await
        .unwrap();
    assert_eq!(followed["transfer"]["state"], "done");
    let again = node
        .call(
            "file_copy",
            json!({ "path": folder.join("carried.txt"), "to": folder.join("into") }),
        )
        .await;
    let refused = again.unwrap_err().message;
    assert!(
        refused.contains("file resume"),
        "never over an existing name: {refused}"
    );
    // Once the name is free, the failed copy resumes by its id alone.
    let id = refused.split("file resume ").nth(1).unwrap();
    let id = id.split_whitespace().next().unwrap().to_owned();
    std::fs::remove_file(folder.join("into/carried.txt")).unwrap();
    let resumed = node.call("file_resume", json!({ "id": id })).await.unwrap();
    assert_eq!(resumed["transfer"]["state"], "done", "{resumed}");
    assert_eq!(
        std::fs::read_to_string(folder.join("into/carried.txt")).unwrap(),
        "carried"
    );
    let unknown = node
        .call("file_resume", json!({ "id": "0".repeat(32) }))
        .await;
    assert!(unknown.unwrap_err().message.contains("no transfer"));
    let nowhere = node
        .call(
            "file_copy",
            json!({ "path": folder.join("carried.txt"), "to": folder, "to_node": "elsewhere" }),
        )
        .await;
    assert_eq!(nowhere.unwrap_err().code, "unknown_node");
    node.stop().await;
    std::fs::remove_dir_all(scratch).unwrap();
}

//! Page state, file requests and user actions.
use crate::{
    open::{self, Draft, Opened, Show},
    store::{self, find::Find, paths, tree::Tree},
};
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, KeyDownEvent, UniformListScrollHandle,
    WeakEntity, Window,
};
use parvi_app_kit::{
    Answers, Ask, Host, Page,
    host::{Request, Status},
    link::client::LinkState,
    menu::MenuRow,
    widgets::{
        code::{Code, CodeEvent},
        field::{FieldEvent, TextField},
        syntax,
    },
};
use parvi_core::NodeId;
use parvi_files_contract::{Changed, Listing, Matches, NodeRoots};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

/// How often a shown page asks whether what it shows changed on disk.
const WATCH: Duration = Duration::from_secs(2);
/// Changes seen on disk ask for recent changes again at most this often.
const RECENT_AGAIN: Duration = Duration::from_secs(60);
/// A pause in typing this long sends the find.
const TYPED: Duration = Duration::from_millis(90);
/// The page the Files panel opens beside.
const PANEL_BESIDE: &str = "terminals";

struct Upload {
    source: PathBuf,
    node: NodeId,
    folder: String,
}
struct UploadConflict {
    upload: Upload,
    name: String,
    bytes: Vec<u8>,
}

/// What the name field is open for, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Naming {
    File(String),
    Folder(String),
    Rename(String),
}
impl Naming {
    pub fn verb(&self) -> &'static str {
        match self {
            Naming::File(_) | Naming::Folder(_) => "Create",
            Naming::Rename(_) => "Rename",
        }
    }
    fn placeholder(&self) -> &'static str {
        match self {
            Naming::File(_) => "Name of the new file",
            Naming::Folder(_) => "Name of the new folder",
            Naming::Rename(_) => "New name",
        }
    }
}

/// The row the keyboard is on in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    Recent(String),
    More,
    Node(String),
    Hit(usize),
    InText,
}

pub struct FilesPage {
    pub host: Host,
    pub focus: FocusHandle,
    pub tree: Tree,
    pub query: Entity<TextField>,
    pub find: Option<Find>,
    find_revision: u64,
    pub pick: Option<Pick>,
    pub recent: Option<Result<Vec<Changed>, String>>,
    pub recent_more: bool,
    recent_asked: bool,
    recent_at: Option<std::time::Instant>,
    pub opened: Option<Opened>,
    pub code: Entity<Code>,
    pub pretty_code: Entity<Code>,
    pub room: crate::picture::Room,
    pending_line: Option<usize>,
    pub drafts: HashMap<(String, String), Draft>,
    pub naming: Option<Naming>,
    pub name_field: Entity<TextField>,
    pub confirm_discard: bool,
    pub conflict: bool,
    copy_ordinal: usize,
    pub confirm_close_beside: bool,
    pub drawn: Vec<std::sync::Arc<gpui::RenderImage>>,
    /// The opened file's header and the room its name has there, and the
    /// list's width, as last laid out: names shorten in the middle to fit.
    pub(crate) head_room: crate::view::Width,
    pub(crate) name_room: crate::view::Width,
    pub(crate) list_room: crate::view::Width,
    pub list_scroll: UniformListScrollHandle,
    /// A row to scroll to once its folder is read.
    scroll_pending: bool,
    /// A path to open the tree down to once the roots are known.
    reveal_pending: Option<String>,
    /// Folders asked for and not answered yet.
    listing: std::collections::HashSet<String>,
    pub beside: bool,
    pub shown: bool,
    beside_view: Option<Entity<FilesPage>>,
    restored: bool,
    pub(crate) beside_from: &'static str,
    me: WeakEntity<FilesPage>,
    pub(crate) sessions: Option<Entity<parvi_terminals_contract::SessionList>>,
    uploads: VecDeque<Upload>,
    uploading: bool,
    upload_conflict: Option<UploadConflict>,
    /// The watch is running, and when each watched path last changed.
    watching: bool,
    seen: HashMap<String, Option<u64>>,
}

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
fn route(kind: &str, path: &str) -> String {
    format!("/api/files/{kind}?path={}", enc(path))
}
fn upload_copy_name(name: &str, ordinal: usize) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
            format!("{stem} ({ordinal}).{ext}")
        }
        _ => format!("{name} ({ordinal})"),
    }
}
fn message(status: u16, body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| {
            format!(
                "Request failed ({status}): {}",
                String::from_utf8_lossy(body)
            )
        })
}
fn code_of(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"]["code"].as_str().map(str::to_owned))
}

impl Page for FilesPage {
    const ID: &'static str = "files";
    const TITLE: &'static str = "Files";
    const KEY: Option<char> = Some('F');
    const AREA: Option<&'static str> = Some("files");
    const ANSWERS: &'static [Answers] = &[Answers::OpenPath, Answers::Reveal("path")];
    const SETTINGS: &'static [parvi_app_kit::Setting] =
        &[
            parvi_app_kit::Setting::text("download_folder", "Download folder", "")
                .note("Leave empty to use Downloads in your home folder."),
        ];
    fn chords(&self, _: &App) -> Vec<parvi_app_kit::chords::Chord> {
        let me = self.me.clone();
        vec![
            parvi_app_kit::chords::Chord::new("o", "Files panel", move |window, cx| {
                let _ = me.update(cx, |page, cx| page.toggle_panel(window, cx));
            })
            .alt("o", Some(false))
            .everywhere(),
        ]
    }
    fn focus(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }
    fn shown(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = on;
        if on {
            self.start(cx);
            self.refresh_recent(cx);
            self.watch(window, cx);
        }
    }
    fn link(
        &mut self,
        node: &NodeId,
        state: Option<LinkState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if state != Some(LinkState::Open) {
            if self.tree.node == node.as_str() {
                self.tree.away = true;
            }
            cx.notify();
            return;
        }
        if let Some(view) = &self.beside_view {
            view.update(cx, |page, cx| {
                page.start(cx);
                if let Some(path) = page
                    .opened
                    .as_ref()
                    .filter(|o| o.node == node.as_str() && matches!(o.show, Show::Failed(_)))
                    .map(|o| o.path.clone())
                {
                    page.open(path, window, cx);
                }
            });
        }
        if self.shown {
            self.start(cx);
            self.refresh_recent(cx);
        }
        if !self.beside && !self.restored {
            self.restored = true;
            let kept: store::Kept = self.host.setting(cx).unwrap_or_default();
            if let Some(file) = kept.file.clone() {
                self.show_in_tree(&file, cx);
                self.open(file, window, cx);
            }
            if kept.beside_open {
                let view = self.panel(window, cx);
                view.update(cx, |p, cx| {
                    p.beside_from = PANEL_BESIDE;
                    if let Some(file) = kept.beside_file {
                        p.open(file, window, cx);
                    }
                    p.watch(window, cx);
                });
                self.host.show_beside(PANEL_BESIDE, view.into(), cx);
            }
        }
    }
    fn answer(
        &mut self,
        ask: Ask,
        from: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ask {
            Ask::OpenPath { node, path, beside } => {
                if beside {
                    self.open_beside(from, node, path, window, cx);
                } else {
                    self.on_node(node.as_str(), cx);
                    self.show_in_tree(&path, cx);
                    self.open(path, window, cx)
                }
            }
            Ask::Reveal { what, id, node, .. } if what == "path" => {
                if let Some(n) = node {
                    self.on_node(n.as_str(), cx);
                }
                self.reveal(id, window, cx);
            }
            _ => {}
        }
    }
}

impl FilesPage {
    pub fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let kept: store::Kept = host.setting(cx).unwrap_or_default();
        let sessions = host.model::<parvi_terminals_contract::SessionList>(cx);
        if let Some(sessions) = &sessions {
            cx.observe(sessions, |_, _, cx| cx.notify()).detach();
        }
        let wrap = !kept.unwrapped;
        let code = cx.new(|cx| {
            let mut c = Code::new(cx);
            c.set_wrap(wrap, cx);
            c
        });
        let pretty_code = cx.new(|cx| {
            let mut c = Code::new(cx);
            c.set_wrap(wrap, cx);
            c
        });
        let query = cx.new(|cx| TextField::new(cx, "Find a file"));
        let name_field = cx.new(|cx| TextField::new(cx, "New name"));
        cx.subscribe_in(
            &code,
            window,
            move |page, _, event, window, cx| match event {
                CodeEvent::Save => page.save(cx),
                CodeEvent::Changed => page.changed(cx),
                CodeEvent::Escape => page.finish_edit(window, cx),
            },
        )
        .detach();
        cx.subscribe_in(&query, window, move |page, field, event, window, cx| {
            match event {
                FieldEvent::Changed => {
                    let q = field.read(cx).text.clone();
                    page.typed(q, cx);
                }
                FieldEvent::Submit => {
                    page.activate(window, cx);
                }
                FieldEvent::Down => page.step(1, cx),
                FieldEvent::Up => page.step(-1, cx),
                _ => {}
            }
            cx.notify();
        })
        .detach();
        cx.subscribe_in(&name_field, window, move |page, _, event, window, cx| {
            if matches!(event, FieldEvent::Submit) {
                page.apply_naming(window, cx);
            }
        })
        .detach();
        host.guard_close(
            &cx.entity(),
            |page: &FilesPage, _| {
                let mut names: Vec<_> = page
                    .drafts
                    .keys()
                    .map(|(_, path)| paths::base_name(path).to_owned())
                    .collect();
                names.sort();
                names.dedup();
                (!names.is_empty()).then(|| names.join(", "))
            },
            cx,
        );
        Self {
            focus: cx.focus_handle().tab_stop(true),
            host,
            tree: Tree::from_kept(&kept.tree),
            query,
            find: None,
            find_revision: 0,
            pick: None,
            recent: None,
            recent_more: false,
            recent_asked: false,
            recent_at: None,
            opened: None,
            code,
            pretty_code,
            room: Default::default(),
            pending_line: None,
            drafts: HashMap::new(),
            naming: None,
            name_field,
            confirm_discard: false,
            conflict: false,
            copy_ordinal: 1,
            confirm_close_beside: false,
            drawn: Vec::new(),
            head_room: Default::default(),
            name_room: Default::default(),
            list_room: Default::default(),
            list_scroll: UniformListScrollHandle::new(),
            scroll_pending: false,
            reveal_pending: None,
            listing: Default::default(),
            beside: false,
            shown: false,
            beside_view: None,
            restored: false,
            beside_from: "",
            me: cx.weak_entity(),
            sessions,
            uploads: VecDeque::new(),
            uploading: false,
            upload_conflict: None,
            watching: false,
            seen: HashMap::new(),
        }
    }

    fn fail(&self, text: String, cx: &mut Context<Self>) {
        self.host.report(Status::failure(text), cx);
    }

    /// The Files panel (Alt+O): this page's view beside the terminals,
    /// at the focused session's folder, or away again. It is the same view
    /// a printed path opens, over the windows, so no session changes size.
    fn toggle_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.beside_view.clone().filter(|v| {
            self.host
                .beside_of(PANEL_BESIDE, cx)
                .is_some_and(|b| b.entity_id() == v.entity_id())
        }) {
            view.update(cx, |p, cx| p.close_beside(cx));
            return;
        }
        let folder = self.session_folder(cx);
        let view = self.panel(window, cx);
        view.update(cx, |p, cx| {
            p.beside_from = PANEL_BESIDE;
            if let Some((node, folder)) = folder {
                p.on_node(&node, cx);
                p.opened = None;
                p.show_folder(folder, cx);
            }
            p.refresh_recent(cx);
            p.watch(window, cx);
        });
        self.keep_beside_open(true, cx);
        self.host.show_beside(PANEL_BESIDE, view.into(), cx);
        self.host.for_page(PANEL_BESIDE, &[]).show(cx);
    }

    /// The view this page opens beside another, made the first time.
    fn panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<FilesPage> {
        if let Some(view) = &self.beside_view {
            return view.clone();
        }
        let host = self.host.clone();
        let kept: store::Kept = host.setting(cx).unwrap_or_default();
        let view = cx.new(|cx| FilesPage::new(host, window, cx));
        view.update(cx, |p, cx| {
            p.beside = true;
            p.tree = Tree::from_kept(&kept.beside);
            p.start(cx);
            p.refresh_recent(cx);
        });
        self.beside_view = Some(view.clone());
        view
    }

    /// Whether the panel beside the terminals is open, kept for the next
    /// run.
    fn keep_beside_open(&self, open: bool, cx: &mut Context<Self>) {
        let mut kept: store::Kept = self.host.setting(cx).unwrap_or_default();
        if kept.beside_open != open {
            kept.beside_open = open;
            let _ = self.host.set_setting(&kept, cx);
        }
    }

    /// The node and folder of the session in the focused terminal tile.
    fn session_folder(&self, cx: &App) -> Option<(String, String)> {
        let list = self.sessions.as_ref()?.read(cx);
        let focused = list.focused.as_ref()?;
        list.sessions
            .iter()
            .find(|s| {
                s.node == focused.node
                    && s.name == focused.name
                    && s.user.as_deref().unwrap_or_default() == focused.user
            })
            .and_then(|s| Some((s.node.clone(), s.cwd.clone()?)))
    }
    fn open_beside(
        &mut self,
        from: &'static str,
        node: NodeId,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = self.panel(window, cx);
        view.update(cx, |p, cx| {
            p.beside_from = from;
            p.on_node(node.as_str(), cx);
            p.show_in_tree(&path, cx);
            p.open(path, window, cx);
            p.watch(window, cx);
        });
        if from == PANEL_BESIDE {
            self.keep_beside_open(true, cx);
        }
        self.host.show_beside(from, view.into(), cx);
    }
    pub fn close_beside(&mut self, cx: &mut Context<Self>) {
        if !self.drafts.is_empty() {
            self.confirm_close_beside = true;
            cx.notify();
            return;
        }
        self.close_beside_now(cx);
    }
    pub fn cancel_close_beside(&mut self, cx: &mut Context<Self>) {
        self.confirm_close_beside = false;
        cx.notify();
    }
    pub fn discard_and_close_beside(&mut self, cx: &mut Context<Self>) {
        self.drafts.clear();
        self.confirm_close_beside = false;
        self.close_beside_now(cx);
    }
    fn close_beside_now(&self, cx: &mut Context<Self>) {
        if self.beside_from == PANEL_BESIDE {
            self.keep_beside_open(false, cx);
        }
        self.host.close_beside(self.beside_from, cx);
    }
    fn request(
        &self,
        r: Request,
        done: impl FnOnce(&mut Self, u16, Vec<u8>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let task = self.host.request(r, cx);
        cx.spawn(async move |this, cx| {
            let (s, b) = match task.await {
                Ok(a) => (a.status, a.body),
                Err(e) => (0, e.message.into_bytes()),
            };
            let _ = this.update(cx, |p, cx| done(p, s, b, cx));
        })
        .detach();
    }
    fn node(&self) -> NodeId {
        NodeId(self.tree.node.clone())
    }
    /// Shows `node`'s files from here on.
    fn on_node(&mut self, node: &str, cx: &mut Context<Self>) {
        if self.tree.node != node {
            self.tree.choose_node(node);
            self.recent = None;
            self.roots(cx);
            self.refresh_recent(cx);
        }
    }
    pub fn start(&mut self, cx: &mut Context<Self>) {
        let Some(node) = self.host.nodes(cx).first().map(|n| n.id.to_string()) else {
            return;
        };
        if self.tree.node.is_empty() {
            self.tree.node = node;
        }
        self.roots(cx);
    }
    fn roots(&mut self, cx: &mut Context<Self>) {
        let node = self.tree.node.clone();
        self.request(
            Request::get(NodeId(node.clone()), "/api/files/roots"),
            move |p, s, b, cx| {
                if p.tree.node != node {
                    return;
                }
                if s != 200 {
                    p.tree.away = true;
                    p.fail(message(s, &b), cx);
                    cx.notify();
                    return;
                }
                match serde_json::from_slice::<NodeRoots>(&b) {
                    Ok(r) => {
                        p.tree.rooted(r.roots);
                        if let Some(path) = p.reveal_pending.take() {
                            p.tree.reveal(&path);
                            p.scroll_pending = true;
                        }
                        p.read_unread(cx);
                    }
                    Err(e) => p.fail(e.to_string(), cx),
                }
                cx.notify()
            },
            cx,
        )
    }
    /// Reads every open folder not yet read.
    fn read_unread(&mut self, cx: &mut Context<Self>) {
        for folder in self.tree.unread() {
            if !self.listing.contains(&folder) {
                self.list(folder, cx);
            }
        }
    }
    pub fn refresh_recent(&mut self, cx: &mut Context<Self>) {
        if self.recent_asked || self.tree.node.is_empty() {
            return;
        }
        self.recent_asked = true;
        self.recent_at = Some(std::time::Instant::now());
        let node = self.tree.node.clone();
        self.request(
            Request::get(NodeId(node.clone()), "/api/files/recent"),
            move |p, s, b, cx| {
                p.recent_asked = false;
                if p.tree.node != node {
                    return;
                }
                // The first Recent lands above the tree: a row picked in
                // the tree stays in view.
                let first = p.recent.is_none();
                p.recent = Some(if s == 200 {
                    serde_json::from_slice::<parvi_files_contract::Recent>(&b)
                        .map(|r| r.files)
                        .map_err(|e| e.to_string())
                } else {
                    Err(message(s, &b))
                });
                if first && matches!(p.pick, Some(Pick::Node(_))) {
                    p.scroll_to_pick();
                }
                cx.notify();
            },
            cx,
        );
    }
    /// A path asked for from elsewhere: a folder opens in the tree, a file
    /// opens.
    fn reveal(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let task = self
            .host
            .request(Request::get(self.node(), route("folder", &path)), cx);
        cx.spawn_in(window, async move |this, cx| {
            let answer = task.await;
            let _ = this.update_in(cx, |page, window, cx| {
                match answer {
                    Ok(answer) if answer.status == 200 => page.show_folder(path, cx),
                    Ok(answer)
                        if code_of(&answer.body).as_deref()
                            == Some(parvi_files_contract::NOT_FOLDER) =>
                    {
                        page.show_in_tree(&path, cx);
                        page.open(path, window, cx);
                    }
                    Ok(answer) => page.fail(message(answer.status, &answer.body), cx),
                    Err(error) => page.fail(error.message, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
    /// Opens the tree down to `folder`, opens it and picks it.
    pub fn show_folder(&mut self, folder: String, cx: &mut Context<Self>) {
        self.clear_find(cx);
        self.reveal_in_tree(&paths::join(&folder, "x"), cx);
        self.tree.select(&folder);
        self.pick = Some(Pick::Node(folder));
        self.scroll_pending = !self.scroll_to_pick();
        self.keep(cx);
        cx.notify();
    }
    /// Beside another page: from the file back to the list, the file
    /// picked in it.
    pub fn back_to_list(&mut self, cx: &mut Context<Self>) {
        if let Some(o) = self.opened.take() {
            self.pick = Some(Pick::Node(o.path));
            self.scroll_pending = !self.scroll_to_pick();
        }
        self.keep(cx);
        cx.notify();
    }
    /// Opens the tree down to `path` and picks its row: what was asked
    /// for from elsewhere shows where it is.
    pub fn show_in_tree(&mut self, path: &str, cx: &mut Context<Self>) {
        self.clear_find(cx);
        self.reveal_in_tree(path, cx);
        self.set_pick(Pick::Node(path.to_owned()), cx);
        self.scroll_pending = !self.scroll_to_pick();
    }
    /// Opens the folders down to `path`, now or once the roots are known.
    fn reveal_in_tree(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.tree.roots.is_empty() {
            self.reveal_pending = Some(path.to_owned());
            return;
        }
        for unread in self.tree.reveal(path) {
            self.list(unread, cx);
        }
    }
    pub fn list(&mut self, path: String, cx: &mut Context<Self>) {
        let node = self.tree.node.clone();
        self.listing.insert(path.clone());
        self.request(
            Request::get(NodeId(node.clone()), route("folder", &path)),
            move |p, s, b, cx| {
                p.listing.remove(&path);
                if p.tree.node != node {
                    return;
                }
                if s == 200 {
                    match serde_json::from_slice::<Listing>(&b) {
                        Ok(l) => p.tree.listed(path.clone(), l.entries),
                        Err(e) => p.tree.failed(path.clone(), e.to_string()),
                    }
                    // Folders kept or asked open inside it show now: read them.
                    p.read_unread(cx);
                    // Listings landing above the pick move it: follow it
                    // until every folder asked for has answered.
                    if p.scroll_pending && p.scroll_to_pick() && p.listing.is_empty() {
                        p.scroll_pending = false;
                    }
                } else if s == 0 || s == 421 || s == 502 || s == 503 {
                    p.tree.away = true;
                } else {
                    if code_of(&b).as_deref() == Some(parvi_files_contract::NOT_FOUND) {
                        p.tree.open.remove(&path);
                        p.tree.listings.remove(&path);
                    }
                    p.tree.failed(path.clone(), message(s, &b));
                }
                cx.notify()
            },
            cx,
        )
    }

    /// The query changed: a find of names across every root, answered as
    /// you type.
    fn typed(&mut self, query: String, cx: &mut Context<Self>) {
        self.find_revision += 1;
        if query.trim().is_empty() {
            self.find = None;
            self.pick = None;
            return;
        }
        let revision = self.find_revision;
        self.find = Some(Find::new(&query, revision));
        self.pick = Some(Pick::Hit(0));
        let node = self.tree.node.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TYPED).await;
            let _ = this.update(cx, |page, cx| {
                if page.find_revision == revision {
                    let uri = format!("/api/files/find?q={}", enc(&query));
                    page.ask_find(Request::get(NodeId(node), uri), revision, false, cx);
                }
            });
        })
        .detach();
    }
    fn ask_find(&mut self, request: Request, revision: u64, inside: bool, cx: &mut Context<Self>) {
        self.request(
            request,
            move |p, s, b, cx| {
                let found = if s == 200 {
                    serde_json::from_slice::<Matches>(&b).map_err(|e| e.to_string())
                } else {
                    Err(message(s, &b))
                };
                if let Some(find) = p.find.as_mut() {
                    if inside {
                        find.answer_inside(revision, found);
                    } else {
                        find.answer(revision, found);
                    }
                }
                cx.notify()
            },
            cx,
        );
    }
    /// The folder whose files' text a find searches: the picked one, or
    /// the picked file's.
    pub fn text_scope(&self) -> Option<String> {
        self.tree.folder_here()
    }
    /// Looks for the query inside the files under the text scope; the
    /// names found stay above.
    pub fn find_in_text(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.text_scope() else {
            return;
        };
        let Some(find) = self.find.as_mut() else {
            return;
        };
        find.inside = Some(store::find::Inside {
            folder: folder.clone(),
            found: None,
        });
        let (query, revision) = (find.query.clone(), find.revision);
        let first = find.matches().len();
        self.pick = Some(Pick::Hit(first));
        let uri = format!("{}&q={}", route("search", &folder), enc(&query));
        self.ask_find(Request::get(self.node(), uri), revision, true, cx);
        cx.notify();
    }
    fn clear_find(&mut self, cx: &mut Context<Self>) {
        if self.find.is_some() || !self.query.read(cx).text.is_empty() {
            self.query.update(cx, |field, cx| field.set("", false, cx));
            self.find = None;
            self.find_revision += 1;
        }
    }

    /// The rows the keyboard steps through, as the list shows them.
    pub fn picks(&self) -> Vec<Pick> {
        crate::view::lines(self)
            .into_iter()
            .filter_map(|line| line.pick())
            .collect()
    }
    pub fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let picks = self.picks();
        if picks.is_empty() {
            return;
        }
        let at = self
            .pick
            .as_ref()
            .and_then(|p| picks.iter().position(|q| q == p));
        let last = picks.len() - 1;
        let next = match (at, by) {
            (_, isize::MAX) | (None, isize::MIN..0) => last,
            (_, isize::MIN) | (None, _) => 0,
            (Some(i), by) => (i as isize).saturating_add(by).clamp(0, last as isize) as usize,
        };
        self.set_pick(picks[next].clone(), cx);
    }
    pub fn set_pick(&mut self, pick: Pick, cx: &mut Context<Self>) {
        if let Pick::Node(path) = &pick {
            self.tree.select(path);
            self.keep(cx);
        }
        self.pick = Some(pick);
        self.scroll_to_pick();
        cx.notify();
    }
    /// Scrolls the list to the picked row; false while it is not there yet.
    fn scroll_to_pick(&self) -> bool {
        let Some(pick) = &self.pick else { return false };
        let Some(at) = crate::view::lines(self)
            .iter()
            .position(|l| l.pick().as_ref() == Some(pick))
        else {
            return false;
        };
        self.list_scroll
            .scroll_to_item(at, gpui::ScrollStrategy::Nearest);
        true
    }
    /// Enter on the picked row, or a click on it.
    pub fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pick = match &self.pick {
            Some(p) => p.clone(),
            None if self.find.is_some() => Pick::Hit(0),
            None => return,
        };
        match pick {
            Pick::Recent(path) => {
                let folder = matches!(&self.recent, Some(Ok(r)) if r.iter().any(|c| c.path == path && c.files.is_some()));
                if folder {
                    self.show_folder(path, cx);
                } else {
                    self.open(path, window, cx)
                }
            }
            Pick::More => {
                self.recent_more = true;
                cx.notify();
            }
            Pick::Node(path) => match self.tree.row(&path) {
                Some(row) if row.folder => {
                    if self.tree.toggle(&path) {
                        self.list(path, cx);
                    }
                    self.keep(cx);
                }
                Some(_) => self.open(path, window, cx),
                None => {}
            },
            Pick::Hit(i) => {
                let hit = self.find.as_ref().and_then(|f| f.matches().get(i).cloned());
                if let Some(hit) = hit {
                    self.open_at(hit.path, hit.line, window, cx);
                }
            }
            Pick::InText => self.find_in_text(cx),
        }
        cx.notify();
    }

    pub fn open_at(
        &mut self,
        path: String,
        line: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(path, window, cx);
        self.pending_line = line;
    }
    /// Opens a file in the viewer, and shows it in the tree.
    pub fn open(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_line = None;
        self.confirm_discard = false;
        self.conflict = false;
        self.code
            .update(cx, |code, cx| code.edit(false, window, cx));
        let node = self.tree.node.clone();
        let name = paths::base_name(&path).to_string();
        self.seen.remove(&path);
        self.opened = Some(Opened::reading(node.clone(), path.clone()));
        self.tree.select(&path);
        self.keep(cx);
        let kind = store::kinds::kind(&name);
        if matches!(
            kind,
            store::kinds::Kind::Audio | store::kinds::Kind::Video | store::kinds::Kind::Pdf
        ) {
            self.opened.as_mut().unwrap().show = Show::Media(kind);
            cx.notify();
            return;
        }
        self.read(node, path, false, cx);
        cx.notify();
    }
    /// Reads the open file; `again` keeps the place read and any edit.
    fn read(&mut self, node: String, path: String, again: bool, cx: &mut Context<Self>) {
        let name = paths::base_name(&path).to_string();
        self.request(
            Request::get(NodeId(node.clone()), route("file", &path)),
            move |p, s, b, cx| {
                if !p
                    .opened
                    .as_ref()
                    .is_some_and(|o| o.node == node && o.path == path)
                {
                    return;
                }
                if s != 200 {
                    if code_of(&b).as_deref() == Some(parvi_files_contract::NOT_FILE) {
                        p.opened = None;
                        p.show_folder(path, cx);
                        return;
                    }
                    if let Some(o) = p.opened.as_mut() {
                        o.show = Show::Failed(message(s, &b));
                    }
                    cx.notify();
                    return;
                }
                p.opened.as_mut().unwrap().size = Some(b.len() as u64);
                if store::kinds::kind(&name) == store::kinds::Kind::Model {
                    p.decode(node, path, cx, move || {
                        crate::model::Model::load(&b)
                            .map(crate::model::Preview::new)
                            .map(Show::Model)
                    });
                    return;
                }
                if let Some(format) = open::picture_format(&name) {
                    let svg = cx.svg_renderer();
                    p.decode(node, path, cx, move || {
                        let image = gpui::Image::from_bytes(format, b)
                            .to_image_data(svg)
                            .map_err(|e| e.to_string())?;
                        let dim = image.size(0);
                        Ok(Show::Picture {
                            natural: store::zoom::Px {
                                w: dim.width.0 as f32,
                                h: dim.height.0 as f32,
                            },
                            image,
                        })
                    });
                    return;
                }
                let (show, text, pretty) = open::show(&name, &b);
                let version = open::version(&b);
                let key = (node.clone(), path.clone());
                let o = p.opened.as_mut().unwrap();
                if again && o.base == version {
                    return;
                }
                let draft = p.drafts.get(&key).cloned();
                if again && draft.is_some() {
                    // Theirs changed under an edit: Save will ask.
                    o.changed_on_disk = true;
                    cx.notify();
                    return;
                }
                let is_text = matches!(show, Show::Text { .. });
                o.show = show;
                o.saved = text.clone();
                o.base = draft.as_ref().map_or(version, |d| d.base.clone());
                o.pretty = pretty.clone();
                o.changed_on_disk = false;
                let lang = syntax::lang(&name);
                let line = p.pending_line.take();
                let log = store::kinds::extension(&name) == "log";
                p.pretty_code.update(cx, |code, cx| {
                    code.set_lang(lang, cx);
                    if again {
                        code.reload(pretty.as_deref().unwrap_or(&text), cx);
                    } else {
                        code.set(pretty.as_deref().unwrap_or(&text), cx);
                    }
                });
                p.code.update(cx, |code, cx| {
                    code.set_lang(lang, cx);
                    let shown = draft.as_ref().map_or(&text, |d| &d.text);
                    if again {
                        code.reload(shown, cx);
                    } else {
                        code.set(shown, cx);
                        if let Some(line) = line {
                            code.go_to_line(line.saturating_sub(1), cx);
                        } else if log {
                            code.show_end(cx);
                        }
                    }
                });
                if line.is_some()
                    && let Some(o) = p.opened.as_mut()
                {
                    o.source = true;
                }
                if is_text && !again {
                    p.read_changes(cx);
                }
                cx.notify()
            },
            cx,
        )
    }
    /// Decodes off the UI thread what a file shows as.
    fn decode(
        &mut self,
        node: String,
        path: String,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> Result<Show, String> + Send + 'static,
    ) {
        let task = cx.background_executor().spawn(async move { work() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |p, cx| {
                if let Some(o) = p
                    .opened
                    .as_mut()
                    .filter(|o| o.node == node && o.path == path)
                {
                    o.show = result.unwrap_or_else(Show::Failed);
                    cx.notify();
                }
            });
        })
        .detach();
    }
    /// What the open file's repository says changed in it, so Changes is
    /// offered only when there are some.
    fn read_changes(&mut self, cx: &mut Context<Self>) {
        let Some(o) = self.opened.as_ref() else {
            return;
        };
        let node = o.node.clone();
        let path = o.path.clone();
        self.request(
            Request::get(NodeId(node.clone()), route("diff", &path)),
            move |page, status, body, cx| {
                let Some(o) = page
                    .opened
                    .as_mut()
                    .filter(|o| o.node == node && o.path == path)
                else {
                    return;
                };
                o.diff = (status == 200)
                    .then(|| serde_json::from_slice::<parvi_files_contract::Diff>(&body).ok())
                    .flatten()
                    .filter(|d| d.error.is_none() && !d.diff.trim().is_empty());
                if o.diff.is_none() {
                    o.diff_open = false;
                }
                cx.notify();
            },
            cx,
        );
    }
    pub fn toggle_diff(&mut self, cx: &mut Context<Self>) {
        if let Some(o) = self.opened.as_mut() {
            o.diff_open = !o.diff_open && o.diff.is_some();
        }
        cx.notify();
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        let Some(o) = self.opened.as_ref() else {
            return;
        };
        let text = self.code.read(cx).text().to_owned();
        if text != o.saved {
            self.drafts.insert(
                o.key(),
                Draft {
                    text,
                    base: o.base.clone(),
                },
            );
        } else {
            self.drafts.remove(&o.key());
        }
        cx.notify()
    }
    pub fn edit(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if let Some(o) = self.opened.as_mut().filter(|o| o.editable()) {
            o.editing = true;
            o.diff_open = false;
            self.code.update(cx, |c, cx| c.edit(true, w, cx));
        }
        cx.notify()
    }
    fn finish_edit(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.opened.as_ref().is_some_and(|o| !o.editing) {
            if self.beside {
                self.close_beside(cx);
            } else {
                w.focus(&self.focus, cx);
            }
            cx.notify();
            return;
        }
        let dirty = self
            .opened
            .as_ref()
            .is_some_and(|o| self.drafts.contains_key(&o.key()));
        if dirty {
            self.confirm_discard = true;
        } else if let Some(o) = self.opened.as_mut() {
            o.editing = false;
            self.code.update(cx, |c, cx| c.edit(false, w, cx));
        }
        cx.notify()
    }
    pub fn keep_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.confirm_discard) {
            Code::focus(&self.code, window, cx);
        }
        cx.notify();
    }
    pub fn save(&mut self, cx: &mut Context<Self>) {
        self.write(open::Write::Save, cx);
    }
    pub fn cancel_conflict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.conflict) {
            Code::focus(&self.code, window, cx)
        }
        cx.notify();
    }
    pub fn write(&mut self, mode: open::Write, cx: &mut Context<Self>) {
        let Some(opened) = self.opened.as_mut() else {
            return;
        };
        if opened.saving {
            return;
        }
        opened.saving = true;
        let node = opened.node.clone();
        let path = opened.path.clone();
        let base = opened.base.clone();
        let name = opened.name.clone();
        let folder = paths::parent(&path);
        let sent = self.code.read(cx).text().to_owned();
        let taken: Vec<String> = self
            .tree
            .listings
            .get(&folder)
            .map(|l| l.iter().map(|e| e.name.clone()).collect())
            .unwrap_or_default();
        let (target, ordinal) = if mode == open::Write::Copy {
            let mut n = self.copy_ordinal;
            while taken.contains(&open::copy_name(&name, n)) {
                n += 1;
            }
            (paths::join(&folder, &open::copy_name(&name, n)), n)
        } else {
            (path.clone(), 0)
        };
        let (method, uri, success) = match mode {
            open::Write::Save => (
                "PUT",
                format!("{}&base={}", route("file", &target), enc(&base)),
                204,
            ),
            open::Write::Overwrite => ("PUT", route("file", &target), 204),
            open::Write::Copy => ("POST", route("file", &target), 201),
        };
        let mut request = Request::new(method, NodeId(node.clone()), uri);
        request.body = Some(("application/octet-stream".into(), sent.as_bytes().to_vec()));
        self.request(
            request,
            move |p, status, body, cx| {
                if !p
                    .opened
                    .as_ref()
                    .is_some_and(|o| o.node == node && o.path == path)
                {
                    return;
                }
                if status != success {
                    p.opened.as_mut().unwrap().saving = false;
                    if status == 412 {
                        p.conflict = true;
                        p.copy_ordinal = 1;
                    } else if status == 409 && mode == open::Write::Copy {
                        p.copy_ordinal = ordinal + 1;
                        p.write(mode, cx);
                    } else {
                        p.fail(message(status, &body), cx)
                    }
                } else {
                    p.conflict = false;
                    p.host
                        .toast(&format!("Saved {}", paths::base_name(&target)), cx);
                    let now = p.code.read(cx).text().to_owned();
                    let o = p.opened.as_mut().unwrap();
                    let old_key = o.key();
                    o.saving = false;
                    o.changed_on_disk = false;
                    if mode == open::Write::Copy {
                        o.path = target.clone();
                        o.name = paths::base_name(&target).to_owned();
                    }
                    o.saved = sent.clone();
                    if matches!(o.show, Show::Text { markdown: Some(_) }) {
                        o.show = Show::Text {
                            markdown: Some(parvi_app_kit::widgets::markdown::parse(&sent)),
                        };
                    }
                    o.pretty = open::pretty(&o.name, &sent);
                    o.base = open::version(sent.as_bytes());
                    let new_key = o.key();
                    let pretty = o.pretty.clone();
                    // Our own write is not a change to show.
                    let opened_path = o.path.clone();
                    p.seen.remove(&opened_path);
                    p.seen.remove(&folder);
                    p.pretty_code.update(cx, |code, cx| {
                        code.reload(pretty.as_deref().unwrap_or(&sent), cx)
                    });
                    p.drafts.remove(&old_key);
                    if let Some(d) = open::acknowledged(&now, &sent) {
                        p.drafts.insert(new_key, d);
                    }
                    if p.tree.listings.contains_key(&folder) {
                        p.list(folder.clone(), cx);
                    }
                    p.read_changes(cx);
                }
                cx.notify();
            },
            cx,
        )
    }
    pub fn search_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if e.keystroke.key == "escape" && self.query.read(cx).is_focused(window) {
            self.clear_find(cx);
            self.pick = None;
            window.focus(&self.focus, cx);
            cx.stop_propagation();
            cx.notify();
        }
    }
    /// The list's keys.
    pub fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        let key = &e.keystroke;
        let m = key.modifiers;
        if m.alt || m.platform || (m.control && key.key != "c") {
            return;
        }
        match key.key.as_str() {
            "up" | "k" => self.step(-1, cx),
            "down" | "j" => self.step(1, cx),
            "home" => self.step(isize::MIN, cx),
            "end" => self.step(isize::MAX, cx),
            "left" | "h" if matches!(self.pick, Some(Pick::Node(_))) => {
                self.tree.left();
                let path = self.tree.selected.clone().unwrap_or_default();
                self.set_pick(Pick::Node(path), cx);
            }
            "right" | "l" if matches!(self.pick, Some(Pick::Node(_))) => {
                if self.tree.right()
                    && let Some(path) = self.tree.selected.clone()
                {
                    self.list(path, cx);
                }
                let path = self.tree.selected.clone().unwrap_or_default();
                self.set_pick(Pick::Node(path), cx);
            }
            "enter" => self.activate(window, cx),
            "f5" => {
                for folder in self.tree.open_folders() {
                    self.list(folder, cx);
                }
                self.refresh_recent(cx);
            }
            "/" => parvi_app_kit::widgets::field::focus(&self.query, window, cx),
            "f2" => self.start_rename(window, cx),
            // Beside another page, Esc takes the view away (its drafts are
            // asked about first).
            "escape" if self.beside && self.find.is_none() => {
                self.close_beside(cx);
            }
            "escape" => {
                if self.find.is_some() {
                    self.clear_find(cx);
                    self.pick = None;
                } else {
                    return;
                }
            }
            "c" if m.control && m.shift => {
                if let Some(path) = self.picked_path() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(path))
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
    /// The path of the picked row.
    pub fn picked_path(&self) -> Option<String> {
        match self.pick.as_ref()? {
            Pick::Recent(p) | Pick::Node(p) => Some(p.clone()),
            Pick::Hit(i) => self
                .find
                .as_ref()
                .and_then(|f| f.matches().get(*i).map(|m| m.path.clone())),
            Pick::More | Pick::InText => None,
        }
    }
    pub fn discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard = false;
        if let Some(opened) = self.opened.as_mut() {
            self.drafts.remove(&opened.key());
            let saved = opened.saved.clone();
            let stale = std::mem::take(&mut opened.changed_on_disk);
            opened.editing = false;
            let (node, path) = (opened.node.clone(), opened.path.clone());
            self.code.update(cx, |code, cx| {
                code.reload(&saved, cx);
                code.edit(false, window, cx)
            });
            if stale {
                // What is on disk now, not what the edit began from.
                self.read(node, path, true, cx);
            }
        }
        cx.notify();
    }

    /// Opens the name field for a new file or folder in `folder`.
    pub fn start_naming(&mut self, naming: Naming, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tree.writable() {
            return;
        }
        let text = match &naming {
            Naming::Rename(path) => paths::base_name(path).to_owned(),
            _ => String::new(),
        };
        self.name_field.update(cx, |field, cx| {
            field.set_placeholder(naming.placeholder());
            field.set(&text, true, cx)
        });
        self.naming = Some(naming);
        parvi_app_kit::widgets::field::focus(&self.name_field, window, cx);
        cx.notify();
    }
    pub fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self
            .picked_path()
            .filter(|p| self.tree.row(p).is_some_and(|r| r.depth > 0))
        {
            self.start_naming(Naming::Rename(path), window, cx);
        }
    }
    pub fn stop_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.naming = None;
        self.name_field
            .update(cx, |field, cx| field.set("", false, cx));
        window.focus(&self.focus, cx);
        cx.notify();
    }
    pub fn apply_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(naming) = self.naming.clone() else {
            return;
        };
        let name = self.name_field.read(cx).text.trim().to_owned();
        if let Some(reason) = paths::bad_name(&name) {
            self.fail(reason.into(), cx);
            return;
        }
        let node = self.tree.node.clone();
        let (folder, uri, made, moved) = match &naming {
            Naming::File(folder) => {
                let to = paths::join(folder, &name);
                (folder.clone(), route("file", &to), to, None)
            }
            Naming::Folder(folder) => {
                let to = paths::join(folder, &name);
                (folder.clone(), route("folder", &to), to, None)
            }
            Naming::Rename(from) => {
                let folder = paths::parent(from);
                let to = paths::join(&folder, &name);
                let uri = format!("{}&to={}", route("rename", from), enc(&to));
                (folder, uri, to.clone(), Some((from.clone(), to)))
            }
        };
        let file = matches!(naming, Naming::File(_));
        let task = self
            .host
            .request(Request::new("POST", NodeId(node.clone()), uri), cx);
        cx.spawn_in(window, async move |this, cx| {
            let (s, b) = match task.await {
                Ok(a) => (a.status, a.body),
                Err(e) => (0, e.message.into_bytes()),
            };
            let _ = this.update_in(cx, |p, window, cx| {
                if !(200..300).contains(&s) {
                    p.fail(message(s, &b), cx);
                    cx.notify();
                    return;
                }
                if let Some((from, to)) = &moved {
                    store::drafts::move_drafts(&mut p.drafts, &node, from, to);
                    if let Some(opened) = p
                        .opened
                        .as_mut()
                        .filter(|o| o.node == node && paths::inside(from, &o.path))
                    {
                        opened.path = format!("{to}{}", &opened.path[from.len()..]);
                        opened.name = paths::base_name(&opened.path).to_owned();
                    }
                    if p.tree.open.remove(from) {
                        p.tree.open.insert(to.clone());
                    }
                }
                p.naming = None;
                p.name_field
                    .update(cx, |field, cx| field.set("", false, cx));
                p.tree.open.insert(folder.clone());
                p.list(folder.clone(), cx);
                p.tree.select(&made);
                p.pick = Some(Pick::Node(made.clone()));
                p.keep(cx);
                // A new file opens, to be written.
                if file {
                    p.open(made.clone(), window, cx);
                }
                cx.notify();
            });
        })
        .detach();
        window.focus(&self.focus, cx);
    }
    pub fn delete(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.tree.writable() {
            return;
        }
        let node = self.tree.node.clone();
        self.request(
            Request::new("DELETE", NodeId(node.clone()), route("file", &path)),
            move |p, s, b, cx| {
                if !(200..300).contains(&s) {
                    p.fail(message(s, &b), cx);
                } else {
                    if p.opened
                        .as_ref()
                        .is_some_and(|o| o.node == node && paths::inside(&path, &o.path))
                    {
                        p.opened = None;
                    }
                    p.tree.open.remove(&path);
                    p.list(paths::parent(&path), cx);
                    p.refresh_recent(cx);
                }
                cx.notify()
            },
            cx,
        );
    }
    pub(crate) fn pick_uploads(
        &mut self,
        folder: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let node = self.node();
        parvi_app_kit::widgets::path_picker::pick(
            self.host.clone(),
            "Upload files",
            window,
            cx,
            move |page, paths, cx| page.queue_uploads(node, folder, &paths, cx),
        );
    }
    pub(crate) fn upload_paths(
        &mut self,
        folder: String,
        paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        let node = self.node();
        self.queue_uploads(node, folder, paths, cx);
    }
    fn queue_uploads(
        &mut self,
        node: NodeId,
        folder: String,
        paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        self.uploads
            .extend(paths.iter().cloned().map(|source| Upload {
                source,
                node: node.clone(),
                folder: folder.clone(),
            }));
        self.next_upload(cx);
    }
    fn next_upload(&mut self, cx: &mut Context<Self>) {
        if self.uploading {
            return;
        }
        let Some(upload) = self.uploads.pop_front() else {
            return;
        };
        self.uploading = true;
        cx.spawn(async move |this, cx| {
            let source = upload.source.clone();
            let task = cx.background_executor().spawn(async move {
                let name = source
                    .file_name()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| "The file name is not valid UTF-8.".to_owned())?
                    .to_owned();
                let metadata = std::fs::metadata(&source)
                    .map_err(|e| format!("Cannot read {}: {e}", source.display()))?;
                if !metadata.is_file() {
                    return Err(format!("{} is not a file.", source.display()));
                }
                if metadata.len() > 64 * 1024 * 1024 {
                    return Err(format!("{} is larger than 64 MiB.", source.display()));
                }
                let bytes = std::fs::read(&source)
                    .map_err(|e| format!("Cannot read {}: {e}", source.display()))?;
                Ok((name, bytes))
            });
            let result = task.await;
            let _ = this.update(cx, |page, cx| match result {
                Ok((name, bytes)) => page.write_upload(upload, name, bytes, false, 0, cx),
                Err(error) => {
                    page.fail(error, cx);
                    page.uploading = false;
                    page.next_upload(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }
    fn write_upload(
        &mut self,
        upload: Upload,
        name: String,
        bytes: Vec<u8>,
        replace: bool,
        ordinal: usize,
        cx: &mut Context<Self>,
    ) {
        let target_name = if ordinal == 0 {
            name.clone()
        } else {
            upload_copy_name(&name, ordinal)
        };
        let path = paths::join(&upload.folder, &target_name);
        let mut request = Request::new(
            if replace { "PUT" } else { "POST" },
            upload.node.clone(),
            route("file", &path),
        );
        request.body = Some(("application/octet-stream".into(), bytes.clone()));
        self.request(
            request,
            move |page, status, body, cx| {
                if (200..300).contains(&status) {
                    if page.tree.node == upload.node.as_str() {
                        page.tree.open.insert(upload.folder.clone());
                        page.list(upload.folder.clone(), cx);
                        page.tree.select(&path);
                        page.pick = Some(Pick::Node(path.clone()));
                    }
                    page.uploading = false;
                    page.next_upload(cx);
                } else if status == 409
                    && code_of(&body).as_deref() == Some(parvi_files_contract::ALREADY_EXISTS)
                {
                    if ordinal > 0 {
                        page.write_upload(upload, name, bytes, false, ordinal + 1, cx);
                    } else {
                        page.upload_conflict = Some(UploadConflict {
                            upload,
                            name,
                            bytes,
                        });
                    }
                } else {
                    page.fail(message(status, &body), cx);
                    page.uploading = false;
                    page.next_upload(cx);
                }
                cx.notify();
            },
            cx,
        );
    }
    pub(crate) fn upload_conflict_name(&self) -> Option<&str> {
        self.upload_conflict.as_ref().map(|c| c.name.as_str())
    }
    pub(crate) fn resolve_upload(&mut self, replace: Option<bool>, cx: &mut Context<Self>) {
        let Some(conflict) = self.upload_conflict.take() else {
            return;
        };
        match replace {
            Some(replace) => self.write_upload(
                conflict.upload,
                conflict.name,
                conflict.bytes,
                replace,
                if replace { 0 } else { 2 },
                cx,
            ),
            None => {
                self.uploading = false;
                self.next_upload(cx);
            }
        }
        cx.notify();
    }

    /// The menu on a row: what can be done with that file or folder.
    pub fn row_menu(
        &mut self,
        path: String,
        folder: bool,
        at: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let rows = self.menu_rows(path, folder);
        self.host.menu(rows, at, cx);
    }

    /// The viewer's "…": how long lines show, then the file's own menu.
    pub fn file_menu(&mut self, at: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let Some(path) = self.opened.as_ref().map(|o| o.path.clone()) else {
            return;
        };
        let me = self.me.clone();
        let wraps = self.code.read(cx).wraps();
        let mut rows = vec![
            MenuRow::new(
                if wraps {
                    "Let long lines run on"
                } else {
                    "Wrap long lines"
                },
                move |_, cx| {
                    let _ = me.update(cx, |p, cx| p.set_wrap(!wraps, cx));
                },
            ),
            MenuRow::rule(),
        ];
        rows.extend(self.menu_rows(path, false));
        self.host.menu(rows, at, cx);
    }

    pub fn set_wrap(&mut self, on: bool, cx: &mut Context<Self>) {
        self.code.update(cx, |c, cx| c.set_wrap(on, cx));
        self.pretty_code.update(cx, |c, cx| c.set_wrap(on, cx));
        let mut k: store::Kept = self.host.setting(cx).unwrap_or_default();
        k.unwrapped = !on;
        let _ = self.host.set_setting(&k, cx);
        cx.notify();
    }

    fn menu_rows(&self, path: String, folder: bool) -> Vec<MenuRow> {
        let me = self.me.clone();
        let node = self.node();
        let root = self.tree.roots.iter().any(|r| r.path == path);
        let writable = self.tree.writable();
        let with = |f: fn(&mut FilesPage, String, &mut Window, &mut Context<FilesPage>)| {
            let me = me.clone();
            let path = path.clone();
            move |window: &mut Window, cx: &mut App| {
                let _ = me.update(cx, |p, cx| f(p, path.clone(), window, cx));
            }
        };
        let mut rows = vec![
            MenuRow::new(
                "Send to session",
                with(|p, path, _, cx| p.send_path(path, cx)),
            ),
            MenuRow::new("Copy path", {
                let path = path.clone();
                move |_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(path.clone()))
            }),
            MenuRow::new("Show in tree", {
                with(|p, path, _, cx| {
                    if p.tree.row(&path).is_some_and(|r| r.folder) {
                        p.show_folder(path, cx)
                    } else {
                        p.show_in_tree(&path, cx);
                    }
                })
            }),
        ];
        if !folder {
            rows.push(MenuRow::new("Download", {
                let path = path.clone();
                let me = me.clone();
                move |_, cx| {
                    let _ = me.update(cx, |p, cx| p.download_path(node.clone(), path.clone(), cx));
                }
            }));
        }
        if writable {
            rows.push(MenuRow::rule());
            let here = if folder {
                path.clone()
            } else {
                paths::parent(&path)
            };
            let with_here =
                |f: fn(&mut FilesPage, String, &mut Window, &mut Context<FilesPage>)| {
                    let me = me.clone();
                    let here = here.clone();
                    move |window: &mut Window, cx: &mut App| {
                        let _ = me.update(cx, |p, cx| f(p, here.clone(), window, cx));
                    }
                };
            rows.push(MenuRow::new(
                "New file",
                with_here(|p, here, w, cx| p.start_naming(Naming::File(here), w, cx)),
            ));
            rows.push(MenuRow::new(
                "New folder",
                with_here(|p, here, w, cx| p.start_naming(Naming::Folder(here), w, cx)),
            ));
            rows.push(MenuRow::new(
                "Upload…",
                with_here(|p, here, w, cx| p.pick_uploads(here, w, cx)),
            ));
            if !root {
                rows.push(MenuRow::new(
                    "Rename",
                    with(|p, path, w, cx| p.start_naming(Naming::Rename(path), w, cx)),
                ));
                rows.push(
                    MenuRow::new("Delete", with(|p, path, _, cx| p.delete(path, cx))).danger(),
                );
            }
        }
        rows
    }

    pub(crate) fn download_path(&mut self, node: NodeId, path: String, cx: &mut Context<Self>) {
        let kept: store::Kept = self.host.setting(cx).unwrap_or_default();
        let folder = if kept.download_folder.trim().is_empty() {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(|p| std::path::PathBuf::from(p).join("Downloads"))
        } else {
            Some(std::path::PathBuf::from(kept.download_folder))
        };
        let Some(folder) = folder else {
            self.fail(
                "Cannot find your Downloads folder; set a download folder in Settings.".into(),
                cx,
            );
            return;
        };
        let destination = folder.join(paths::base_name(&path));
        self.request(
            Request::get(node, route("file", &path)),
            move |p, status, body, cx| {
                if status != 200 {
                    p.fail(message(status, &body), cx);
                    return;
                }
                cx.spawn(async move |this, cx| {
                    let task = cx.background_executor().spawn(async move {
                        use std::io::Write;
                        let result = std::fs::create_dir_all(&folder).and_then(|_| {
                            let mut file = std::fs::OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .open(&destination)?;
                            file.write_all(&body)
                        });
                        (destination, result)
                    });
                    let (destination, result) = task.await;
                    let _ = this.update(cx, |page, cx| match result {
                        Ok(()) => page.host.report(
                            Status::new(format!("Downloaded to {}", destination.display())),
                            cx,
                        ),
                        Err(e) => page.fail(
                            format!("Could not download to {}: {e}", destination.display()),
                            cx,
                        ),
                    });
                })
                .detach();
            },
            cx,
        );
    }
    pub fn play(&mut self, cx: &mut Context<Self>) {
        let Some(opened) = self.opened.as_ref() else {
            return;
        };
        let node = opened.node.clone();
        let path = opened.path.clone();
        let name = opened.name.clone();
        self.request(
            Request::get(NodeId(node.clone()), route("file", &path)),
            move |p, s, b, cx| {
                if s != 200 {
                    p.fail(message(s, &b), cx)
                } else {
                    let key = open::version(format!("{node}\n{path}").as_bytes());
                    let dir = std::env::temp_dir().join("parvi-open").join(&key[..16]);
                    let dest = dir.join(&name);
                    match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&dest, b)) {
                        Ok(()) => cx.open_with_system(&dest),
                        Err(e) => p.fail(e.to_string(), cx),
                    };
                }
                cx.notify();
            },
            cx,
        )
    }
    pub fn follow_link(
        &mut self,
        node: &str,
        folder: &str,
        link: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if link.starts_with("http://")
            || link.starts_with("https://")
            || link.starts_with("mailto:")
        {
            cx.open_url(link);
            return;
        }
        let link = link.split('#').next().unwrap_or_default();
        if link.is_empty() || self.tree.node != node {
            return;
        }
        let mut at = folder.to_owned();
        for part in link.split(['/', '\\']) {
            match part {
                "" | "." => {}
                ".." => at = paths::parent(&at),
                p => at = paths::join(&at, p),
            }
        }
        if !self.tree.roots.is_empty() && paths::root_of(&self.tree.roots, &at).is_none() {
            self.fail("That link is outside the roots.".into(), cx);
            return;
        }
        self.show_in_tree(&at, cx);
        self.open(at, window, cx);
    }
    pub fn send_path(&self, path: String, cx: &mut Context<Self>) {
        let ask = Ask::Send {
            reference: paths::reference(&path),
            target: None,
        };
        if let Err(e) = self.host.ask(ask, cx) {
            self.fail(format!("Not sent: {}", e.message), cx);
        }
    }

    /// Sends the open file to a session. Lines selected in it are named
    /// after the path (`path:12-18`).
    pub fn send(&self, cx: &mut Context<Self>) {
        let Some(o) = self.opened.as_ref() else {
            return;
        };
        let lines = matches!(o.show, Show::Text { .. })
            .then(|| self.code.read(cx).selected_lines())
            .flatten();
        let path = match lines {
            Some((first, last)) if first == last => format!("{}:{first}", o.path),
            Some((first, last)) => format!("{}:{first}-{last}", o.path),
            None => o.path.clone(),
        };
        self.send_path(path, cx);
    }
    pub(crate) fn keep(&self, cx: &mut Context<Self>) {
        let mut k: store::Kept = self.host.setting(cx).unwrap_or_default();
        if self.beside {
            k.beside = self.tree.kept();
            k.beside_file = self.opened.as_ref().map(|o| o.path.clone());
        } else {
            k.tree = self.tree.kept();
            k.file = self.opened.as_ref().map(|o| o.path.clone());
        }
        if k != self.host.setting(cx).unwrap_or_default() {
            let _ = self.host.set_setting(&k, cx);
        }
    }

    /// Whether anyone sees this view now.
    fn seen_now(&self, cx: &App) -> bool {
        if self.beside {
            self.host
                .beside_of(self.beside_from, cx)
                .is_some_and(|v| Some(v.entity_id()) == self.me.upgrade().map(|e| e.entity_id()))
        } else {
            self.shown
        }
    }

    /// While the view is seen and the window active, asks every few
    /// seconds whether the open file or an open folder changed on disk, and
    /// reads again what did. Asking is cheap: when each last changed.
    pub fn watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.watching {
            return;
        }
        self.watching = true;
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(WATCH).await;
                let go = this.update_in(cx, |p, window, cx| {
                    if !p.seen_now(cx) {
                        p.watching = false;
                        p.seen.clear();
                        // Closed by the page it was beside (its Esc).
                        if p.beside && p.beside_from == PANEL_BESIDE {
                            p.keep_beside_open(false, cx);
                        }
                        return false;
                    }
                    if window.is_window_active() {
                        p.check(cx);
                    }
                    true
                });
                if !matches!(go, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }
    fn check(&mut self, cx: &mut Context<Self>) {
        let mut asked: Vec<String> = Vec::new();
        if let Some(o) = self
            .opened
            .as_ref()
            .filter(|o| !matches!(o.show, Show::Reading | Show::Media(_)) && !o.saving)
        {
            asked.push(o.path.clone());
        }
        // Beside, the list is seen only while no file is open.
        if !self.beside || self.opened.is_none() {
            asked.extend(
                self.tree
                    .open_folders()
                    .into_iter()
                    .filter(|f| self.tree.listings.contains_key(f)),
            );
        }
        if asked.is_empty() || self.tree.node.is_empty() {
            return;
        }
        let node = self.tree.node.clone();
        let body = serde_json::json!({ "paths": asked });
        self.request(
            Request::new("POST", NodeId(node.clone()), "/api/files/stat").json(&body),
            move |p, s, b, cx| {
                if s != 200 || p.tree.node != node {
                    return;
                }
                let Ok(times) = serde_json::from_slice::<Vec<Option<u64>>>(&b) else {
                    return;
                };
                let mut any = false;
                for (path, time) in asked.into_iter().zip(times) {
                    // The first answer is checked too: the file may have
                    // changed between its read and this look.
                    if p.seen.insert(path.clone(), time) == Some(time) {
                        continue;
                    }
                    any = true;
                    if let Some(o) = p.opened.as_ref().filter(|o| o.path == path) {
                        if time.is_some() {
                            let node = o.node.clone();
                            p.read(node, path, true, cx);
                        }
                    } else if p.tree.listings.contains_key(&path) {
                        p.list(path, cx);
                    }
                }
                // Recent changes walk every root: a change seen here asks
                // for them again only now and then.
                if any && p.recent_at.is_none_or(|at| at.elapsed() >= RECENT_AGAIN) {
                    p.refresh_recent(cx);
                }
            },
            cx,
        );
    }
}

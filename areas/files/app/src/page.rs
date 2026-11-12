//! Page state, file requests and user actions.
use crate::{
    open::{self, Draft, Opened, Show},
    store::{self, pane::Pane, paths},
};
use gpui::{App, AppContext, Context, Entity, FocusHandle, KeyDownEvent, WeakEntity, Window};
use parvi_app_kit::{
    Answers, Ask, Host, Page,
    host::Request,
    link::client::LinkState,
    widgets::{
        code::{Code, CodeEvent},
        field::{FieldEvent, TextField},
        syntax,
    },
};
use parvi_core::NodeId;
use parvi_files_contract::{Listing, Matches, NodeRoots};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;

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

/// What the name field is open for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Naming {
    File,
    Folder,
    Rename,
}
impl Naming {
    fn action(self) -> &'static str {
        match self {
            Naming::File => "file",
            Naming::Folder => "folder",
            Naming::Rename => "rename",
        }
    }
    pub fn verb(self) -> &'static str {
        match self {
            Naming::File | Naming::Folder => "Create",
            Naming::Rename => "Rename",
        }
    }
    fn placeholder(self) -> &'static str {
        match self {
            Naming::File => "Name of the new file",
            Naming::Folder => "Name of the new folder",
            Naming::Rename => "New name",
        }
    }
}

pub struct Side {
    pub pane: Pane,
    pub code: Entity<Code>,
    pub pretty_code: Entity<Code>,
    pub search: Entity<TextField>,
    pub search_content: bool,
    pub pending_line: Option<usize>,
    pub name: Entity<TextField>,
    pub room: crate::picture::Room,
    pub opened: Option<Opened>,
}
pub struct FilesPage {
    pub host: Host,
    pub focus: FocusHandle,
    pub side: Side,
    pub shown: bool,
    pub drafts: HashMap<(String, String), Draft>,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub beside: bool,
    pub naming: Option<Naming>,
    pub confirm_discard: bool,
    pub conflict: bool,
    copy_ordinal: usize,
    pub confirm_close_beside: bool,
    pub menu: Option<gpui::Point<gpui::Pixels>>,
    pub drawn: Vec<std::sync::Arc<gpui::RenderImage>>,
    beside_view: Option<Entity<FilesPage>>,
    restored: bool,
    beside_from: &'static str,
    me: WeakEntity<FilesPage>,
    dock_view: Option<Entity<FilesPage>>,
    pub(crate) dock: bool,
    dock_previous_focus: Option<FocusHandle>,
    pub(crate) landing: bool,
    pub(crate) projects: BTreeMap<String, parvi_files_contract::Projects>,
    pub(crate) sessions: Option<Entity<parvi_terminals_contract::SessionList>>,
    uploads: VecDeque<Upload>,
    uploading: bool,
    upload_conflict: Option<UploadConflict>,
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
impl Page for FilesPage {
    const ID: &'static str = "files";
    const TITLE: &'static str = "Files";
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
                let _ = me.update(cx, |page, cx| page.toggle_dock(window, cx));
            })
            .alt("o", Some(false))
            .everywhere(),
        ]
    }
    fn focus(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }
    fn shown(&mut self, on: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.shown = on;
        if on {
            self.start(cx)
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
            self.projects.remove(node.as_str());
            if self.side.pane.node == node.as_str() {
                self.side.pane.away = true;
                self.side.pane.error = Some("Waiting for this node to reconnect.".into());
            }
            cx.notify();
        }
        if state == Some(LinkState::Open) {
            if let Some(view) = &self.dock_view {
                view.update(cx, |page, cx| {
                    page.start(cx);
                    if let Some(path) = page
                        .side
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
                self.start(cx)
            }
            if !self.beside && !self.restored {
                self.restored = true;
                let kept: store::Kept = self.host.setting(cx).unwrap_or_default();
                if kept.sidecar_open
                    && let (Some(folder), Some(name)) = (kept.sidecar.path, kept.sidecar.selected)
                {
                    self.open_beside(
                        "terminals",
                        NodeId(kept.sidecar.node),
                        paths::join(&folder, &name),
                        window,
                        cx,
                    );
                }
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
                    self.landing = false;
                    self.side.pane.node = node.to_string();
                    self.list(
                        paths::parent(&path),
                        Some(paths::base_name(&path).to_string()),
                        cx,
                    );
                    self.open(path, window, cx)
                }
            }
            Ask::Reveal { what, id, node, .. } if what == "path" => {
                self.landing = false;
                if let Some(n) = node {
                    let changed = self.side.pane.node != n.as_str();
                    self.side.pane.choose_node(n.as_str());
                    if changed {
                        self.roots(cx);
                    }
                }
                self.reveal(id, window, cx);
            }
            _ => {}
        }
    }
}
impl FilesPage {
    pub fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::create(host, window, cx, true)
    }
    fn create(host: Host, window: &mut Window, cx: &mut Context<Self>, guard: bool) -> Self {
        let kept: store::Kept = host.setting(cx).unwrap_or_default();
        let sessions = host.model::<parvi_terminals_contract::SessionList>(cx);
        if let Some(sessions) = &sessions {
            cx.observe(sessions, |_, _, cx| cx.notify()).detach();
        }
        let side = {
            let code = cx.new(Code::new);
            let pretty_code = cx.new(Code::new);
            let search = cx.new(|cx| TextField::new(cx, "Find files"));
            let name = cx.new(|cx| TextField::new(cx, "New name"));
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
            cx.subscribe_in(
                &search,
                window,
                move |page, field, event, window, cx| match event {
                    FieldEvent::Changed => {
                        let query = field.read(cx).text.clone();
                        page.side.pane.filter = query.clone();
                        let revision = page.side.pane.set_find(&query, page.side.search_content);
                        let node = page.side.pane.node.clone();
                        let folder = page.side.pane.path.clone();
                        if !query.trim().is_empty() {
                            cx.spawn(async move |this, cx| {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(250))
                                    .await;
                                let _ = this.update(cx, |page, cx| {
                                    if page.side.pane.find_revision == revision
                                        && page.side.pane.node == node
                                        && page.side.pane.path == folder
                                    {
                                        page.find(query, cx);
                                    }
                                });
                            })
                            .detach();
                        }
                        cx.notify();
                    }
                    FieldEvent::Submit => {
                        let q = field.read(cx).text.clone();
                        page.find(q, cx)
                    }
                    FieldEvent::Down => {
                        page.side.pane.step(1);
                        window.focus(&page.focus, cx);
                        cx.notify();
                    }
                    _ => {}
                },
            )
            .detach();
            cx.subscribe_in(&name, window, move |page, _, event, window, cx| {
                if matches!(event, FieldEvent::Submit) {
                    page.apply_naming(window, cx);
                }
            })
            .detach();
            Side {
                pane: Pane::from_kept(&kept.pane),
                code,
                pretty_code,
                search,
                search_content: false,
                pending_line: None,
                name,
                room: Default::default(),
                opened: None,
            }
        };
        if guard {
            host.guard_close(
                &cx.entity(),
                |page: &FilesPage, cx| {
                    let mut names: Vec<_> = page
                        .drafts
                        .keys()
                        .chain(
                            page.dock_view
                                .iter()
                                .flat_map(|dock| dock.read(cx).drafts.keys()),
                        )
                        .map(|(_, path)| paths::base_name(path).to_owned())
                        .collect();
                    names.sort();
                    names.dedup();
                    (!names.is_empty()).then(|| names.join(", "))
                },
                cx,
            );
        }
        Self {
            focus: cx.focus_handle().tab_stop(true),
            host,
            side,
            shown: false,
            drafts: HashMap::new(),
            error: None,
            notice: None,
            beside: false,
            naming: None,
            confirm_discard: false,
            conflict: false,
            copy_ordinal: 1,
            confirm_close_beside: false,
            menu: None,
            drawn: Vec::new(),
            beside_view: None,
            restored: false,
            beside_from: "",
            me: cx.weak_entity(),
            dock_view: None,
            dock: false,
            dock_previous_focus: None,
            landing: kept.pane.path.is_none(),
            projects: BTreeMap::new(),
            sessions,
            uploads: VecDeque::new(),
            uploading: false,
            upload_conflict: None,
        }
    }
    fn toggle_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.host.docked(cx) {
            let focused = self
                .dock_view
                .as_ref()
                .is_some_and(|view| view.read(cx).focus.contains_focused(window, cx));
            self.host.close_dock(cx);
            if let Some(focus) = self.dock_previous_focus.take().filter(|_| focused) {
                window.focus(&focus, cx);
            }
            return;
        }
        self.dock_previous_focus = window.focused(cx);
        let view = match &self.dock_view {
            Some(view) => view.clone(),
            None => {
                let host = self.host.clone();
                let kept: store::Kept = host.setting(cx).unwrap_or_default();
                let view = cx.new(|cx| FilesPage::create(host, window, cx, false));
                view.update(cx, |page, cx| {
                    page.dock = true;
                    page.landing = false;
                    page.side.pane = Pane::from_kept(&kept.dock);
                    page.start(cx);
                    if let Some(path) = kept.dock_file {
                        page.open(path, window, cx);
                    }
                });
                self.dock_view = Some(view.clone());
                view
            }
        };
        self.host.show_dock(view.into(), cx);
    }
    fn open_beside(
        &mut self,
        from: &'static str,
        node: NodeId,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if from == "terminals" {
            let mut kept: store::Kept = self.host.setting(cx).unwrap_or_default();
            kept.sidecar_open = true;
            kept.sidecar.node = node.to_string();
            kept.sidecar.path = Some(paths::parent(&path));
            kept.sidecar.selected = Some(paths::base_name(&path).to_string());
            let _ = self.host.set_setting(&kept, cx);
        }
        let host = self.host.clone();
        let view = match &self.beside_view {
            Some(view) => view.clone(),
            None => {
                let view = cx.new(|cx| FilesPage::new(host.clone(), window, cx));
                self.beside_view = Some(view.clone());
                view
            }
        };
        view.update(cx, |p, cx| {
            p.beside = true;
            p.beside_from = from;
            p.side.pane.node = node.to_string();
            p.open(path, window, cx)
        });
        host.show_beside(from, view.into(), cx);
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
        if self.beside_from == "terminals" {
            let mut kept: store::Kept = self.host.setting(cx).unwrap_or_default();
            kept.sidecar_open = false;
            let _ = self.host.set_setting(&kept, cx);
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
    fn fail(&mut self, status: u16, body: &[u8]) {
        self.error = Some(
            serde_json::from_slice::<serde_json::Value>(body)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
                .unwrap_or_else(|| {
                    format!(
                        "Request failed ({status}): {}",
                        String::from_utf8_lossy(body)
                    )
                }),
        );
    }
    pub fn start(&mut self, cx: &mut Context<Self>) {
        let Some(node) = self.host.nodes(cx).first().map(|n| n.id.to_string()) else {
            return;
        };
        if self.side.pane.node.is_empty() {
            self.side.pane.node = node;
        }
        self.roots(cx);
        if self.landing && !self.beside && !self.dock {
            self.refresh_projects(cx);
        }
    }
    pub(crate) fn show_projects(&mut self, cx: &mut Context<Self>) {
        self.landing = true;
        self.refresh_projects(cx);
        cx.notify();
    }
    fn refresh_projects(&mut self, cx: &mut Context<Self>) {
        for node in self.host.nodes(cx) {
            let id = node.id.clone();
            self.request(
                Request::get(node.id, "/api/files/projects"),
                move |page, status, body, cx| {
                    if status == 200 {
                        match serde_json::from_slice(&body) {
                            Ok(projects) => {
                                page.projects.insert(id.to_string(), projects);
                            }
                            Err(error) => page.error = Some(error.to_string()),
                        }
                    } else {
                        page.fail(status, &body);
                    }
                    cx.notify();
                },
                cx,
            );
        }
    }
    pub(crate) fn browse_project(
        &mut self,
        node: String,
        folder: String,
        document: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.landing = false;
        self.side
            .search
            .update(cx, |field, cx| field.set("", false, cx));
        let changed = self.side.pane.node != node;
        self.side.pane.choose_node(&node);
        if self.side.pane.path.as_deref() != Some(folder.as_str()) {
            self.side.pane.entries.clear();
            self.side.pane.find = None;
            self.side.pane.filter.clear();
        }
        self.side.pane.path = Some(folder.clone());
        self.side.pane.selected = document.clone();
        self.side.opened = None;
        if changed {
            self.roots(cx);
        } else {
            self.list(folder.clone(), document.clone(), cx);
        }
        if let Some(name) = document {
            self.open(paths::join(&folder, &name), window, cx);
        }
        cx.notify();
    }
    fn roots(&mut self, cx: &mut Context<Self>) {
        let node = self.side.pane.node.clone();
        let asked = self.side.pane.asked;
        self.request(
            Request::get(NodeId(node.clone()), "/api/files/roots"),
            move |p, s, b, cx| {
                if p.side.pane.node != node {
                    return;
                }
                if s != 200 {
                    p.fail(s, &b);
                    p.side.pane.away = true;
                    p.side.pane.error = p.error.clone();
                    cx.notify();
                    return;
                }
                match serde_json::from_slice::<NodeRoots>(&b) {
                    Ok(r) => {
                        p.side.pane.roots = r.roots;
                        if p.side.pane.asked == asked
                            && (!p.landing || p.beside || p.dock)
                            && let Some(path) = p.side.pane.start_path()
                        {
                            p.list(path, None, cx)
                        }
                    }
                    Err(e) => p.error = Some(e.to_string()),
                }
                cx.notify()
            },
            cx,
        )
    }
    fn reveal(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let node = self.side.pane.node.clone();
        self.side.pane.asked += 1;
        let asked = self.side.pane.asked;
        let task = self.host.request(
            Request::get(NodeId(node.clone()), route("folder", &path)),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = task.await;
            let _ = this.update_in(cx, |page, window, cx| {
                if page.side.pane.node != node || page.side.pane.asked != asked {
                    return;
                }
                match answer {
                    Ok(answer) if answer.status == 200 => page.list(path, None, cx),
                    Ok(answer)
                        if serde_json::from_slice::<serde_json::Value>(&answer.body)
                            .ok()
                            .is_some_and(|v| {
                                v["error"]["code"] == parvi_files_contract::NOT_FOLDER
                            }) =>
                    {
                        page.list(
                            paths::parent(&path),
                            Some(paths::base_name(&path).to_string()),
                            cx,
                        );
                        page.open(path, window, cx);
                    }
                    Ok(answer) => page.fail(answer.status, &answer.body),
                    Err(error) => page.error = Some(error.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_folder(
        &mut self,
        node: &str,
        folder: &str,
        select: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.side.pane.node == node && self.side.pane.path.as_deref() == Some(folder) {
            self.list(folder.to_owned(), select, cx);
        }
    }

    pub fn list(&mut self, path: String, select: Option<String>, cx: &mut Context<Self>) {
        self.list_with_menu(path, select, None, cx);
    }

    fn list_with_menu(
        &mut self,
        path: String,
        select: Option<String>,
        menu: Option<gpui::Point<gpui::Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let node = self.side.pane.node.clone();
        self.side.pane.asked += 1;
        let id = self.side.pane.asked;
        self.request(
            Request::get(NodeId(node.clone()), route("folder", &path)),
            move |p, s, b, cx| {
                if p.side.pane.node != node || p.side.pane.asked != id {
                    return;
                }
                if s != 200 {
                    p.fail(s, &b);
                    if s == 0 || s == 421 || s == 502 || s == 503 {
                        p.side.pane.away = true;
                        p.side.pane.error = p.error.clone();
                    }
                } else {
                    match serde_json::from_slice::<Listing>(&b) {
                        Ok(l) => {
                            let moved = p.side.pane.path.as_deref() != Some(l.path.as_str());
                            let opening_selected = p.side.opened.as_ref().is_some_and(|o| {
                                paths::parent(&o.path) == l.path
                                    && select.as_deref() == Some(paths::base_name(&o.path))
                            });
                            p.side.pane.listed(l.path, l.entries, select);
                            if moved && !opening_selected && menu.is_none() {
                                p.side.opened = None;
                            }
                            if let Some(at) = menu {
                                p.side.pane.find = None;
                                p.side
                                    .search
                                    .update(cx, |field, cx| field.set("", false, cx));
                                if p.side.pane.selection().is_some() {
                                    p.side.pane.armed = false;
                                    p.menu = Some(at);
                                }
                            }
                            p.keep(cx)
                        }
                        Err(e) => p.error = Some(e.to_string()),
                    }
                }
                cx.notify()
            },
            cx,
        )
    }
    pub fn find(&mut self, q: String, cx: &mut Context<Self>) {
        let Some(path) = self.side.pane.path.clone() else {
            return;
        };
        let content = self.side.search_content;
        let revision = self.side.pane.set_find(&q, content);
        if q.trim().is_empty() {
            cx.notify();
            return;
        }
        let node = self.side.pane.node.clone();
        let uri = format!(
            "{}&q={}",
            route(if content { "search" } else { "find" }, &path),
            enc(&q)
        );
        self.request(
            Request::get(NodeId(node.clone()), uri),
            move |p, s, b, cx| {
                let found = if s == 200 {
                    serde_json::from_slice::<Matches>(&b).map_err(|e| e.to_string())
                } else {
                    Err(String::from_utf8_lossy(&b).into_owned())
                };
                if p.side.pane.node == node && p.side.pane.path.as_deref() == Some(path.as_str()) {
                    p.side.pane.found(revision, found);
                }
                cx.notify()
            },
            cx,
        )
    }
    pub fn toggle_search_content(&mut self, cx: &mut Context<Self>) {
        self.side.search_content = !self.side.search_content;
        let query = self.side.search.read(cx).text.clone();
        self.find(query, cx);
    }
    pub fn open_at(
        &mut self,
        path: String,
        line: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(path, window, cx);
        self.side.pending_line = line;
    }
    pub fn open(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.side.pending_line = None;
        self.side
            .code
            .update(cx, |code, cx| code.edit(false, window, cx));
        if !self.beside && !self.dock {
            Code::focus(&self.side.code, window, cx);
        }
        let node = self.side.pane.node.clone();
        let name = paths::base_name(&path).to_string();
        self.side.opened = Some(Opened {
            node: node.clone(),
            path: path.clone(),
            name: name.clone(),
            size: None,
            show: Show::Reading,
            saved: String::new(),
            base: String::new(),
            pretty: None,
            editing: false,
            saving: false,
            source: matches!(
                crate::store::kinds::kind(&name),
                crate::store::kinds::Kind::Json
            ),
            diff_open: false,
            diff: None,
        });
        if self.dock {
            self.keep(cx);
        }
        if matches!(
            crate::store::kinds::kind(&name),
            crate::store::kinds::Kind::Audio
                | crate::store::kinds::Kind::Video
                | crate::store::kinds::Kind::Pdf
        ) {
            self.side.opened.as_mut().unwrap().show = Show::Media(crate::store::kinds::kind(&name));
            cx.notify();
            return;
        }
        self.request(
            Request::get(NodeId(node.clone()), route("file", &path)),
            move |p, s, b, cx| {
                if !p
                    .side
                    .opened
                    .as_ref()
                    .is_some_and(|o| o.node == node && o.path == path)
                {
                    return;
                }
                if s != 200 {
                    if serde_json::from_slice::<serde_json::Value>(&b)
                        .ok()
                        .is_some_and(|v| v["error"]["code"] == "not_file")
                    {
                        p.list(path, None, cx);
                        return;
                    }
                    p.fail(s, &b);
                    if let Some(o) = p.side.opened.as_mut() {
                        o.show = Show::Failed(p.error.clone().unwrap_or_default());
                    }
                    cx.notify();
                    return;
                }
                if crate::store::kinds::kind(&name) == crate::store::kinds::Kind::Model {
                    let task = cx.background_executor().spawn(async move {
                        crate::model::Model::load(&b).map(crate::model::Preview::new)
                    });
                    cx.spawn(async move |this, cx| {
                        let result = task.await;
                        let _ = this.update(cx, |p, cx| {
                            if let Some(o) = p
                                .side
                                .opened
                                .as_mut()
                                .filter(|o| o.node == node && o.path == path)
                            {
                                o.show = match result {
                                    Ok(v) => Show::Model(v),
                                    Err(e) => Show::Failed(e),
                                };
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                    return;
                }
                if let Some(format) = open::picture_format(&name) {
                    let svg = cx.svg_renderer();
                    let task = cx.background_executor().spawn(async move {
                        gpui::Image::from_bytes(format, b).to_image_data(svg)
                    });
                    cx.spawn(async move |this, cx| {
                        let result = task.await;
                        let _ = this.update(cx, |p, cx| {
                            if let Some(o) = p
                                .side
                                .opened
                                .as_mut()
                                .filter(|o| o.node == node && o.path == path)
                            {
                                o.show = match result {
                                    Ok(image) => {
                                        let dim = image.size(0);
                                        Show::Picture {
                                            natural: crate::store::zoom::Px {
                                                w: dim.width.0 as f32,
                                                h: dim.height.0 as f32,
                                            },
                                            image,
                                        }
                                    }
                                    Err(e) => Show::Failed(e.to_string()),
                                };
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                    return;
                }
                let (show, text, pretty) = open::show(&name, &b);
                let base = open::version(&b);
                let draft = p.drafts.get(&(node, path)).cloned();
                let code = p.side.code.clone();
                p.side.pretty_code.update(cx, |code, cx| {
                    code.set_lang(syntax::lang(&name), cx);
                    code.set(pretty.as_deref().unwrap_or(&text), cx);
                });
                let line = p.side.pending_line.take();
                code.update(cx, |code, cx| {
                    code.set_lang(syntax::lang(&name), cx);
                    code.set(draft.as_ref().map_or(&text, |d| &d.text), cx);
                    if let Some(line) = line {
                        code.show_line(line.saturating_sub(1), cx);
                    }
                });
                let o = p.side.opened.as_mut().unwrap();
                if line.is_some() {
                    o.source = true;
                }
                o.show = show;
                o.saved = text;
                o.base = draft.map_or(base, |d| d.base);
                o.pretty = pretty;
                o.size = Some(b.len() as u64);
                cx.notify()
            },
            cx,
        )
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        let Some(o) = self.side.opened.as_ref() else {
            return;
        };
        let text = self.side.code.read(cx).text().to_owned();
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
    pub fn toggle_diff(&mut self, cx: &mut Context<Self>) {
        let Some(opened) = self.side.opened.as_mut() else {
            return;
        };
        opened.diff_open = !opened.diff_open;
        if !opened.diff_open {
            cx.notify();
            return;
        }
        opened.diff = None;
        let node = opened.node.clone();
        let path = opened.path.clone();
        self.request(
            Request::get(NodeId(node.clone()), route("diff", &path)),
            move |page, status, body, cx| {
                let answer = if status == 200 {
                    serde_json::from_slice::<parvi_files_contract::Diff>(&body)
                        .map_err(|e| e.to_string())
                } else {
                    Err(String::from_utf8_lossy(&body).into_owned())
                };
                if let Some(opened) = page
                    .side
                    .opened
                    .as_mut()
                    .filter(|o| o.node == node && o.path == path)
                {
                    opened.diff = Some(answer);
                }
                cx.notify();
            },
            cx,
        );
        cx.notify();
    }
    pub fn edit(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if let Some(o) = self.side.opened.as_mut() {
            o.editing = true;
            self.side.code.update(cx, |c, cx| c.edit(true, w, cx));
        }
        cx.notify()
    }
    fn finish_edit(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.side.opened.as_ref().is_some_and(|o| !o.editing) {
            self.side.opened = None;
            w.focus(&self.focus, cx);
            cx.notify();
            return;
        }
        let dirty = self
            .side
            .opened
            .as_ref()
            .is_some_and(|o| self.drafts.contains_key(&o.key()));
        if dirty {
            self.confirm_discard = true;
        } else if let Some(o) = self.side.opened.as_mut() {
            o.editing = false;
            self.side.code.update(cx, |c, cx| c.edit(false, w, cx));
        }
        cx.notify()
    }
    pub fn keep_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.confirm_discard) {
            Code::focus(&self.side.code, window, cx);
        }
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        self.write(open::Write::Save, cx);
    }
    pub fn cancel_conflict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.conflict) {
            Code::focus(&self.side.code, window, cx)
        }
        cx.notify();
    }
    pub fn write(&mut self, mode: open::Write, cx: &mut Context<Self>) {
        let Some(opened) = self.side.opened.as_mut() else {
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
        let sent = self.side.code.read(cx).text().to_owned();
        let (target, ordinal) = if mode == open::Write::Copy {
            let mut n = self.copy_ordinal;
            while self
                .side
                .pane
                .entries
                .iter()
                .any(|e| e.name == open::copy_name(&name, n))
            {
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
                    .side
                    .opened
                    .as_ref()
                    .is_some_and(|o| o.node == node && o.path == path)
                {
                    return;
                }
                if status != success {
                    p.side.opened.as_mut().unwrap().saving = false;
                    if status == 412 {
                        p.conflict = true;
                        p.copy_ordinal = 1;
                    } else if status == 409 && mode == open::Write::Copy {
                        p.copy_ordinal = ordinal + 1;
                        p.write(mode, cx);
                    } else {
                        p.fail(status, &body)
                    }
                } else {
                    p.conflict = false;
                    p.host
                        .toast(&format!("Saved {}", paths::base_name(&target)), cx);
                    let now = p.side.code.read(cx).text().to_owned();
                    let o = p.side.opened.as_mut().unwrap();
                    let old_key = o.key();
                    o.saving = false;
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
                    p.side.pretty_code.update(cx, |code, cx| {
                        code.set(pretty.as_deref().unwrap_or(&sent), cx)
                    });
                    p.drafts.remove(&old_key);
                    if let Some(d) = open::acknowledged(&now, &sent) {
                        p.drafts.insert(new_key, d);
                    }
                    let selection =
                        (mode == open::Write::Copy).then(|| paths::base_name(&target).to_owned());
                    p.refresh_folder(&node, &folder, selection, cx);
                }
                cx.notify();
            },
            cx,
        )
    }
    pub fn search_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if e.keystroke.key == "escape" && self.side.search.read(cx).is_focused(window) {
            self.side
                .search
                .update(cx, |field, cx| field.set("", false, cx));
            self.side.pane.filter.clear();
            self.side.pane.find = None;
            window.focus(&self.focus, cx);
            cx.stop_propagation();
            cx.notify();
        }
    }
    /// Keyboard navigation in the file table.
    pub fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        let key = &e.keystroke;
        let m = key.modifiers;
        if m.alt || m.platform || (m.control && key.key != "c") {
            return;
        }
        if self.menu.is_some() {
            self.menu = None;
            if key.key == "escape" {
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }
        match key.key.as_str() {
            "up" | "k" => {
                self.side.pane.step(-1);
            }
            "down" | "j" => {
                self.side.pane.step(1);
            }
            "home" => {
                self.side.pane.step(isize::MIN);
            }
            "end" => {
                self.side.pane.step(isize::MAX);
            }
            "enter" => {
                let selected = self
                    .side
                    .pane
                    .selection()
                    .map(|e| (e.name.clone(), e.folder));
                if let (Some((name, folder)), Some(dir)) = (selected, self.side.pane.path.clone()) {
                    let path = paths::join(&dir, &name);
                    if folder {
                        self.list(path, None, cx)
                    } else {
                        self.open(path, window, cx)
                    }
                }
            }
            "backspace" => {
                if let Some(up) = self.side.pane.up() {
                    let select = self.side.pane.came_from();
                    self.list(up, select, cx)
                }
            }
            "f5" => {
                if let Some(path) = self.side.pane.path.clone() {
                    self.list(path, None, cx)
                }
            }
            "delete" => self.mutate("delete", cx),
            "/" => parvi_app_kit::widgets::field::focus(&self.side.search, window, cx),
            "f2" => self.start_naming(Naming::Rename, window, cx),
            "escape" => {
                if self.side.opened.is_some() {
                    self.side.opened = None
                } else {
                    self.side.pane.find = None;
                    self.side.pane.filter.clear();
                    self.side.search.update(cx, |f, cx| f.set("", false, cx));
                }
            }
            "c" if m.control && m.shift => {
                if let Some(path) = self.side.pane.selected_path() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(path))
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
    pub fn sort(&mut self, column: store::listing::Column, cx: &mut Context<Self>) {
        self.side.pane.sort = self.side.pane.sort.toggled(column);
        self.keep(cx);
        cx.notify();
    }
    pub fn discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard = false;
        if let Some(opened) = self.side.opened.as_mut() {
            self.drafts.remove(&opened.key());
            let saved = opened.saved.clone();
            opened.editing = false;
            self.side.code.update(cx, |code, cx| {
                code.set(&saved, cx);
                code.edit(false, window, cx)
            });
        }
        cx.notify();
    }
    /// Opens the name field to create a file or folder here, or to rename
    /// the selected entry.
    pub fn start_naming(&mut self, naming: Naming, window: &mut Window, cx: &mut Context<Self>) {
        if !self.side.pane.writable() {
            return;
        }
        let text = match naming {
            Naming::Rename => match self.side.pane.selection() {
                Some(entry) => entry.name.clone(),
                None => return,
            },
            _ => String::new(),
        };
        self.naming = Some(naming);
        self.side.name.update(cx, |field, cx| {
            field.set_placeholder(naming.placeholder());
            field.set(&text, true, cx)
        });
        parvi_app_kit::widgets::field::focus(&self.side.name, window, cx);
        cx.notify();
    }
    pub fn stop_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.naming = None;
        self.side
            .name
            .update(cx, |field, cx| field.set("", false, cx));
        window.focus(&self.focus, cx);
        cx.notify();
    }
    pub fn apply_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(naming) = self.naming {
            self.mutate(naming.action(), cx);
            window.focus(&self.focus, cx);
        }
    }
    pub fn mutate(&mut self, action: &'static str, cx: &mut Context<Self>) {
        let pane = &mut self.side.pane;
        let Some(folder) = pane.path.clone() else {
            return;
        };
        if !pane.writable() {
            return;
        }
        let node = pane.node.clone();
        let name = self.side.name.read(cx).text.trim().to_owned();
        let selected = self.side.pane.selected_path();
        if matches!(action, "file" | "folder" | "rename")
            && let Some(reason) = paths::bad_name(&name)
        {
            self.error = Some(reason.into());
            cx.notify();
            return;
        }
        let moved = if action == "rename" {
            selected
                .as_ref()
                .map(|from| (from.clone(), paths::join(&folder, &name)))
        } else {
            None
        };
        let (method, uri) = match action {
            "file" | "folder" if !name.is_empty() => {
                ("POST", route(action, &paths::join(&folder, &name)))
            }
            "rename" if !name.is_empty() && selected.is_some() => (
                "POST",
                format!(
                    "{}&to={}",
                    route("rename", selected.as_deref().unwrap()),
                    enc(&paths::join(&folder, &name))
                ),
            ),
            "delete" => {
                let Some(path) = self.side.pane.press_delete() else {
                    cx.notify();
                    return;
                };
                ("DELETE", route("file", &path))
            }
            _ => return,
        };
        self.request(
            Request::new(method, NodeId(node.clone()), uri),
            move |p, s, b, cx| {
                if !(200..300).contains(&s) {
                    p.fail(s, &b)
                } else {
                    if action == "delete"
                        && let Some(deleted) = &selected
                        && p.side
                            .opened
                            .as_ref()
                            .is_some_and(|o| o.node == node && paths::inside(deleted, &o.path))
                    {
                        p.side.opened = None;
                    }
                    let select = if let Some((from, to)) = moved {
                        store::drafts::move_drafts(&mut p.drafts, &node, &from, &to);
                        if let Some(opened) = p
                            .side
                            .opened
                            .as_mut()
                            .filter(|o| o.node == node && paths::inside(&from, &o.path))
                        {
                            opened.path = format!("{to}{}", &opened.path[from.len()..]);
                            opened.name = paths::base_name(&opened.path).to_owned();
                        }
                        Some(paths::base_name(&to).to_owned())
                    } else if action == "file" || action == "folder" {
                        Some(name)
                    } else {
                        None
                    };
                    if action != "delete" {
                        p.naming = None;
                        p.side.name.update(cx, |field, cx| field.set("", false, cx));
                    }
                    p.refresh_folder(&node, &folder, select, cx)
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
        let node = NodeId(self.side.pane.node.clone());
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
        let node = NodeId(self.side.pane.node.clone());
        self.queue_uploads(node, folder, paths, cx);
    }
    fn queue_uploads(
        &mut self,
        node: NodeId,
        folder: String,
        paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        if !self.uploading {
            self.error = None;
        }
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
                    page.error = Some(error);
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
                    page.refresh_folder(
                        upload.node.as_str(),
                        &upload.folder,
                        Some(target_name),
                        cx,
                    );
                    page.uploading = false;
                    page.next_upload(cx);
                } else if status == 409
                    && serde_json::from_slice::<serde_json::Value>(&body)
                        .ok()
                        .is_some_and(|v| v["error"]["code"] == parvi_files_contract::ALREADY_EXISTS)
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
                    page.fail(status, &body);
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
    pub fn context_menu(
        &mut self,
        name: String,
        at: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.side.pane.select(&name);
        self.side.pane.armed = false;
        self.menu = Some(at);
        cx.notify();
    }
    pub fn find_context_menu(
        &mut self,
        path: String,
        at: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.list_with_menu(
            paths::parent(&path),
            Some(paths::base_name(&path).to_owned()),
            Some(at),
            cx,
        );
    }

    pub fn toggle_pin(&mut self, cx: &mut Context<Self>) {
        let pane = &self.side.pane;
        let Some(path) = pane.selected_path() else {
            return;
        };
        let folder = pane.selection().is_some_and(|entry| entry.folder);
        let mut kept: store::Kept = self.host.setting(cx).unwrap_or_default();
        if let Some(at) = kept
            .pins
            .iter()
            .position(|p| p.node == pane.node && p.path == path)
        {
            kept.pins.remove(at);
        } else {
            kept.pins.insert(
                0,
                store::Pin {
                    node: pane.node.clone(),
                    path,
                    folder,
                },
            );
        }
        if let Err(e) = self.host.set_setting(&kept, cx) {
            self.error = Some(e.message);
        }
        cx.notify();
    }
    pub fn open_pin(&mut self, pin: store::Pin, window: &mut Window, cx: &mut Context<Self>) {
        let changed_node = self.side.pane.node != pin.node;
        self.side.pane.choose_node(&pin.node);
        let folder = if pin.folder {
            pin.path.clone()
        } else {
            paths::parent(&pin.path)
        };
        let selected = (!pin.folder).then(|| paths::base_name(&pin.path).to_owned());
        if changed_node {
            self.side.pane.path = Some(folder);
            self.side.pane.selected = selected;
            self.roots(cx);
        } else {
            self.list(folder, selected, cx);
        }
        if !pin.folder {
            self.open(pin.path, window, cx);
        }
    }
    fn show_notice(&mut self, message: String, cx: &mut Context<Self>) {
        self.notice = Some(message.clone());
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(5))
                .await;
            let _ = this.update(cx, |page, cx| {
                if page.notice.as_deref() == Some(message.as_str()) {
                    page.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn download(&mut self, cx: &mut Context<Self>) {
        let pane = &self.side.pane;
        let Some(path) = pane.selected_path() else {
            return;
        };
        if pane.selection().is_none_or(|e| e.folder) {
            return;
        }
        let node = NodeId(pane.node.clone());
        self.download_path(node, path, cx);
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
            self.error = Some(
                "Cannot find your Downloads folder; set a download folder in Settings.".into(),
            );
            cx.notify();
            return;
        };
        let destination = folder.join(paths::base_name(&path));
        self.notice = None;
        self.request(
            Request::get(node, route("file", &path)),
            move |p, status, body, cx| {
                if status != 200 {
                    p.fail(status, &body);
                    cx.notify();
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
                    let _ = this.update(cx, |page, cx| {
                        match result {
                            Ok(()) => page.show_notice(
                                format!("Downloaded to {}", destination.display()),
                                cx,
                            ),
                            Err(e) => {
                                page.error = Some(format!(
                                    "Could not download to {}: {e}",
                                    destination.display()
                                ))
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            },
            cx,
        );
    }
    pub fn play(&mut self, cx: &mut Context<Self>) {
        let Some(opened) = self.side.opened.as_ref() else {
            return;
        };
        let node = opened.node.clone();
        let path = opened.path.clone();
        let name = opened.name.clone();
        self.request(
            Request::get(NodeId(node.clone()), route("file", &path)),
            move |p, s, b, cx| {
                if s != 200 {
                    p.fail(s, &b)
                } else {
                    let key = open::version(format!("{node}\n{path}").as_bytes());
                    let dir = std::env::temp_dir().join("parvi-open").join(&key[..16]);
                    let dest = dir.join(&name);
                    match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&dest, b)) {
                        Ok(()) => cx.open_with_system(&dest),
                        Err(e) => p.error = Some(e.to_string()),
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
        if link.is_empty() || self.side.pane.node != node {
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
        if !self.side.pane.roots.is_empty() && paths::root_of(&self.side.pane.roots, &at).is_none()
        {
            self.error = Some("That link is outside the roots.".into());
            cx.notify();
            return;
        }
        self.list(
            paths::parent(&at),
            Some(paths::base_name(&at).to_owned()),
            cx,
        );
        self.open(at, window, cx);
    }
    pub fn send_selected(&self, cx: &mut Context<Self>) {
        if let Some(path) = self.side.pane.selected_path() {
            let _ = self.host.ask(
                Ask::Send {
                    reference: paths::reference(&path),
                    target: None,
                },
                cx,
            );
        }
    }

    pub fn send(&self, cx: &mut Context<Self>) {
        let path = self
            .side
            .opened
            .as_ref()
            .map(|o| o.path.clone())
            .or_else(|| self.side.pane.selected_path());
        if let Some(path) = path {
            let _ = self.host.ask(
                Ask::Send {
                    reference: paths::reference(&path),
                    target: None,
                },
                cx,
            );
        }
    }
    pub(crate) fn keep(&self, cx: &mut Context<Self>) {
        let mut k: store::Kept = self.host.setting(cx).unwrap_or_default();
        if self.dock {
            k.dock = self.side.pane.kept();
            k.dock_file = self.side.opened.as_ref().map(|o| o.path.clone());
        } else {
            k.pane = self.side.pane.kept();
        }
        let _ = self.host.set_setting(&k, cx);
    }
}

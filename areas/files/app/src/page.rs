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
    side: usize,
}
struct UploadConflict {
    upload: Upload,
    name: String,
    bytes: Vec<u8>,
}

pub struct Side {
    pub pane: Pane,
    pub code: Entity<Code>,
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
    pub sides: [Side; 2],
    pub active: usize,
    pub shown: bool,
    pub drafts: HashMap<(String, String), Draft>,
    pub transfers: Vec<store::transfers::Transfer>,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub beside: bool,
    pub confirm_discard: Option<usize>,
    pub conflict: Option<usize>,
    copy_ordinal: usize,
    pub confirm_close_beside: bool,
    pub menu: Option<(usize, gpui::Point<gpui::Pixels>)>,
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
            for side in &mut self.sides {
                if side.pane.node == node.as_str() {
                    side.pane.away = true;
                    side.pane.error = Some("Waiting for this node to reconnect.".into());
                }
            }
            cx.notify();
        }
        if state == Some(LinkState::Open) {
            if let Some(view) = &self.dock_view {
                view.update(cx, |page, cx| {
                    page.start(cx);
                    if let Some(path) = page.sides[0]
                        .opened
                        .as_ref()
                        .filter(|o| o.node == node.as_str() && matches!(o.show, Show::Failed(_)))
                        .map(|o| o.path.clone())
                    {
                        page.open(0, path, window, cx);
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
                    let i = self.active;
                    self.sides[i].pane.node = node.to_string();
                    self.list(
                        i,
                        paths::parent(&path),
                        Some(paths::base_name(&path).to_string()),
                        cx,
                    );
                    self.open(i, path, window, cx)
                }
            }
            Ask::Reveal { what, id, node, .. } if what == "path" => {
                self.landing = false;
                if let Some(n) = node {
                    self.sides[self.active].pane.node = n.to_string();
                }
                let dir = paths::parent(&id);
                self.list(
                    self.active,
                    dir,
                    Some(paths::base_name(&id).to_string()),
                    cx,
                );
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
        let sides = std::array::from_fn(|i| {
            let code = cx.new(Code::new);
            let search = cx.new(|cx| TextField::new(cx, "Find files"));
            let name = cx.new(|cx| TextField::new(cx, "New name"));
            cx.subscribe_in(
                &code,
                window,
                move |page, _, event, window, cx| match event {
                    CodeEvent::Save => page.save(i, cx),
                    CodeEvent::Changed => page.changed(i, cx),
                    CodeEvent::Escape => page.finish_edit(i, window, cx),
                },
            )
            .detach();
            cx.subscribe_in(
                &search,
                window,
                move |page, field, event, window, cx| match event {
                    FieldEvent::Changed => {
                        let query = field.read(cx).text.clone();
                        page.sides[i].pane.filter = query.clone();
                        let revision = page.sides[i]
                            .pane
                            .set_find(&query, page.sides[i].search_content);
                        let node = page.sides[i].pane.node.clone();
                        let folder = page.sides[i].pane.path.clone();
                        if !query.trim().is_empty() {
                            cx.spawn(async move |this, cx| {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(250))
                                    .await;
                                let _ = this.update(cx, |page, cx| {
                                    if page.sides[i].pane.find_revision == revision
                                        && page.sides[i].pane.node == node
                                        && page.sides[i].pane.path == folder
                                    {
                                        page.find(i, query, cx);
                                    }
                                });
                            })
                            .detach();
                        }
                        cx.notify();
                    }
                    FieldEvent::Submit => {
                        let q = field.read(cx).text.clone();
                        page.find(i, q, cx)
                    }
                    FieldEvent::Down => {
                        page.sides[i].pane.step(1);
                        window.focus(&page.focus, cx);
                        cx.notify();
                    }
                    _ => {}
                },
            )
            .detach();
            cx.subscribe_in(&name, window, move |page, _, event, _, cx| {
                if matches!(event, FieldEvent::Submit) {
                    page.mutate(i, "rename", cx);
                }
            })
            .detach();
            Side {
                pane: Pane::from_kept(&kept.panes[i]),
                code,
                search,
                search_content: false,
                pending_line: None,
                name,
                room: Default::default(),
                opened: None,
            }
        });
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
            sides,
            active: 0,
            shown: false,
            drafts: HashMap::new(),
            transfers: kept
                .transfers
                .into_iter()
                .filter(|item| item.request.node == item.request.target_node)
                .map(|item| {
                    let mut transfer = store::transfers::Transfer::new(item);
                    if transfer.kept.state == store::transfers::State::Running {
                        transfer.wait("The copy was interrupted; retry it.");
                    }
                    transfer
                })
                .collect(),
            error: None,
            notice: None,
            beside: false,
            confirm_discard: None,
            conflict: None,
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
            landing: kept.panes[0].path.is_none(),
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
                    page.sides[0].pane = Pane::from_kept(&kept.dock);
                    page.start(cx);
                    if let Some(path) = kept.dock_file {
                        page.open(0, path, window, cx);
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
            p.sides[0].pane.node = node.to_string();
            p.open(0, path, window, cx)
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
        for i in 0..2 {
            if self.sides[i].pane.node.is_empty() {
                self.sides[i].pane.node = node.clone();
            }
            self.roots(i, cx);
        }
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
        self.active = 0;
        self.sides[0]
            .search
            .update(cx, |field, cx| field.set("", false, cx));
        let changed = self.sides[0].pane.node != node;
        self.sides[0].pane.choose_node(&node);
        if self.sides[0].pane.path.as_deref() != Some(folder.as_str()) {
            self.sides[0].pane.entries.clear();
            self.sides[0].pane.find = None;
            self.sides[0].pane.filter.clear();
        }
        self.sides[0].pane.path = Some(folder.clone());
        self.sides[0].pane.selected = document.clone();
        self.sides[0].opened = None;
        if changed {
            self.roots(0, cx);
        } else {
            self.list(0, folder.clone(), document.clone(), cx);
        }
        if self.sides[1].pane.path.is_none() {
            self.roots(1, cx);
        }
        if let Some(name) = document {
            self.open(0, paths::join(&folder, &name), window, cx);
        }
        cx.notify();
    }
    fn roots(&mut self, i: usize, cx: &mut Context<Self>) {
        let node = self.sides[i].pane.node.clone();
        self.request(
            Request::get(NodeId(node.clone()), "/api/files/roots"),
            move |p, s, b, cx| {
                if p.sides[i].pane.node != node {
                    return;
                }
                if s != 200 {
                    p.fail(s, &b);
                    p.sides[i].pane.away = true;
                    p.sides[i].pane.error = p.error.clone();
                    cx.notify();
                    return;
                }
                match serde_json::from_slice::<NodeRoots>(&b) {
                    Ok(r) => {
                        p.sides[i].pane.roots = r.roots;
                        if (!p.landing || p.beside || p.dock)
                            && let Some(path) = p.sides[i].pane.start_path(i)
                        {
                            p.list(i, path, None, cx)
                        }
                    }
                    Err(e) => p.error = Some(e.to_string()),
                }
                cx.notify()
            },
            cx,
        )
    }
    pub fn list(&mut self, i: usize, path: String, select: Option<String>, cx: &mut Context<Self>) {
        let node = self.sides[i].pane.node.clone();
        self.sides[i].pane.asked += 1;
        let id = self.sides[i].pane.asked;
        self.request(
            Request::get(NodeId(node.clone()), route("folder", &path)),
            move |p, s, b, cx| {
                if p.sides[i].pane.node != node || p.sides[i].pane.asked != id {
                    return;
                }
                if s != 200 {
                    p.fail(s, &b);
                    if s == 0 || s == 421 || s == 502 || s == 503 {
                        p.sides[i].pane.away = true;
                        p.sides[i].pane.error = p.error.clone();
                    }
                } else {
                    match serde_json::from_slice::<Listing>(&b) {
                        Ok(l) => {
                            let moved = p.sides[i].pane.path.as_deref() != Some(l.path.as_str());
                            let opening_selected = p.sides[i].opened.as_ref().is_some_and(|o| {
                                paths::parent(&o.path) == l.path
                                    && select.as_deref() == Some(paths::base_name(&o.path))
                            });
                            p.sides[i].pane.listed(l.path, l.entries, select);
                            if moved && !opening_selected {
                                p.sides[i].opened = None;
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
    pub fn find(&mut self, i: usize, q: String, cx: &mut Context<Self>) {
        let Some(path) = self.sides[i].pane.path.clone() else {
            return;
        };
        let content = self.sides[i].search_content;
        let revision = self.sides[i].pane.set_find(&q, content);
        if q.trim().is_empty() {
            cx.notify();
            return;
        }
        let node = self.sides[i].pane.node.clone();
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
                if p.sides[i].pane.node == node
                    && p.sides[i].pane.path.as_deref() == Some(path.as_str())
                {
                    p.sides[i].pane.found(revision, found);
                }
                cx.notify()
            },
            cx,
        )
    }
    pub fn toggle_search_content(&mut self, i: usize, cx: &mut Context<Self>) {
        self.sides[i].search_content = !self.sides[i].search_content;
        let query = self.sides[i].search.read(cx).text.clone();
        self.find(i, query, cx);
    }
    pub fn open_at(
        &mut self,
        i: usize,
        path: String,
        line: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(i, path, window, cx);
        self.sides[i].pending_line = line;
    }
    pub fn open(&mut self, i: usize, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.sides[i].pending_line = None;
        self.sides[i]
            .code
            .update(cx, |code, cx| code.edit(false, window, cx));
        if !self.beside && !self.dock {
            Code::focus(&self.sides[i].code, window, cx);
        }
        self.active = i;
        let node = self.sides[i].pane.node.clone();
        let name = paths::base_name(&path).to_string();
        self.sides[i].opened = Some(Opened {
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
            source: false,
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
            self.sides[i].opened.as_mut().unwrap().show =
                Show::Media(crate::store::kinds::kind(&name));
            cx.notify();
            return;
        }
        self.request(
            Request::get(NodeId(node.clone()), route("file", &path)),
            move |p, s, b, cx| {
                if !p.sides[i]
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
                        p.list(i, path, None, cx);
                        return;
                    }
                    p.fail(s, &b);
                    if let Some(o) = p.sides[i].opened.as_mut() {
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
                            if let Some(o) = p.sides[i]
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
                            if let Some(o) = p.sides[i]
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
                let code = p.sides[i].code.clone();
                let line = p.sides[i].pending_line.take();
                code.update(cx, |code, cx| {
                    code.set_lang(syntax::lang(&name), cx);
                    code.set(draft.as_ref().map_or(&text, |d| &d.text), cx);
                    if let Some(line) = line {
                        code.show_line(line.saturating_sub(1), cx);
                    }
                });
                let o = p.sides[i].opened.as_mut().unwrap();
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
    fn changed(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(o) = self.sides[i].opened.as_ref() else {
            return;
        };
        let text = self.sides[i].code.read(cx).text().to_owned();
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
    pub fn toggle_diff(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(opened) = self.sides[i].opened.as_mut() else {
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
                if let Some(opened) = page.sides[i]
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
    pub fn edit(&mut self, i: usize, w: &mut Window, cx: &mut Context<Self>) {
        if let Some(o) = self.sides[i].opened.as_mut() {
            o.editing = true;
            self.sides[i].code.update(cx, |c, cx| c.edit(true, w, cx));
        }
        cx.notify()
    }
    fn finish_edit(&mut self, i: usize, w: &mut Window, cx: &mut Context<Self>) {
        if self.sides[i].opened.as_ref().is_some_and(|o| !o.editing) {
            self.sides[i].opened = None;
            w.focus(&self.focus, cx);
            cx.notify();
            return;
        }
        let dirty = self.sides[i]
            .opened
            .as_ref()
            .is_some_and(|o| self.drafts.contains_key(&o.key()));
        if dirty {
            self.confirm_discard = Some(i);
        } else if let Some(o) = self.sides[i].opened.as_mut() {
            o.editing = false;
            self.sides[i].code.update(cx, |c, cx| c.edit(false, w, cx));
        }
        cx.notify()
    }
    pub fn keep_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.confirm_discard.take() {
            Code::focus(&self.sides[i].code, window, cx);
        }
        cx.notify();
    }

    pub fn save(&mut self, i: usize, cx: &mut Context<Self>) {
        self.write(i, open::Write::Save, cx);
    }
    pub fn cancel_conflict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.conflict.take() {
            Code::focus(&self.sides[i].code, window, cx)
        }
        cx.notify();
    }
    pub fn write(&mut self, i: usize, mode: open::Write, cx: &mut Context<Self>) {
        let Some(opened) = self.sides[i].opened.as_mut() else {
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
        let sent = self.sides[i].code.read(cx).text().to_owned();
        let (target, ordinal) = if mode == open::Write::Copy {
            let mut n = self.copy_ordinal;
            while self.sides[i]
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
                if !p.sides[i]
                    .opened
                    .as_ref()
                    .is_some_and(|o| o.node == node && o.path == path)
                {
                    return;
                }
                if status != success {
                    p.sides[i].opened.as_mut().unwrap().saving = false;
                    if status == 412 {
                        p.conflict = Some(i);
                        p.copy_ordinal = 1;
                    } else if status == 409 && mode == open::Write::Copy {
                        p.copy_ordinal = ordinal + 1;
                        p.write(i, mode, cx);
                    } else {
                        p.fail(status, &body)
                    }
                } else {
                    p.conflict = None;
                    let now = p.sides[i].code.read(cx).text().to_owned();
                    let o = p.sides[i].opened.as_mut().unwrap();
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
                    p.drafts.remove(&old_key);
                    if let Some(d) = open::acknowledged(&now, &sent) {
                        p.drafts.insert(new_key, d);
                    }
                    if mode == open::Write::Copy {
                        p.list(i, folder, Some(paths::base_name(&target).to_owned()), cx);
                    }
                }
                cx.notify();
            },
            cx,
        )
    }
    pub fn search_key(
        &mut self,
        i: usize,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if e.keystroke.key == "escape" && self.sides[i].search.read(cx).is_focused(window) {
            self.sides[i]
                .search
                .update(cx, |field, cx| field.set("", false, cx));
            self.sides[i].pane.filter.clear();
            self.sides[i].pane.find = None;
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
        let i = self.active;
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
                self.sides[i].pane.step(-1);
            }
            "down" | "j" => {
                self.sides[i].pane.step(1);
            }
            "home" => {
                self.sides[i].pane.step(isize::MIN);
            }
            "end" => {
                self.sides[i].pane.step(isize::MAX);
            }
            "tab" => self.active = 1 - i,
            "enter" => {
                let selected = self.sides[i]
                    .pane
                    .selection()
                    .map(|e| (e.name.clone(), e.folder));
                if let (Some((name, folder)), Some(dir)) =
                    (selected, self.sides[i].pane.path.clone())
                {
                    let path = paths::join(&dir, &name);
                    if folder {
                        self.list(i, path, None, cx)
                    } else {
                        self.open(i, path, window, cx)
                    }
                }
            }
            "backspace" => {
                if let Some(up) = self.sides[i].pane.up() {
                    let select = self.sides[i].pane.came_from();
                    self.list(i, up, select, cx)
                }
            }
            "f5" => {
                if let Some(path) = self.sides[i].pane.path.clone() {
                    self.list(i, path, None, cx)
                }
            }
            "delete" => self.mutate(i, "delete", cx),
            "/" => parvi_app_kit::widgets::field::focus(&self.sides[i].search, window, cx),
            "f2" => {
                if let Some(name) = self.sides[i]
                    .pane
                    .selection()
                    .map(|entry| entry.name.clone())
                {
                    self.sides[i]
                        .name
                        .update(cx, |field, cx| field.set(&name, true, cx));
                    parvi_app_kit::widgets::field::focus(&self.sides[i].name, window, cx);
                }
            }
            "escape" => {
                if self.sides[i].opened.is_some() {
                    self.sides[i].opened = None
                } else {
                    self.sides[i].pane.find = None;
                    self.sides[i].pane.filter.clear();
                    self.sides[i]
                        .search
                        .update(cx, |f, cx| f.set("", false, cx));
                }
            }
            "c" if m.control && m.shift => {
                if let Some(path) = self.sides[i].pane.selected_path() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(path))
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
    pub fn sort(&mut self, i: usize, column: store::listing::Column, cx: &mut Context<Self>) {
        self.sides[i].pane.sort = self.sides[i].pane.sort.toggled(column);
        self.keep(cx);
        cx.notify();
    }
    pub fn discard(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard = None;
        if let Some(opened) = self.sides[i].opened.as_mut() {
            self.drafts.remove(&opened.key());
            let saved = opened.saved.clone();
            opened.editing = false;
            self.sides[i].code.update(cx, |code, cx| {
                code.set(&saved, cx);
                code.edit(false, window, cx)
            });
        }
        cx.notify();
    }
    pub fn mutate(&mut self, i: usize, action: &'static str, cx: &mut Context<Self>) {
        let pane = &mut self.sides[i].pane;
        let Some(folder) = pane.path.clone() else {
            return;
        };
        if !pane.writable() {
            return;
        }
        let node = pane.node.clone();
        let name = self.sides[i].name.read(cx).text.trim().to_owned();
        let selected = self.sides[i].pane.selected_path();
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
                let Some(path) = self.sides[i].pane.press_delete() else {
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
                    let select = if let Some((from, to)) = moved {
                        store::drafts::move_drafts(&mut p.drafts, &node, &from, &to);
                        for side in &mut p.sides {
                            if let Some(opened) = side
                                .opened
                                .as_mut()
                                .filter(|o| o.node == node && paths::inside(&from, &o.path))
                            {
                                opened.path = format!("{to}{}", &opened.path[from.len()..]);
                                opened.name = paths::base_name(&opened.path).to_owned();
                            }
                        }
                        Some(paths::base_name(&to).to_owned())
                    } else if action == "file" || action == "folder" {
                        Some(name)
                    } else {
                        None
                    };
                    p.list(i, folder, select, cx)
                }
                cx.notify()
            },
            cx,
        );
    }
    pub(crate) fn pick_uploads(&mut self, i: usize, folder: String, cx: &mut Context<Self>) {
        let node = NodeId(self.sides[i].pane.node.clone());
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Upload files".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = picked.await;
            let _ = this.update(cx, |page, cx| match result {
                Ok(Ok(Some(paths))) => page.queue_uploads(i, node, folder, &paths, cx),
                Ok(Ok(None)) | Err(_) => {}
                Ok(Err(e)) => {
                    page.error = Some(format!("The file picker failed: {e}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(crate) fn upload_paths(
        &mut self,
        i: usize,
        folder: String,
        paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        let node = NodeId(self.sides[i].pane.node.clone());
        self.queue_uploads(i, node, folder, paths, cx);
    }
    fn queue_uploads(
        &mut self,
        i: usize,
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
                side: i,
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
                    if page.sides[upload.side].pane.node == upload.node.as_str()
                        && page.sides[upload.side].pane.path.as_deref()
                            == Some(upload.folder.as_str())
                    {
                        page.list(upload.side, upload.folder.clone(), Some(target_name), cx);
                    }
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
        i: usize,
        name: String,
        at: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.active = i;
        self.sides[i].pane.select(&name);
        self.sides[i].pane.armed = false;
        self.menu = Some((i, at));
        cx.notify();
    }
    pub fn toggle_pin(&mut self, i: usize, cx: &mut Context<Self>) {
        let pane = &self.sides[i].pane;
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
    pub fn open_pin(
        &mut self,
        i: usize,
        pin: store::Pin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changed_node = self.sides[i].pane.node != pin.node;
        self.sides[i].pane.choose_node(&pin.node);
        let folder = if pin.folder {
            pin.path.clone()
        } else {
            paths::parent(&pin.path)
        };
        let selected = (!pin.folder).then(|| paths::base_name(&pin.path).to_owned());
        if changed_node {
            self.sides[i].pane.path = Some(folder);
            self.sides[i].pane.selected = selected;
            self.roots(i, cx);
        } else {
            self.list(i, folder, selected, cx);
        }
        if !pin.folder {
            self.open(i, pin.path, window, cx);
        }
    }
    pub fn download(&mut self, i: usize, cx: &mut Context<Self>) {
        let pane = &self.sides[i].pane;
        let Some(path) = pane.selected_path() else {
            return;
        };
        if pane.selection().is_none_or(|e| e.folder) {
            return;
        }
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
        let node = NodeId(pane.node.clone());
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
                            Ok(()) => {
                                page.notice =
                                    Some(format!("Downloaded to {}", destination.display()))
                            }
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
    pub fn copy_to_other(&mut self, i: usize, cx: &mut Context<Self>) {
        let other = 1 - i;
        let Some(source) = self.sides[i].pane.selected_path() else {
            return;
        };
        let Some(folder) = self.sides[other].pane.path.clone() else {
            return;
        };
        let node = self.sides[i].pane.node.clone();
        if !self.sides[i].pane.writable()
            || !self.sides[other].pane.writable()
            || node != self.sides[other].pane.node
        {
            self.error = Some("Copy is available only within one reachable node.".into());
            cx.notify();
            return;
        }
        let request = parvi_files_contract::TransferRequest {
            node: node.clone(),
            path: source.clone(),
            target_node: node,
            folder: folder.clone(),
            id: store::transfers::new_id(),
        };
        self.transfers
            .push(store::transfers::Transfer::new(store::transfers::Kept {
                name: paths::base_name(&source).to_owned(),
                from: store::transfers::End {
                    shown: source,
                    ..Default::default()
                },
                to: store::transfers::End {
                    shown: folder,
                    ..Default::default()
                },
                request,
                state: store::transfers::State::Running,
                bytes: 0,
                total: 0,
                error: None,
                finished: None,
            }));
        self.keep(cx);
        self.run_copy(self.transfers.len() - 1, cx);
    }
    pub fn retry_copy(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(transfer) = self.transfers.get_mut(index) {
            transfer.restart();
            self.keep(cx);
            self.run_copy(index, cx)
        }
    }
    fn run_copy(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(transfer) = self.transfers.get(index) else {
            return;
        };
        let job = transfer.kept.request.clone();
        let id = job.id.clone();
        let request = Request::new("POST", NodeId(job.node.clone()), "/api/files/bridge/start")
            .json(&serde_json::to_value(&job).expect("copy request is JSON"));
        self.request(
            request,
            move |page, status, body, cx| {
                if status != 202 {
                    page.fail(status, &body);
                    let message = page.error.clone().unwrap_or_default();
                    if let Some(transfer) = page.transfers.get_mut(index).filter(|t| t.id() == id) {
                        transfer.wait(&message);
                    }
                    page.keep(cx);
                    cx.notify();
                    return;
                }
                let uri = format!("/api/files/bridge/events?id={}", enc(&id));
                let request = Request::get(NodeId(job.node.clone()), uri)
                    .within(std::time::Duration::from_secs(120));
                page.request(
                    request,
                    move |page, status, body, cx| {
                        if status != 200 {
                            page.fail(status, &body);
                            let message = page.error.clone().unwrap_or_default();
                            if let Some(transfer) =
                                page.transfers.get_mut(index).filter(|t| t.id() == id)
                            {
                                transfer.wait(&message);
                            }
                        } else {
                            let result = String::from_utf8_lossy(&body);
                            let progress = result
                                .lines()
                                .filter_map(|l| l.strip_prefix("data: "))
                                .filter_map(|l| {
                                    serde_json::from_str::<parvi_files_contract::Progress>(l).ok()
                                })
                                .next_back();
                            if let Some(transfer) =
                                page.transfers.get_mut(index).filter(|t| t.id() == id)
                            {
                                if let Some(progress) = progress {
                                    transfer.progress(&progress, parvi_app_kit::hub::now_ms());
                                    if progress.state == "running" {
                                        transfer.wait("The copy did not finish; retry it.");
                                    }
                                    if progress.state == "done" {
                                        for side in 0..2 {
                                            if page.sides[side].pane.node == job.target_node
                                                && page.sides[side].pane.path.as_deref()
                                                    == Some(&job.folder)
                                            {
                                                page.list(
                                                    side,
                                                    job.folder.clone(),
                                                    Some(paths::base_name(&job.path).to_owned()),
                                                    cx,
                                                );
                                            }
                                        }
                                    }
                                } else {
                                    transfer.wait("The copy did not report progress; retry it.");
                                }
                            }
                        }
                        page.keep(cx);
                        cx.notify();
                    },
                    cx,
                );
            },
            cx,
        );
    }
    pub fn play(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(opened) = self.sides[i].opened.as_ref() else {
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
        i: usize,
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
        if link.is_empty() || self.sides[i].pane.node != node {
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
        if !self.sides[i].pane.roots.is_empty()
            && paths::root_of(&self.sides[i].pane.roots, &at).is_none()
        {
            self.error = Some("That link is outside the roots.".into());
            cx.notify();
            return;
        }
        self.list(
            i,
            paths::parent(&at),
            Some(paths::base_name(&at).to_owned()),
            cx,
        );
        self.open(i, at, window, cx);
    }
    pub fn send(&self, i: usize, cx: &mut Context<Self>) {
        let path = self.sides[i]
            .opened
            .as_ref()
            .map(|o| o.path.clone())
            .or_else(|| self.sides[i].pane.selected_path());
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
    fn keep(&self, cx: &mut Context<Self>) {
        let mut k: store::Kept = self.host.setting(cx).unwrap_or_default();
        if self.dock {
            k.dock = self.sides[0].pane.kept();
            k.dock_file = self.sides[0].opened.as_ref().map(|o| o.path.clone());
        } else {
            k.panes = std::array::from_fn(|i| self.sides[i].pane.kept());
            k.transfers = self.transfers.iter().map(|t| t.kept.clone()).collect();
        }
        let _ = self.host.set_setting(&k, cx);
    }
}

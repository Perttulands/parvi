//! Page state, file requests and user actions.
use crate::{
    open::{self, Draft, Opened, Show},
    store::{self, pane::Pane, paths},
};
use gpui::{App, AppContext, Context, Entity, FocusHandle, KeyDownEvent, Window};
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
use std::collections::HashMap;

pub struct Side {
    pub pane: Pane,
    pub code: Entity<Code>,
    pub search: Entity<TextField>,
    pub name: Entity<TextField>,
    pub opened: Option<Opened>,
}
pub struct FilesPage {
    pub host: Host,
    pub focus: FocusHandle,
    pub sides: [Side; 2],
    pub active: usize,
    pub shown: bool,
    pub drafts: HashMap<(String, String), Draft>,
    pub error: Option<String>,
    pub beside: bool,
    restored: bool,
    beside_from: &'static str,
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
impl Page for FilesPage {
    const ID: &'static str = "files";
    const TITLE: &'static str = "Files";
    const AREA: Option<&'static str> = Some("files");
    const ANSWERS: &'static [Answers] = &[Answers::OpenPath, Answers::Reveal("path")];
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
        _: &NodeId,
        state: Option<LinkState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if state == Some(LinkState::Open) {
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
                    self.sides[self.active].pane.node = node.to_string();
                    self.open(self.active, path, window, cx)
                }
            }
            Ask::Reveal { what, id, node, .. } if what == "path" => {
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
        let kept: store::Kept = host.setting(cx).unwrap_or_default();
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
                move |page, field, event, _, cx| match event {
                    FieldEvent::Changed => {
                        page.sides[i].pane.filter = field.read(cx).text.clone();
                        cx.notify();
                    }
                    FieldEvent::Submit => {
                        let q = field.read(cx).text.clone();
                        page.find(i, q, cx)
                    }
                    FieldEvent::Down => {
                        page.sides[i].pane.step(1);
                        cx.notify();
                    }
                    _ => {}
                },
            )
            .detach();
            Side {
                pane: Pane::from_kept(&kept.panes[i]),
                code,
                search,
                name,
                opened: None,
            }
        });
        Self {
            focus: cx.focus_handle().tab_stop(true),
            host,
            sides,
            active: 0,
            shown: false,
            drafts: HashMap::new(),
            error: None,
            beside: false,
            restored: false,
            beside_from: "",
        }
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
        let view = cx.new(|cx| FilesPage::new(host.clone(), window, cx));
        view.update(cx, |p, cx| {
            p.beside = true;
            p.beside_from = from;
            p.sides[0].pane.node = node.to_string();
            p.open(0, path, window, cx)
        });
        host.show_beside(from, view.into(), cx);
    }
    pub fn close_beside(&self, cx: &mut Context<Self>) {
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
    }
    fn roots(&mut self, i: usize, cx: &mut Context<Self>) {
        let node = self.sides[i].pane.node.clone();
        self.request(
            Request::get(NodeId(node), "/api/files/roots"),
            move |p, s, b, cx| {
                if s != 200 {
                    p.fail(s, &b);
                    cx.notify();
                    return;
                }
                match serde_json::from_slice::<NodeRoots>(&b) {
                    Ok(r) => {
                        p.sides[i].pane.roots = r.roots;
                        if let Some(path) = p.sides[i].pane.start_path(i) {
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
            Request::get(NodeId(node), route("folder", &path)),
            move |p, s, b, cx| {
                if p.sides[i].pane.asked != id {
                    return;
                }
                if s != 200 {
                    p.fail(s, &b)
                } else {
                    match serde_json::from_slice::<Listing>(&b) {
                        Ok(l) => {
                            p.sides[i].pane.listed(l.path, l.entries, select);
                            p.sides[i].opened = None;
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
        self.sides[i].pane.set_find(&q, false);
        if q.trim().is_empty() {
            cx.notify();
            return;
        }
        let node = self.sides[i].pane.node.clone();
        let uri = format!("{}&q={}", route("find", &path), enc(&q));
        self.request(
            Request::get(NodeId(node), uri),
            move |p, s, b, cx| {
                let found = if s == 200 {
                    serde_json::from_slice::<Matches>(&b).map_err(|e| e.to_string())
                } else {
                    Err(String::from_utf8_lossy(&b).into_owned())
                };
                p.sides[i].pane.found(&q, false, found);
                cx.notify()
            },
            cx,
        )
    }
    pub fn open(&mut self, i: usize, path: String, _: &mut Window, cx: &mut Context<Self>) {
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
                code.update(cx, |code, cx| {
                    code.set_lang(syntax::lang(&name), cx);
                    code.set(draft.as_ref().map_or(&text, |d| &d.text), cx)
                });
                let o = p.sides[i].opened.as_mut().unwrap();
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
    pub fn edit(&mut self, i: usize, w: &mut Window, cx: &mut Context<Self>) {
        if let Some(o) = self.sides[i].opened.as_mut() {
            o.editing = true;
            self.sides[i].code.update(cx, |c, cx| c.edit(true, w, cx));
        }
        cx.notify()
    }
    fn finish_edit(&mut self, i: usize, w: &mut Window, cx: &mut Context<Self>) {
        if let Some(o) = self.sides[i].opened.as_mut() {
            o.editing = false;
            self.sides[i].code.update(cx, |c, cx| c.edit(false, w, cx));
        }
        cx.notify()
    }
    pub fn save(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(o) = self.sides[i].opened.as_mut() else {
            return;
        };
        if o.saving {
            return;
        }
        o.saving = true;
        let node = o.node.clone();
        let path = o.path.clone();
        let base = o.base.clone();
        let sent = self.sides[i].code.read(cx).text().to_owned();
        let mut r = Request::new(
            "PUT",
            NodeId(node.clone()),
            format!("{}&base={}", route("file", &path), enc(&base)),
        );
        r.body = Some(("application/octet-stream".into(), sent.as_bytes().to_vec()));
        self.request(
            r,
            move |p, s, b, cx| {
                if !p.sides[i]
                    .opened
                    .as_ref()
                    .is_some_and(|o| o.node == node && o.path == path)
                {
                    return;
                }
                if s != 204 {
                    p.fail(s, &b);
                    p.sides[i].opened.as_mut().unwrap().saving = false;
                } else {
                    let now = p.sides[i].code.read(cx).text().to_owned();
                    let o = p.sides[i].opened.as_mut().unwrap();
                    o.saving = false;
                    o.saved = sent.clone();
                    o.base = open::version(sent.as_bytes());
                    let key = o.key();
                    if let Some(d) = open::acknowledged(&now, &sent) {
                        p.drafts.insert(key, d);
                    } else {
                        p.drafts.remove(&key);
                    }
                }
                cx.notify()
            },
            cx,
        )
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
            "f2" => parvi_app_kit::widgets::field::focus(&self.sides[i].name, window, cx),
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
    pub fn discard(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
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
        let node = pane.node.clone();
        let name = self.sides[i].name.read(cx).text.trim().to_owned();
        let selected = self.sides[i].pane.selected_path();
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
            Request::new(method, NodeId(node), uri),
            move |p, s, b, cx| {
                if !(200..300).contains(&s) {
                    p.fail(s, &b)
                } else {
                    p.list(i, folder, None, cx)
                }
                cx.notify()
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
        if node != self.sides[other].pane.node {
            self.error = Some("Copy is available only within one node.".into());
            cx.notify();
            return;
        }
        let id = crate::store::transfers::new_id();
        let payload = serde_json::json!({"node":node,"path":source,"target_node":node,"folder":folder,"id":id});
        let request =
            Request::new("POST", NodeId(node.clone()), "/api/files/bridge/start").json(&payload);
        self.request(
            request,
            move |page, status, body, cx| {
                if status != 202 {
                    page.fail(status, &body);
                    cx.notify();
                    return;
                }
                let uri = format!("/api/files/bridge/events?id={}", enc(&id));
                let request =
                    Request::get(NodeId(node), uri).within(std::time::Duration::from_secs(120));
                page.request(
                    request,
                    move |page, status, body, cx| {
                        if status != 200 {
                            page.fail(status, &body)
                        } else {
                            let result = String::from_utf8_lossy(&body);
                            if result
                                .lines()
                                .filter_map(|l| l.strip_prefix("data: "))
                                .filter_map(|l| {
                                    serde_json::from_str::<parvi_files_contract::Progress>(l).ok()
                                })
                                .next_back()
                                .is_some_and(|p| p.state == "done")
                            {
                                page.list(
                                    other,
                                    folder,
                                    Some(paths::base_name(&source).to_owned()),
                                    cx,
                                );
                            } else {
                                page.error = Some(
                                    "Copy did not finish; check the destination before retrying."
                                        .into(),
                                );
                            }
                        }
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
        k.panes = std::array::from_fn(|i| self.sides[i].pane.kept());
        let _ = self.host.set_setting(&k, cx);
    }
}

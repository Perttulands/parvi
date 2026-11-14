//! Drawing the page: the list (recent changes, the tree, or what a find
//! found) beside the opened file, in the shared page skeleton.
use crate::open::Show;
use crate::page::{FilesPage, Pick};
use crate::store::{format, paths, tree::Row};
use gpui::{
    AnyElement, Context, Div, MouseButton, MouseMoveEvent, Render, ScrollDelta, SharedString,
    Window, div, img, prelude::*, px, uniform_list,
};
use parvi_app_kit::theme::{Theme, size, theme};
use parvi_app_kit::widgets::kit::{self, Kind};
use parvi_app_kit::widgets::pane;
use parvi_files_contract::{Changed, Match};

/// Under each of these widths a file's header leaves out one more of its
/// least needed words. A view beside the terminals opens about 420 wide,
/// where the name needs the room the zoom steps would take.
const HEAD_NARROW: [f32; 3] = [640., 520., 440.];
/// JetBrains Mono's advance, as a share of its size: how many letters of a
/// name fit a width.
const ADVANCE: f32 = 0.6;
/// Recent changes shown before More.
const RECENT_FIRST: usize = 6;
/// A tree level's indent.
const INDENT: f32 = 12.;
/// The disclosure's column.
const DISCLOSURE: f32 = 12.;
/// Letters kept for a row's age.
const AGE: usize = 4;

/// A width as last laid out.
pub type Width = std::rc::Rc<std::cell::Cell<Option<f32>>>;

/// Records the width of what it is put in, and draws again when it
/// changed.
fn measure(width: Width) -> impl IntoElement {
    gpui::canvas(
        move |bounds, window, cx| {
            let now = Some(f32::from(bounds.size.width));
            if width.get() != now {
                width.set(now);
                // A refresh asked for while drawing is dropped, so ask after it.
                window.defer(cx, |window, _| window.refresh());
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

/// How many letters fit `width` pixels of text.
fn letters(width: f32) -> usize {
    (width / (size::TEXT * ADVANCE)).floor().max(4.) as usize
}

/// One line of the list.
#[derive(Clone, Debug)]
pub enum Line {
    Label(String),
    /// What changed; `true` for a file under the folder line opened above it.
    Recent(Changed, bool),
    More(usize),
    /// A word in place of rows: still reading, nothing, or why not.
    Note {
        text: String,
        depth: usize,
        error: bool,
    },
    Node(Row),
    Hit(usize, Match),
    InText(String),
}

impl Line {
    /// The pick this line is, when it can be picked.
    pub fn pick(&self) -> Option<Pick> {
        match self {
            Line::Recent(c, _) => Some(Pick::Recent(c.path.clone())),
            Line::More(_) => Some(Pick::More),
            Line::Node(r) => Some(Pick::Node(r.path.clone())),
            Line::Hit(i, _) => Some(Pick::Hit(*i)),
            Line::InText(_) => Some(Pick::InText),
            Line::Label(_) | Line::Note { .. } => None,
        }
    }
}

/// The lines the list shows now.
pub fn lines(p: &FilesPage) -> Vec<Line> {
    let mut out = Vec::new();
    if let Some(find) = &p.find {
        // Names first, then the lines inside files, each under its label.
        let quoted = format!("\u{201c}{}\u{201d}", find.query.trim());
        out.push(Line::Label("Names".into()));
        let names = found_lines(
            &find.found,
            0,
            format!("No file name has {quoted}"),
            &mut out,
        );
        let scope = find
            .inside
            .as_ref()
            .map(|i| i.folder.clone())
            .or_else(|| p.text_scope());
        if let Some(folder) = scope {
            out.push(Line::Label(format!(
                "Inside files in {}",
                paths::shown(&p.tree.roots, &folder)
            )));
            match &find.inside {
                Some(inside) => {
                    found_lines(
                        &inside.found,
                        names,
                        format!("No file here has {quoted}"),
                        &mut out,
                    );
                }
                None => out.push(Line::InText(quoted)),
            }
        }
        return out;
    }
    if let Some(Ok(recent)) = &p.recent
        && !recent.is_empty()
    {
        out.push(Line::Label("Recent".into()));
        let shown = if p.recent_more {
            recent.len()
        } else {
            RECENT_FIRST.min(recent.len())
        };
        for changed in &recent[..shown] {
            out.push(Line::Recent(changed.clone(), false));
            let Some((_, inside)) = p
                .recent_inside
                .as_ref()
                .filter(|(folder, _)| changed.files.is_some() && *folder == changed.path)
            else {
                continue;
            };
            let note = |text: String, error: bool| Line::Note {
                text,
                depth: 0,
                error,
            };
            match inside {
                None => out.push(note("Reading\u{2026}".into(), false)),
                Some(Err(why)) => out.push(note(why.clone(), true)),
                Some(Ok(files)) => {
                    out.extend(files.iter().cloned().map(|c| Line::Recent(c, true)));
                }
            }
        }
        if shown < recent.len() {
            out.push(Line::More(recent.len() - shown));
        }
        out.push(Line::Label("Folders".into()));
    }
    for row in p.tree.rows() {
        let failed = p.tree.failed.get(&row.path).cloned();
        let empty = row.open
            && p.tree
                .listings
                .get(&row.path)
                .is_some_and(|l| crate::store::listing::shown(l).is_empty());
        let depth = row.depth + 1;
        out.push(Line::Node(row));
        if let Some(why) = failed {
            out.push(Line::Note {
                text: why,
                depth,
                error: true,
            });
        } else if empty {
            out.push(Line::Note {
                text: "Empty".into(),
                depth,
                error: false,
            });
        }
    }
    if p.tree.roots.is_empty() {
        out.push(Line::Note {
            text: if p.tree.away {
                "The device does not answer".into()
            } else {
                "Reading…".into()
            },
            depth: 0,
            error: p.tree.away,
        });
    }
    out
}

/// One group of hits, numbered from `first`: still looking, why not, none,
/// or the hits (with a word first when there were more). Answers how many.
fn found_lines(
    found: &Option<Result<parvi_files_contract::Matches, String>>,
    first: usize,
    none: String,
    out: &mut Vec<Line>,
) -> usize {
    let note = |text: String, error: bool| Line::Note {
        text,
        depth: 0,
        error,
    };
    match found {
        None => {
            out.push(note("Looking\u{2026}".into(), false));
            0
        }
        Some(Err(e)) => {
            out.push(note(e.clone(), true));
            0
        }
        Some(Ok(m)) if m.matches.is_empty() => {
            out.push(note(none, false));
            0
        }
        Some(Ok(m)) => {
            // Said first: under a long list it would never be seen.
            if m.truncated {
                out.push(note(
                    format!("Only the first {}; type more of it", m.matches.len()),
                    false,
                ));
            }
            for (i, hit) in m.matches.iter().enumerate() {
                out.push(Line::Hit(first + i, hit.clone()));
            }
            m.matches.len()
        }
    }
}

impl FilesPage {
    /// Letters a row's words have in the list.
    fn row_letters(&self, indent: f32) -> usize {
        let width = self.list_room.get().unwrap_or(size::LIST) - 2. * size::ROW_PAD - indent;
        letters(width)
    }

    fn line(&self, line: Line, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let picked = line.pick().is_some_and(|l| self.pick.as_ref() == Some(&l));
        match line {
            Line::Label(text) => {
                let fit = (self.row_letters(0.) as f32 * size::TEXT / size::SMALL) as usize;
                pane::label(t, pane::middle(&text, fit))
                    .h(px(size::ROW))
                    .whitespace_nowrap()
                    .into_any_element()
            }
            Line::Note { text, depth, error } => div()
                .debug_selector({
                    let id = format!("files-note-{text}");
                    move || id
                })
                .h(px(size::ROW))
                .flex()
                .items_center()
                .pl(px(size::ROW_PAD + depth as f32 * INDENT + DISCLOSURE))
                .pr(px(size::ROW_PAD))
                .text_size(px(size::SMALL))
                .text_color(if error { t.error } else { t.dim })
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(text)
                .into_any_element(),
            Line::More(n) => pane::row(t, "recent-more", picked)
                .h(px(size::ROW))
                .debug_selector(|| "recent-more".into())
                .child(pane::meta(t, format!("{n} more"), None))
                .on_click(cx.listener(|p, _, w, cx| {
                    p.set_pick(Pick::More, cx);
                    p.activate(w, cx);
                }))
                .into_any_element(),
            Line::Recent(c, inside) => {
                let mut name = paths::base_name(&c.path).to_owned();
                let mut folder = paths::base_name(&paths::parent(&c.path)).to_owned();
                // A folder in which many files changed together.
                if let Some(n) = c.files {
                    name.push('/');
                    folder = format!("{n} files, in {folder}");
                }
                let indent = if inside { INDENT + DISCLOSURE } else { 0. };
                let room = self.row_letters(indent).saturating_sub(AGE + 1);
                let path = c.path.clone();
                let selector = format!("recent-{name}");
                self.file_row(
                    t,
                    SharedString::from(format!("recent-{}", c.path)),
                    picked,
                    &name,
                    Some(folder),
                    room,
                    Some(format::ago(c.modified)),
                    cx,
                )
                .pl(px(size::ROW_PAD - size::ROW_INSET + indent))
                .debug_selector(move || selector.clone())
                .tooltip(kit::tip(c.path.clone()))
                .on_mouse_down(
                    MouseButton::Right,
                    self.menu_at(c.path.clone(), c.files.is_some(), cx),
                )
                .on_click(cx.listener(move |p, _, w, cx| {
                    p.set_pick(Pick::Recent(path.clone()), cx);
                    p.activate(w, cx);
                }))
                .into_any_element()
            }
            Line::Hit(i, hit) => {
                let folder = paths::parent(&hit.path);
                let place = paths::root_of(&self.tree.roots, &folder)
                    .map(|r| {
                        let inner = paths::relative(&r.path, &folder);
                        if inner == folder {
                            r.name.clone()
                        } else {
                            paths::join(&r.name, inner)
                        }
                    })
                    .unwrap_or(folder);
                let name = match hit.line {
                    Some(line) => format!("{}:{line}", hit.name),
                    None => hit.name.clone(),
                };
                let room = self.row_letters(0.);
                let selector = format!("hit-{name}");
                let row = match hit.text.as_deref().map(str::trim) {
                    // A hit in the text shows the line it is in after the name.
                    Some(text) => pane::row(t, SharedString::from(format!("hit-{i}")), picked)
                        .h(px(size::ROW))
                        .child(
                            div()
                                .flex_none()
                                .child(pane::middle(&name, room.min(name.len()).max(room / 2))),
                        )
                        .child(pane::cell(text.to_owned()).text_color(t.dim)),
                    None => self.file_row(
                        t,
                        SharedString::from(format!("hit-{i}")),
                        picked,
                        &name,
                        Some(place),
                        room,
                        None,
                        cx,
                    ),
                };
                row.debug_selector(move || selector.clone())
                    .tooltip(kit::tip(hit.path.clone()))
                    .on_mouse_down(
                        MouseButton::Right,
                        self.menu_at(hit.path.clone(), false, cx),
                    )
                    .on_click(cx.listener(move |p, _, w, cx| {
                        p.set_pick(Pick::Hit(i), cx);
                        p.activate(w, cx);
                    }))
                    .into_any_element()
            }
            Line::InText(quoted) => pane::row(t, "files-in-text", picked)
                .h(px(size::ROW))
                .debug_selector(|| "files-in-text".into())
                .tooltip(kit::tip("Enter looks inside every file here"))
                .child(pane::cell(format!("Look for {quoted} inside them")).text_color(t.text2))
                .on_click(cx.listener(|p, _, w, cx| {
                    p.set_pick(Pick::InText, cx);
                    p.activate(w, cx);
                }))
                .into_any_element(),
            Line::Node(row) => self.tree_row(row, picked, t, cx),
        }
    }

    /// A file's row: its name shortened in the middle to `room` letters,
    /// the folder it is in dim after it while there is room, and `age`.
    #[allow(clippy::too_many_arguments)]
    fn file_row(
        &self,
        t: &Theme,
        id: SharedString,
        picked: bool,
        name: &str,
        folder: Option<String>,
        room: usize,
        age: Option<String>,
        _cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let shown = pane::middle(name, room);
        let left = room.saturating_sub(shown.chars().count() + 1);
        pane::row(t, id, picked)
            .h(px(size::ROW))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(px(size::HEAD_GAP))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(div().flex_none().child(shown))
                    .children(folder.filter(|_| left >= 4).map(|f| {
                        // Small letters: more of them fit. Shortened in its
                        // middle, a folder keeps its root and its nearest
                        // folders, the parts that tell two hits apart.
                        let fit = (left as f32 * size::TEXT / size::SMALL) as usize;
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(px(size::SMALL))
                            .text_color(t.dim)
                            .font_weight(gpui::FontWeight::NORMAL)
                            .child(pane::middle(&f, fit))
                    })),
            )
            .children(age.map(|a| pane::meta(t, a, None)))
    }

    fn tree_row(&self, row: Row, picked: bool, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let indent = row.depth as f32 * INDENT;
        let room = self.row_letters(indent + DISCLOSURE);
        let reading = self
            .opened
            .as_ref()
            .is_some_and(|o| o.path == row.path && o.node == self.tree.node);
        let path = row.path.clone();
        let folder = row.folder;
        let selector = format!("file-row-{}", row.name);
        let disclosure = match (row.folder, row.open) {
            (true, true) => "▾",
            (true, false) => "▸",
            _ => "",
        };
        pane::row(t, SharedString::from(format!("node-{}", row.path)), picked)
            .h(px(size::ROW))
            .gap_0()
            .debug_selector(move || selector.clone())
            .when(reading && !picked, |r| r.text_color(t.text))
            .when(picked, |r| {
                let id = format!("picked-{}", row.name);
                r.child(div().absolute().debug_selector(move || id))
            })
            .child(div().flex_none().w(px(indent)))
            .child(
                div()
                    .flex_none()
                    .w(px(DISCLOSURE))
                    .text_size(px(size::SMALL))
                    .text_color(t.dim)
                    .child(disclosure),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(pane::middle(&row.name, room)),
            )
            .when(row.reading, |r| r.child(pane::meta(t, "…", None)))
            .when(folder, |r| {
                let destination = path.clone();
                r.on_drop(cx.listener(move |p, dropped: &gpui::ExternalPaths, _, cx| {
                    p.upload_paths(destination.clone(), dropped.paths(), cx);
                    cx.stop_propagation();
                }))
            })
            .on_mouse_down(MouseButton::Right, self.menu_at(path.clone(), folder, cx))
            .on_click(cx.listener(move |p, _, w, cx| {
                w.focus(&p.focus, cx);
                p.set_pick(Pick::Node(path.clone()), cx);
                // A click on a folder opens or closes it; a file opens.
                p.activate(w, cx);
            }))
            .into_any_element()
    }

    fn menu_at(
        &self,
        path: String,
        folder: bool,
        cx: &mut Context<Self>,
    ) -> impl Fn(&gpui::MouseDownEvent, &mut Window, &mut gpui::App) + 'static {
        cx.listener(move |p, e: &gpui::MouseDownEvent, window, cx| {
            window.focus(&p.focus, cx);
            p.row_menu(path.clone(), folder, e.position, cx);
            cx.stop_propagation();
        })
    }

    /// The list column: its head holds the find (or the name being given)
    /// and New; under it the lines.
    fn list_column(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let field = match &self.naming {
            Some(_) => div()
                .id("files-name")
                .debug_selector(|| "files-name".into())
                .on_key_down(cx.listener(|page, e: &gpui::KeyDownEvent, w, cx| {
                    if e.keystroke.key == "escape" {
                        page.stop_naming(w, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(self.name_field.clone()),
            None => div()
                .id("files-search")
                .debug_selector(|| "files-search".into())
                .on_key_down(
                    cx.listener(|page, e: &gpui::KeyDownEvent, w, cx| page.search_key(e, w, cx)),
                )
                .child(self.query.clone()),
        };
        let mut head = pane::list_head(t, Some(field.into_any_element()))
            .child(measure(self.list_room.clone()));
        if let Some(naming) = &self.naming {
            head = head.child(
                pane::act(t, "files-name-apply", naming.verb())
                    .debug_selector(|| "files-name-apply".into())
                    .on_click(cx.listener(|p, _, w, cx| p.apply_naming(w, cx))),
            );
        } else if self.tree.writable() {
            head = head.child(
                pane::act(t, "files-new", "New")
                    .debug_selector(|| "files-new".into())
                    .tooltip(kit::tip(
                        "A file, a folder or an upload, in the folder picked",
                    ))
                    .on_click(cx.listener(|p, e: &gpui::ClickEvent, _, cx| {
                        if let Some(folder) = p.tree.folder_here() {
                            p.row_menu(folder, true, e.position(), cx);
                        }
                    })),
            );
        }
        head = head.children(self.beside_acts(t, cx));
        let n = lines(self).len();
        let body = uniform_list(
            "files-list",
            n,
            cx.processor(|p, range: std::ops::Range<usize>, _, cx| {
                let t = theme(cx).clone();
                let all = lines(p);
                range
                    .filter_map(|at| all.get(at).cloned())
                    // Each row takes the list's whole width.
                    .map(|line| {
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .child(p.line(line, &t, cx))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.list_scroll)
        .flex_1()
        .debug_selector(|| "files-list".into());
        let drop_into = self.tree.folder_here();
        pane::list_frame(t, head, body)
            .when(self.beside, |l| l.flex_1().min_w_0().border_r_0())
            .on_drop(cx.listener(move |p, dropped: &gpui::ExternalPaths, _, cx| {
                if let Some(folder) = drop_into.clone() {
                    p.upload_paths(folder, dropped.paths(), cx);
                    cx.stop_propagation();
                }
            }))
    }

    /// The viewer's head: the file's folder dim and its name, Send, then
    /// what can be done with it now.
    fn viewer_head(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let mut head = pane::head(t)
            .relative()
            .child(measure(self.head_room.clone()));
        if self.beside {
            head = head.child(
                pane::act(t, "files-back", "Back")
                    .debug_selector(|| "files-back".into())
                    .tooltip(kit::tip("To the list, the file picked in it"))
                    .on_click(cx.listener(|p, _, _, cx| p.back_to_list(cx))),
            );
        }
        let Some(o) = &self.opened else {
            return head.child(div().flex_1());
        };
        // A narrow header leaves out the picture's pixel size and 1:1, then
        // the percentage, then the zoom steps; the name gives way in the
        // middle, keeping its extension.
        let narrow = self.head_room.get().map_or(0, |w| {
            HEAD_NARROW.iter().filter(|&&at| w < at).count() as u8
        });
        let room = self.name_room.get().map(letters);
        let name = room.map_or(o.name.clone(), |n| paths::elide_name(&o.name, n));
        let folder = paths::shown(&self.tree.roots, &paths::parent(&o.path));
        let folder_fits =
            room.is_none_or(|n| n >= o.name.chars().count() + folder.chars().count().min(12));
        head = head.child(
            div()
                .id("opened-name")
                .flex_1()
                .min_w_0()
                .relative()
                .flex()
                .overflow_hidden()
                .whitespace_nowrap()
                .tooltip(kit::tip(o.path.clone()))
                .debug_selector({
                    let id = format!("file-view-{}", o.name);
                    move || id.clone()
                })
                .child(measure(self.name_room.clone()))
                .when(!folder.is_empty() && folder_fits, |d| {
                    d.child(
                        div()
                            .min_w_0()
                            .flex_shrink_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(t.dim)
                            .child(format!("{folder}/")),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(name),
                ),
        );
        let text = matches!(o.show, Show::Text { .. });
        let dirty = self.drafts.contains_key(&o.key());
        if o.editing {
            if o.changed_on_disk {
                head = head.child(
                    pane::meta(t, "Changed on disk", Some(t.error))
                        .debug_selector(|| "files-changed-on-disk".into()),
                );
            } else if dirty || o.saving {
                head = head.child(
                    pane::meta(t, if o.saving { "Saving…" } else { "Unsaved" }, None)
                        .debug_selector(|| "files-unsaved".into()),
                );
            }
            // Save stands out only while there is something to save.
            head = head
                .child(
                    kit::button(
                        t,
                        "files-save",
                        "Save",
                        if dirty { Kind::Primary } else { Kind::Quiet },
                    )
                    .debug_selector(|| "files-save".into())
                    .tooltip(kit::tip("Save (Ctrl+S)"))
                    .when(!dirty, |b| b.opacity(parvi_app_kit::theme::fade::IDLE))
                    .on_click(cx.listener(|p, _, _, cx| p.save(cx))),
                )
                .child(
                    pane::act(t, "files-discard", if dirty { "Discard" } else { "Done" })
                        .debug_selector(|| "files-discard".into())
                        .tooltip(kit::tip("Esc"))
                        .on_click(cx.listener(|p, _, w, cx| p.discard(w, cx))),
                );
            return head;
        }
        if narrow < 3 || !self.beside {
            head = head.child(
                pane::act(t, "files-send", "Send")
                    .debug_selector(|| "files-send".into())
                    .tooltip(kit::tip(if text {
                        "Send it, or the lines selected, to a session"
                    } else {
                        "Send it to a session"
                    }))
                    .on_click(cx.listener(|p, _, _, cx| p.send(cx))),
            );
        }
        if let Some(zoom) = self.zoom_bar(t, narrow, cx) {
            head = head.child(zoom);
        }
        if o.diff.is_some() {
            head = head.child(
                pane::act(
                    t,
                    "files-changes",
                    if o.diff_open { "File" } else { "Changes" },
                )
                .debug_selector(|| "files-changes".into())
                .tooltip(kit::tip(if o.diff_open {
                    "Back to the file"
                } else {
                    "What changed since the last commit"
                }))
                .on_click(cx.listener(|p, _, _, cx| p.toggle_diff(cx))),
            );
        }
        if o.editable() {
            head = head.child(
                pane::act(t, "files-edit", "Edit")
                    .debug_selector(|| "files-edit".into())
                    .on_click(cx.listener(|p, _, w, cx| p.edit(w, cx))),
            );
        } else if text && !matches!(o.show, Show::Reading) {
            head = head.child(pane::meta(t, "Too large to edit here", None));
        }
        head = head.child(
            pane::act(t, "files-more", "\u{2026}")
                .debug_selector(|| "files-more".into())
                .tooltip(kit::tip("Long lines, and the file's menu"))
                .on_click(
                    cx.listener(|p, e: &gpui::ClickEvent, _, cx| p.file_menu(e.position(), cx)),
                ),
        );
        head.children(self.beside_acts(t, cx))
    }

    /// Beside another page: Expand (where that page can give its whole
    /// room) and Close.
    fn beside_acts(&self, t: &Theme, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        if !self.beside {
            return vec![];
        }
        let from = self.beside_from;
        let mut acts: Vec<gpui::AnyElement> = vec![];
        if let Some(whole) = self.host.whole_beside(from, cx) {
            acts.push(
                kit::whole_button(t, whole)
                    .on_click(cx.listener(move |p, _, _, cx| p.host.toggle_whole_beside(from, cx)))
                    .into_any_element(),
            );
        }
        acts.push(
            pane::act(t, "files-close-beside", "Close")
                .debug_selector(|| "files-close-beside".into())
                .tooltip(kit::tip("Esc"))
                .on_click(cx.listener(|p, _, _, cx| p.close_beside(cx)))
                .into_any_element(),
        );
        acts
    }

    /// A decision the page waits on, on a line under the viewer's head.
    fn decision(&self, t: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        let line = |words: String| {
            div()
                .flex_none()
                .min_h(px(size::TAB_BAR))
                .px(px(size::HEAD_PAD))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(size::HEAD_GAP))
                .border_b_1()
                .border_color(t.divider)
                .child(div().flex_1().min_w_0().text_color(t.text).child(words))
        };
        if self.conflict {
            return Some(
                line("It changed on disk since you began. Your edit is kept.".into())
                    .child(
                        kit::button(t, "conflict-copy", "Save as a copy", Kind::Primary)
                            .debug_selector(|| "conflict-copy".into())
                            .on_click(
                                cx.listener(|p, _, _, cx| p.write(crate::open::Write::Copy, cx)),
                            ),
                    )
                    .child(
                        pane::act(t, "conflict-overwrite", "Overwrite theirs")
                            .debug_selector(|| "conflict-overwrite".into())
                            .on_click(cx.listener(|p, _, _, cx| {
                                p.write(crate::open::Write::Overwrite, cx)
                            })),
                    )
                    .child(
                        pane::act(t, "conflict-cancel", "Keep editing")
                            .on_click(cx.listener(|p, _, w, cx| p.cancel_conflict(w, cx))),
                    ),
            );
        }
        if self.confirm_discard {
            return Some(
                line("Your changes will be lost.".into())
                    .child(
                        kit::button(t, "files-confirm-discard", "Discard", Kind::Primary)
                            .debug_selector(|| "files-confirm-discard".into())
                            .on_click(cx.listener(|p, _, w, cx| p.discard(w, cx))),
                    )
                    .child(
                        pane::act(t, "files-keep-editing", "Keep editing")
                            .debug_selector(|| "files-keep-editing".into())
                            .on_click(cx.listener(|p, _, w, cx| p.keep_editing(w, cx))),
                    ),
            );
        }
        if self.confirm_close_beside {
            return Some(
                line("Closing loses your unsaved changes.".into())
                    .child(
                        kit::button(t, "files-close-discard", "Discard and close", Kind::Primary)
                            .debug_selector(|| "files-close-discard".into())
                            .on_click(cx.listener(|p, _, _, cx| p.discard_and_close_beside(cx))),
                    )
                    .child(
                        pane::act(t, "files-close-cancel", "Keep editing")
                            .on_click(cx.listener(|p, _, _, cx| p.cancel_close_beside(cx))),
                    ),
            );
        }
        if let Some(name) = self.upload_conflict_name() {
            return Some(
                line(format!("{name} is already there."))
                    .child(
                        kit::button(t, "upload-replace", "Replace", Kind::Primary)
                            .debug_selector(|| "upload-replace".into())
                            .on_click(cx.listener(|p, _, _, cx| p.resolve_upload(Some(true), cx))),
                    )
                    .child(
                        pane::act(t, "upload-keep", "Keep both")
                            .debug_selector(|| "upload-keep".into())
                            .on_click(cx.listener(|p, _, _, cx| p.resolve_upload(Some(false), cx))),
                    )
                    .child(
                        pane::act(t, "upload-cancel", "Skip")
                            .on_click(cx.listener(|p, _, _, cx| p.resolve_upload(None, cx))),
                    ),
            );
        }
        None
    }

    fn viewer_body(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(o) = &self.opened else {
            return pane::empty(t, "No file open", None).into_any_element();
        };
        if o.diff_open
            && let Some(diff) = &o.diff
        {
            return self.diff(t, diff).into_any_element();
        }
        match &o.show {
            Show::Reading => pane::empty(t, "Reading…", None).into_any_element(),
            Show::Failed(why) => div()
                .p(px(size::PAD))
                .debug_selector(|| "files-failed".into())
                .child(kit::note(t, why.clone(), Some(t.error)))
                .into_any_element(),
            Show::Not(why) => {
                let node = parvi_core::NodeId(o.node.clone());
                let path = o.path.clone();
                pane::empty(
                    t,
                    why.clone(),
                    Some(
                        pane::act(t, "viewer-download", "Download")
                            .debug_selector(|| "viewer-download".into())
                            .on_click(cx.listener(move |p, _, _, cx| {
                                p.download_path(node.clone(), path.clone(), cx)
                            }))
                            .into_any_element(),
                    ),
                )
                .into_any_element()
            }
            Show::Picture { image, natural } => self.picture(image, *natural, cx),
            Show::Model(m) => self.model(t, m, cx),
            Show::Media(_) => pane::empty(
                t,
                "It plays in this computer's own player",
                Some(
                    kit::button(t, "files-play", "Open", Kind::Primary)
                        .debug_selector(|| "files-play".into())
                        .on_click(cx.listener(|p, _, _, cx| p.play(cx)))
                        .into_any_element(),
                ),
            )
            .into_any_element(),
            Show::Text {
                markdown: Some(blocks),
            } if !o.editing && !o.source && !self.drafts.contains_key(&o.key()) => {
                let node = o.node.clone();
                let folder = paths::parent(&o.path);
                let me = cx.entity().downgrade();
                let on_link =
                    std::rc::Rc::new(move |link: &str, window: &mut Window, cx: &mut gpui::App| {
                        let _ = me.update(cx, |page, cx| {
                            page.follow_link(&node, &folder, link, window, cx)
                        });
                    });
                let look = parvi_app_kit::widgets::markdown::Look {
                    t,
                    refs: false,
                    on_link: Some(on_link),
                    select: None,
                };
                div()
                    .id("files-markdown-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .debug_selector(|| "files-markdown-rendered".into())
                    .child(
                        div()
                            .w_full()
                            .max_w(px(size::READING))
                            .p(px(size::PAD))
                            .debug_selector(|| "files-markdown-content".into())
                            .child(parvi_app_kit::widgets::markdown::render(
                                &look,
                                "file-markdown",
                                blocks,
                            )),
                    )
                    .into_any_element()
            }
            Show::Text { .. }
                if o.pretty.is_some()
                    && !o.editing
                    && !o.source
                    && !self.drafts.contains_key(&o.key()) =>
            {
                div()
                    .size_full()
                    .debug_selector(|| "files-json-formatted".into())
                    .child(self.pretty_code.clone())
                    .into_any_element()
            }
            Show::Text { .. } => {
                let selector = format!("files-code-top-{}", self.code.read(cx).top_line() + 1);
                div()
                    .size_full()
                    .debug_selector(move || selector.clone())
                    .child(self.code.clone())
                    .into_any_element()
            }
        }
    }

    fn diff(&self, t: &Theme, diff: &parvi_files_contract::Diff) -> Div {
        let mut rows = div()
            .id("file-diff")
            .debug_selector(|| "files-diff".into())
            .size_full()
            .overflow_scroll()
            .p(px(size::HEAD_PAD))
            .flex()
            .flex_col();
        for row in crate::store::diff::rows(&diff.diff) {
            let (prefix, colour, fill) = match row.kind {
                crate::store::diff::Kind::Add => (
                    "+",
                    parvi_app_kit::theme::rgb(t.ansi[2]),
                    Some(parvi_app_kit::theme::alpha(
                        parvi_app_kit::theme::rgb(t.ansi[2]),
                        0.08,
                    )),
                ),
                crate::store::diff::Kind::Delete => (
                    "-",
                    parvi_app_kit::theme::rgb(t.ansi[1]),
                    Some(parvi_app_kit::theme::alpha(
                        parvi_app_kit::theme::rgb(t.ansi[1]),
                        0.08,
                    )),
                ),
                crate::store::diff::Kind::Hunk => ("", t.dim, None),
                crate::store::diff::Kind::Context => (" ", t.text2, None),
            };
            rows = rows.child(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_color(colour)
                    .when_some(fill, |d, f| d.bg(f))
                    .child(format!("{prefix}{}", row.text)),
            );
        }
        if diff.truncated {
            rows = rows.child(pane::meta(t, "The rest is not shown", None));
        }
        div().size_full().child(rows)
    }

    fn model(&self, t: &Theme, m: &crate::model::Preview, cx: &mut Context<Self>) -> AnyElement {
        fn with(p: &mut FilesPage, f: impl FnOnce(&mut crate::model::Preview)) {
            if let Some(Show::Model(model)) = p.opened.as_mut().map(|o| &mut o.show) {
                f(model)
            }
        }
        div()
            .id("model-stage")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .child(
                pane::act(t, "model-reset", "Reset view").on_click(cx.listener(|p, _, _, cx| {
                    with(p, |m| m.reset());
                    cx.notify()
                })),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|p, e: &gpui::MouseDownEvent, _, _| {
                    with(p, |m| {
                        m.last = Some((f32::from(e.position.x), f32::from(e.position.y)))
                    })
                }),
            )
            .on_mouse_move(cx.listener(|p, e: &MouseMoveEvent, _, cx| {
                with(p, |model| {
                    if e.dragging() {
                        let now = (f32::from(e.position.x), f32::from(e.position.y));
                        if let Some(last) = model.last {
                            model.yaw += (now.0 - last.0) * 0.01;
                            model.pitch = (model.pitch + (now.1 - last.1) * 0.01).clamp(-1.5, 1.5);
                            model.redraw();
                        }
                        model.last = Some(now);
                    } else {
                        model.last = None;
                    }
                });
                cx.notify();
            }))
            .on_scroll(cx.listener(|p, e: &gpui::ScrollEvent, _, cx| {
                with(p, |model| {
                    let y = match e.delta {
                        ScrollDelta::Lines(v) => v.y,
                        ScrollDelta::Pixels(v) => f32::from(v.y) / 20.,
                    };
                    model.zoom = (model.zoom * (y * 0.1).exp()).clamp(0.1, 10.);
                    model.redraw();
                });
                cx.notify();
                cx.stop_propagation();
            }))
            .child(img(m.image.clone()).max_w_full().max_h_full())
            .into_any_element()
    }

    /// Whether the file shows as text set to a reading width: Markdown,
    /// rendered.
    fn reading(&self) -> bool {
        self.opened.as_ref().is_some_and(|o| {
            matches!(o.show, Show::Text { markdown: Some(_) })
                && !o.editing
                && !o.source
                && !self.drafts.contains_key(&o.key())
        })
    }

    fn viewer(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let mut head = self.viewer_head(t, cx);
        if self.reading() {
            // The head shares the text's column, so its actions stand at
            // the right edge of what they act on (as pane::detail's do).
            head = div()
                .flex_none()
                .w_full()
                .h(px(size::TAB_BAR))
                .border_b_1()
                .border_color(t.divider)
                .child(
                    head.border_0()
                        .h_full()
                        .w_full()
                        .max_w(px(size::READING))
                        .px(px(size::PAD)),
                );
        }
        let body = div()
            .size_full()
            .flex()
            .flex_col()
            .children(self.decision(t, cx))
            .child(div().flex_1().min_h_0().child(self.viewer_body(t, cx)));
        pane::detail_frame(head, body)
            .debug_selector(|| "files-viewer".into())
            .when(self.beside, |d| d.border_r_1().border_color(t.divider))
    }
}

impl Render for FilesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.release_pictures(window);
        if self.newest_due {
            cx.defer_in(window, |page, window, cx| page.open_newest(window, cx));
        }
        let t = theme(cx).clone();
        let root = div()
            .debug_selector(|| "files-page".into())
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .bg(t.bg)
            .text_color(t.text);
        if self.beside {
            // Beside another page it is one column: the list, or the file
            // opened from it.
            return root.flex().child(if self.opened.is_some() {
                self.viewer(&t, cx)
            } else {
                self.list_column(&t, cx)
            });
        }
        root.child(pane::page(
            &t,
            self.list_column(&t, cx),
            self.viewer(&t, cx),
        ))
    }
}

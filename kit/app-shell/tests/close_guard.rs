//! The shell owns the window's one close handler. Pages, and views they
//! show elsewhere, guard their unsaved work through their Host: the window
//! closes when none holds any, and otherwise only once the operator chooses
//! to leave it. Two toy pages and a toy draft view, made here.

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, Window, div, prelude::*};
use parvi_app_kit::{Build, Host, Mounted, Page, mount};
use parvi_app_shell::Shell;
use std::cell::RefCell;

/// A page holding at most one unsaved thing, named.
struct Pen {
    host: Host,
    dirty: Option<String>,
}

/// The same, a second page.
struct Ink {
    dirty: Option<String>,
}

/// A view a page shows beside another page or in a dock.
struct Draft {
    dirty: Option<String>,
}

thread_local! {
    static PEN: RefCell<Option<Entity<Pen>>> = const { RefCell::new(None) };
    static INK: RefCell<Option<Entity<Ink>>> = const { RefCell::new(None) };
}

impl Page for Pen {
    const ID: &'static str = "pen";
    const TITLE: &'static str = "Pen";
}

impl Page for Ink {
    const ID: &'static str = "ink";
    const TITLE: &'static str = "Ink";
}

impl Render for Pen {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Render for Ink {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Render for Draft {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn pen(host: Host, window: &mut Window, cx: &mut gpui::App) -> Mounted {
    mount(host, window, cx, |host, _, cx| {
        let me = cx.entity();
        PEN.with(|p| *p.borrow_mut() = Some(me.clone()));
        host.guard_close(&me, |p: &Pen, _| p.dirty.clone(), cx);
        Pen { host, dirty: None }
    })
}

fn ink(host: Host, window: &mut Window, cx: &mut gpui::App) -> Mounted {
    mount(host, window, cx, |host, _, cx| {
        let me = cx.entity();
        INK.with(|p| *p.borrow_mut() = Some(me.clone()));
        host.guard_close(&me, |p: &Ink, _| p.dirty.clone(), cx);
        Ink { dirty: None }
    })
}

const PAGES: &[Build] = &[pen, ink];

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    cx.run_until_parked();
    let at = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
        .center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
}

/// The operator closes the window: whether it closed.
fn close(cx: &mut VisualTestContext) -> bool {
    let closed = cx.simulate_close();
    cx.run_until_parked();
    closed
}

fn asking(cx: &mut VisualTestContext) -> bool {
    cx.run_until_parked();
    cx.debug_bounds("close-guard").is_some()
}

fn shown(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<&'static str> {
    shell.read_with(cx, |s, _| s.shown_page())
}

#[gpui::test]
fn unsaved_work_holds_the_window_until_the_operator_leaves_it(cx: &mut TestAppContext) {
    // A node nobody answers on: the guard needs none.
    let node = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        format!("http://{}", l.local_addr().expect("its address"))
    };
    cx.executor().allow_parking();
    cx.update(parvi_app_shell::prepare);
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        parvi_app_shell::start(PAGES, Some(node), None, window, cx)
    });
    cx.run_until_parked();
    let pen = PEN.with(|p| p.borrow().clone()).expect("Pen was made");
    let ink = INK.with(|p| p.borrow().clone()).expect("Ink was made");
    let dirty = |cx: &mut VisualTestContext, pen_label: Option<&str>, ink_label: Option<&str>| {
        pen.update(cx, |p, _| p.dirty = pen_label.map(str::to_string));
        ink.update(cx, |p, _| p.dirty = ink_label.map(str::to_string));
    };

    // Nothing unsaved: it closes, and nothing asks.
    assert!(close(cx));
    assert!(!asking(cx));

    // Ink holds an edit: refused, Ink comes forward and the guard asks.
    dirty(cx, None, Some("notes.md"));
    assert!(!close(cx));
    assert!(asking(cx));
    assert_eq!(shown(&shell, cx), Some("ink"));

    // Keep editing: the guard goes, and closing still asks.
    click(cx, "close-stay");
    assert!(!asking(cx));
    assert!(!close(cx));
    assert!(asking(cx));
    cx.simulate_keystrokes("escape");
    assert!(!asking(cx), "Escape keeps editing too");

    // Saved: Ink says so, and it closes.
    dirty(cx, None, None);
    assert!(close(cx));

    // Both pages hold edits: neither hides the other.
    dirty(cx, Some("a.md"), Some("b.md"));
    let unsaved = shell.read_with(cx, |s, cx| s.hub().read(cx).unsaved(cx));
    assert_eq!(
        unsaved,
        [("pen", "a.md".to_string()), ("ink", "b.md".to_string())]
    );
    assert!(!close(cx));
    assert_eq!(shown(&shell, cx), Some("pen"));
    click(cx, "close-stay");
    dirty(cx, None, Some("b.md"));
    assert!(!close(cx), "Ink's edit holds it alone");
    click(cx, "close-stay");
    dirty(cx, None, None);

    // A view Pen shows elsewhere guards its own edit, until it is gone.
    let pen_host = pen.read_with(cx, |p, _| p.host.clone());
    let draft = cx.update(|_, cx| {
        let draft = cx.new(|_| Draft {
            dirty: Some("c.md".into()),
        });
        pen_host.guard_close(&draft, |d: &Draft, _| d.dirty.clone(), cx);
        draft
    });
    assert!(!close(cx));
    click(cx, "close-stay");
    drop(draft);
    cx.run_until_parked();
    assert!(close(cx), "a view that is gone holds nothing");

    // Close without saving: the window goes.
    dirty(cx, Some("a.md"), None);
    assert!(!close(cx));
    assert_eq!(cx.windows().len(), 1);
    click(cx, "close-leave");
    assert!(cx.windows().is_empty(), "the operator chose to leave");
}

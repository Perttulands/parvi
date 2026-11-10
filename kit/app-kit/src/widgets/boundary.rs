//! CHROTE's per-view error boundary (PRD, Components): a view that fails
//! while it draws is replaced by a notice naming it, with Try again, and
//! nothing else stops. The tab bar, the status line, the terminals and
//! their sockets carry on. The app is built with unwinding for this
//! (`profile.app` in the workspace `Cargo.toml`).

use super::kit;
use crate::theme::{Theme, size};
use gpui::{AnyElement, Div, Hsla, Stateful, div, prelude::*, px};
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

thread_local! {
    /// A failure in drawing that runs after a page has been built (a
    /// canvas painting), until the next frame takes it to its page.
    static LATE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The failure a deferred painter caught since the last frame, if any.
pub fn take_late() -> Option<String> {
    LATE.with(|l| l.borrow_mut().take())
}

/// Runs `draw`, or returns what it failed with.
pub fn guard<T>(draw: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(draw)).map_err(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "An unexpected error occurred".into())
    })
}

/// A canvas painter that cannot take the window down. Painting runs after
/// the page was built, outside its guard, so a failing paint is kept for
/// the next frame, which shows the page's failure with Try again.
pub fn paint<A>(
    f: impl Fn(gpui::Bounds<gpui::Pixels>, A, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl FnOnce(gpui::Bounds<gpui::Pixels>, A, &mut gpui::Window, &mut gpui::App) + 'static {
    move |b, a, window, cx| {
        if let Err(why) = guard(|| f(b, a, window, cx)) {
            LATE.with(|l| l.borrow_mut().get_or_insert(why).clone());
            // A refresh asked for while drawing is dropped, so ask after it.
            window.defer(cx, |window, _| window.refresh());
        }
    }
}

/// What a failed view shows instead: CHROTE's words, the failure, and the
/// button that draws it again.
pub fn failed(t: &Theme, view: &str, message: &str, retry: Stateful<Div>) -> AnyElement {
    let ink: Hsla = t.text2;
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(t.bg)
        .child(
            div()
                .max_w(px(size::NOTICE))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .child(div().text_size(px(28.)).text_color(t.error).child("⚠"))
                .child(
                    div()
                        .text_size(px(16.))
                        .text_color(t.text)
                        .child("Something went wrong"),
                )
                .child(kit::note(t, format!("{view}: {message}"), Some(ink)))
                .child(kit::note(t, "The other tabs carry on.", Some(t.dim)))
                .child(retry),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failing_draw_comes_back_as_its_words() {
        assert_eq!(guard(|| 3), Ok(3));
        let got = guard(|| -> u8 { panic!("index out of bounds") });
        let owned = guard(|| -> u8 { panic!("{} broke", "Fleet") });
        assert_eq!(got, Err("index out of bounds".into()));
        assert_eq!(owned, Err("Fleet broke".into()));
    }

    #[test]
    fn a_late_failure_waits_for_the_next_frame_once() {
        LATE.with(|l| *l.borrow_mut() = Some("chart broke".into()));
        assert_eq!(take_late().as_deref(), Some("chart broke"));
        assert_eq!(take_late(), None);
    }
}

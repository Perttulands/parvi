//! The shared kit: every control is drawn from here, at CHROTE's measures,
//! never improvised in a view.

use crate::theme::{Chrome, Theme, chrome, size};
use gpui::{App, Div, ElementId, FontWeight, Hsla, SharedString, Stateful, div, prelude::*, px};

/// A section label: small, upper case, dim.
pub fn label(t: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(size::SMALL))
        .text_color(t.dim)
        .child(text.into().to_uppercase())
}

/// A page's name at the head of the page: every page draws it the same.
pub fn page_title(t: &Theme, text: impl Into<SharedString>) -> Div {
    let text = text.into();
    let selector = format!("page-title-{text}");
    div()
        .debug_selector(move || selector)
        .flex_none()
        .text_size(px(crate::theme::board::HEADING))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.text)
        .child(text)
}

/// The bar at a page's head, as high as the tab bar: its title, a note,
/// and its actions to the right.
pub fn page_bar(t: &Theme) -> Div {
    div()
        .flex_none()
        .h(px(size::TAB_BAR))
        .px(px(crate::theme::board::PAD))
        .flex()
        .items_center()
        .gap(px(12.))
        .border_b_1()
        .border_color(t.divider)
}

/// A small switch, reachable with Tab and flipped with Space or Enter.
pub fn toggle(t: &Theme, id: impl Into<ElementId>, on: bool) -> Stateful<Div> {
    let accent = t.accent;
    div()
        .id(id)
        .tab_index(0)
        .w(px(36.))
        .h(px(20.))
        .flex_none()
        .rounded_full()
        .p(px(2.))
        .flex()
        .when(on, |d| d.justify_end())
        .bg(if on { t.accent } else { t.raised })
        .border_1()
        .border_color(if on { t.accent } else { t.divider })
        .focus(move |s| s.border_color(accent))
        .cursor_pointer()
        .child(
            div()
                .size(px(14.))
                .rounded_full()
                .bg(if on { t.on_accent() } else { t.text2 }),
        )
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The one button that acts: accent fill.
    Primary,
    /// Outlined.
    Plain,
    /// Words only, for a header or a row.
    Quiet,
}

/// How the tab bar draws its tabs: spaced out where the window has room
/// for every page's tab, close together where it has not. The shell sets
/// it as it draws the bar; every tab on the bar reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabStyle {
    pub compact: bool,
}

impl gpui::Global for TabStyle {}

/// A tab's padding either side, spaced out and compact.
pub const TAB_PAD: f32 = 16.;
pub const TAB_PAD_COMPACT: f32 = 10.;

fn compact(cx: &App) -> bool {
    cx.try_global::<TabStyle>().is_some_and(|s| s.compact)
}

/// A tab's label: its words as written.
pub fn tab_label(title: &str) -> Div {
    let selector = format!("tab-label-{title}");
    div()
        .debug_selector(move || selector)
        .child(title.to_string())
}

/// One tab on the tab bar, drawn as the chrome draws tabs. It never
/// shrinks. The bar it sits on draws its own bottom rule first, so the
/// active card covers it and joins the page below.
pub fn tab(t: &Theme, id: impl Into<ElementId>, active: bool, cx: &App) -> Stateful<Div> {
    let chrome = chrome(cx);
    let d = div()
        .id(id)
        .flex_none()
        .px(px(if compact(cx) {
            TAB_PAD_COMPACT
        } else {
            TAB_PAD
        }))
        .flex()
        .items_center()
        .gap(px(6.))
        .cursor_pointer()
        .text_size(px(size::TEXT))
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(if active { t.text } else { t.text2 })
        .hover(hovered(t, chrome == Chrome::Pills && !active));
    match chrome {
        Chrome::Tabs => d
            .rounded_t(px(size::RADIUS))
            .border_1()
            .border_b_0()
            .border_color(if active {
                t.divider
            } else {
                gpui::transparent_black()
            })
            .when(active, |d| d.bg(t.surface)),
        // Every tab keeps the underline's room, so none moves when chosen.
        Chrome::Line => d.border_b(px(size::TAB_LINE)).border_color(if active {
            t.accent
        } else {
            gpui::transparent_black()
        }),
        Chrome::Pills => d
            .my(px(size::TAB_PILL_Y))
            .rounded(px(size::RADIUS))
            .when(active, |d| d.bg(t.accent_light())),
    }
}

/// What the pointer over a tab or an opener does: brightens its words,
/// and fills it where the chrome's are filled.
fn hovered(t: &Theme, fill: bool) -> impl FnOnce(gpui::StyleRefinement) -> gpui::StyleRefinement {
    let (text, raised) = (t.text, t.raised);
    move |d| {
        let d = d.text_color(text);
        if fill { d.bg(raised) } else { d }
    }
}

/// A word on a page's bar that opens a panel and shows it is open: a
/// smaller cousin of the tab.
pub fn opener(t: &Theme, id: impl Into<ElementId>, open: bool, cx: &App) -> Stateful<Div> {
    let chrome = chrome(cx);
    let d = div()
        .id(id)
        .flex_none()
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(6.))
        .cursor_pointer()
        .text_color(if open { t.text } else { t.text2 })
        .hover(hovered(t, chrome != Chrome::Line && !open));
    match chrome {
        Chrome::Line => d
            .h_full()
            .border_b(px(size::TAB_LINE))
            .border_color(if open {
                t.accent
            } else {
                gpui::transparent_black()
            }),
        Chrome::Tabs => d
            .h(px(size::CONTROL))
            .rounded(px(size::RADIUS))
            .when(open, |d| d.bg(t.raised)),
        Chrome::Pills => d
            .h(px(size::CONTROL))
            .rounded(px(size::RADIUS))
            .when(open, |d| d.bg(t.accent_light())),
    }
}

/// The bar tabs sit on: its rule along the bottom, drawn first so an
/// active card covers it.
pub fn tab_bar(t: &Theme, cx: &App) -> Div {
    div()
        .relative()
        .flex()
        .items_stretch()
        .bg(match chrome(cx) {
            Chrome::Tabs => t.bg,
            Chrome::Line | Chrome::Pills => t.surface,
        })
        // Cards stand on the rule; whatever else is on the bar stands with
        // them, on one line.
        .when(chrome(cx) == Chrome::Tabs, |d| d.pt(px(size::TAB_CARD_TOP)))
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(1.))
                .bg(t.divider),
        )
}

pub fn button(
    t: &Theme,
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    kind: Kind,
) -> Stateful<Div> {
    let b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(size::CONTROL))
        .px(px(10.))
        .rounded(px(size::RADIUS))
        .text_size(px(size::TEXT))
        .cursor_pointer()
        .child(text.into());
    let accent = t.accent;
    let text = t.text;
    // A hairline every kind has, so focus (Tab) can colour it without a jump.
    let b = b.border_1();
    match kind {
        Kind::Primary => b
            .bg(t.accent)
            .border_color(t.accent)
            .text_color(t.on_accent())
            .font_weight(FontWeight::SEMIBOLD)
            .hover(|s| s.opacity(crate::theme::fade::HOVER))
            .focus(move |s| s.border_color(text)),
        Kind::Plain => b
            .border_color(t.divider)
            .text_color(t.text)
            .hover(move |s| s.border_color(accent))
            .focus(move |s| s.border_color(accent)),
        Kind::Quiet => b
            .px(px(6.))
            .border_color(gpui::transparent_black())
            .text_color(t.text2)
            .hover(move |s| s.text_color(text))
            .focus(move |s| s.border_color(accent)),
    }
}

/// A key cap: filled for the modifier, outlined for the key.
pub fn cap(t: &Theme, text: &str, filled: bool) -> Div {
    let c = div()
        .px(px(6.))
        .h(px(22.))
        .flex()
        .items_center()
        .rounded(px(3.))
        .text_size(px(12.))
        .font_weight(FontWeight::BOLD)
        .child(text.to_string());
    if filled {
        c.bg(t.accent).text_color(t.on_accent())
    } else {
        c.border_1().border_color(t.text2).text_color(t.text)
    }
}

/// Caps joined with `+`, as the keys panel and the echo show them.
pub fn caps(t: &Theme, list: &[String]) -> Div {
    let mut row = div().flex().items_center().gap(px(6.));
    for (i, c) in list.iter().enumerate() {
        if i > 0 {
            row = row.child(div().text_color(t.dim).text_size(px(12.)).child("+"));
        }
        let modifier = matches!(
            c.as_str(),
            "ALT" | "SHIFT" | "CTRL" | "LEADER" | "CTRL+SHIFT"
        );
        row = row.child(cap(t, c, modifier));
    }
    row
}

/// A user's square: their initial on their identity colour.
pub fn user_square(t: &Theme, user: &str) -> Div {
    let initial = user
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    div()
        .flex_none()
        .size(px(14.))
        .rounded(px(2.))
        .bg(t.identity_of(user))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(9.))
        .font_weight(FontWeight::BOLD)
        .text_color(t.text)
        .child(initial)
}

/// Where something runs: the device's name in the device's colour, then its
/// environment, dim.
pub fn device_label(t: &Theme, device: &str, environment: &str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.identity_of(device))
                .child(device.to_string()),
        )
        .child(note(t, environment.to_string(), Some(t.dim)))
}

/// The mark of a session that waits for an answer: an accent dot.
pub fn waiting_dot(t: &Theme) -> Div {
    div().flex_none().size(px(7.)).rounded_full().bg(t.accent)
}

/// A popover's frame: raised surface, hairline, shadow.
pub fn panel(t: &Theme) -> Div {
    div()
        .bg(t.surface)
        .border_1()
        .border_color(t.divider)
        .rounded(px(6.))
        .shadow_lg()
        .text_color(t.text)
        .text_size(px(size::TEXT))
}

/// A thin rule.
pub fn rule(t: &Theme) -> Div {
    div().h(px(1.)).w_full().bg(t.divider)
}

/// A muted line of text.
pub fn note(t: &Theme, text: impl Into<SharedString>, c: Option<Hsla>) -> Div {
    div()
        .text_size(px(12.))
        .text_color(c.unwrap_or(t.text2))
        .child(text.into())
}

/// A small badge on a row or a tile: words in a hairline box.
pub fn badge(
    t: &Theme,
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    why: String,
) -> Stateful<Div> {
    div()
        .id(id)
        .tooltip(move |_, cx| cx.new(|_| Tip(why.clone().into())).into())
        .flex_none()
        .px(px(4.))
        .h(px(16.))
        .flex()
        .items_center()
        .rounded(px(3.))
        .border_1()
        .border_color(t.divider)
        .text_size(px(10.))
        .text_color(t.text2)
        .child(text.into())
}

/// A hover explanation, drawn like a menu: `.tooltip(kit::tip(why))`.
pub fn tip(
    why: impl Into<SharedString>,
) -> impl Fn(&mut gpui::Window, &mut App) -> gpui::AnyView + 'static {
    let why: SharedString = why.into();
    move |_, cx| cx.new(|_| Tip(why.clone())).into()
}

pub struct Tip(pub SharedString);

impl Render for Tip {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let t = crate::theme::theme(cx);
        panel(t)
            .max_w(px(320.))
            .px(px(8.))
            .py(px(4.))
            .text_size(px(12.))
            .text_color(t.text)
            .child(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Window};

    struct Labelled;

    impl Render for Labelled {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let t = Theme::violet();
            div()
                .child(device_label(&t, "landmass", "wsl").debug_selector(|| "device-label".into()))
        }
    }

    #[gpui::test]
    fn a_device_label_draws_the_device_and_its_environment(cx: &mut gpui::TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Labelled);
        let bounds = cx.debug_bounds("device-label").expect("the label is drawn");
        assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
    }
}

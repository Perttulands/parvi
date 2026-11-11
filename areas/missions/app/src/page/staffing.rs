//! The selected slot's role, harness, model and effort pickers.
use super::*;
use crate::store::staffing::{self, Choice};
use gpui::{SharedString, Stateful};
use parvi_app_kit::theme::Theme;

#[derive(Clone, Copy)]
enum Part {
    Role,
    Harness,
    Model,
    Effort,
}

/// A choice from the old staffing menu, with the current choice highlighted.
fn chip(t: &Theme, id: SharedString, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(TAB_GAP))
        .py(px(size::RADIUS))
        .rounded(px(size::RADIUS))
        .border_1()
        .border_color(if selected { t.accent } else { t.divider })
        .when(selected, |d| d.bg(t.raised))
        .hover(|d| d.border_color(t.text2))
        .cursor_pointer()
}
impl MissionsPage {
    /// Draw all choices for the selected slot, preserving unknown current
    /// values while the catalog is loading or after it changes.
    pub(super) fn slot_pickers(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let staff = [
            self.role.read(cx).text.clone(),
            self.harness.read(cx).text.clone(),
            self.model.read(cx).text.clone(),
            self.effort.read(cx).text.clone(),
        ];
        let choices = staffing::choices(self.roles.as_ref(), &staff);
        let row = |label: &'static str,
                   part: Part,
                   values: Vec<Choice>,
                   selected: &str,
                   cx: &mut Context<Self>| {
            let mut chips = div().flex().flex_wrap().gap(px(TAB_GAP));
            for choice in values {
                let on = choice.id == selected;
                let selector = format!(
                    "staffing-{}-{}{}",
                    label.to_lowercase(),
                    if choice.id.is_empty() {
                        "default"
                    } else {
                        &choice.id
                    },
                    if on { "-selected" } else { "" }
                );
                let id = choice.id;
                chips = chips.child(
                    chip(t, SharedString::from(selector.clone()), on)
                        .debug_selector(move || selector.clone())
                        .child(choice.label)
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.pick_staffing(part, &id, cx);
                        })),
                );
            }
            div()
                .flex()
                .flex_col()
                .gap(px(size::RADIUS))
                .child(kit::label(t, label))
                .child(chips)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(TAB_GAP))
            .child(row("Role", Part::Role, choices.roles, &staff[0], cx))
            .child(row(
                "Harness",
                Part::Harness,
                choices.harnesses,
                &staff[1],
                cx,
            ))
            .child(row("Model", Part::Model, choices.models, &staff[2], cx))
            .child(row("Effort", Part::Effort, choices.efforts, &staff[3], cx))
            .when(self.roles.is_none(), |d| {
                d.child(
                    div()
                        .text_color(t.dim)
                        .child("Reading what this node offers…"),
                )
            })
    }
    fn pick_staffing(&mut self, part: Part, id: &str, cx: &mut Context<Self>) {
        let mut staff = [
            self.role.read(cx).text.clone(),
            self.harness.read(cx).text.clone(),
            self.model.read(cx).text.clone(),
            self.effort.read(cx).text.clone(),
        ];
        match part {
            Part::Role => staff[0] = id.to_owned(),
            Part::Harness => {
                if let Some(catalog) = &self.roles {
                    staffing::pick_harness(catalog, &mut staff, id);
                } else {
                    staff[1] = id.to_owned();
                }
            }
            Part::Model => {
                if let Some(catalog) = &self.roles {
                    staffing::pick_model(catalog, &mut staff, id);
                } else {
                    staff[2] = id.to_owned();
                }
            }
            Part::Effort => staff[3] = id.to_owned(),
        }
        for (field, value) in [
            (&self.role, &staff[0]),
            (&self.harness, &staff[1]),
            (&self.model, &staff[2]),
            (&self.effort, &staff[3]),
        ] {
            if field.read(cx).text != *value {
                field.update(cx, |f, cx| f.set(value, false, cx));
            }
        }
        cx.notify();
    }
}

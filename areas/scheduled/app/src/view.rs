//! Drawing the page: the toolbar, the task list in each device's room, a
//! task's detail and the form.

use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, SharedString, Window, div, prelude::*, px,
};
use jiff::Timestamp;
use parvi_app_kit::hub::now_ms;
use parvi_app_kit::theme::{Theme, alpha, size, theme};
use parvi_app_kit::widgets::kit::{self, Kind};
use parvi_scheduled_contract::{Target, Task};

use crate::form::{self, MODES, Mode, ScheduleForm, TaskForm, WEEKDAYS};
use crate::page::ScheduledPage;
use crate::tokens::*;

impl Render for ScheduledPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A node that came into reach since the page was shown.
        self.read_unread(cx);
        let t = theme(cx).clone();
        let toolbar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.))
            .h(px(size::TAB_BAR))
            .px(px(GAP))
            .border_b_1()
            .border_color(t.divider)
            .child(div().font_weight(FontWeight::BOLD).child("SCHEDULED TASKS"))
            .child(kit::note(
                &t,
                "Prompts Parvi sends into your sessions on a timer",
                Some(t.dim),
            ))
            .child(div().flex_1())
            .child(
                kit::button(&t, "scheduled-refresh", "Refresh", Kind::Quiet)
                    .tooltip(kit::tip("Read every device's tasks again"))
                    .on_click(cx.listener(|page, _, _, cx| page.read_all(cx))),
            )
            .child(
                kit::button(&t, "scheduled-new", "New task", Kind::Primary)
                    .on_click(cx.listener(|page, _, _, cx| page.new_task(cx))),
            );
        let sc = &self.state;
        let main = div()
            .size_full()
            .flex()
            .flex_col()
            .child(toolbar)
            .when_some(sc.error.clone(), |d, e| d.child(banner(e, t.error)))
            .when_some(sc.notice.clone(), |d, n| d.child(banner(n, t.text2)));
        let detail: AnyElement = if sc.form.is_some() {
            self.form(&t, cx)
        } else if let Some((node, task)) = sc.selected_task() {
            let (node, task) = (node.to_string(), task.clone());
            self.detail(&t, &node, &task, cx)
        } else {
            div()
                .p(px(24.))
                .child(kit::note(
                    &t,
                    "Select a task, or create one to send a prompt into your sessions on a schedule.",
                    Some(t.dim),
                ))
                .into_any_element()
        };
        div().size_full().bg(t.bg).text_color(t.text).child(
            main.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(12.))
                    .p(px(12.))
                    .child(
                        panel(&t)
                            .id("scheduled-list")
                            .w(px(LIST_W))
                            .flex_none()
                            .overflow_y_scroll()
                            .child(self.list(&t, cx)),
                    )
                    .child(
                        panel(&t)
                            .id("scheduled-detail")
                            .flex_1()
                            .min_w_0()
                            .overflow_y_scroll()
                            .child(detail),
                    ),
            ),
        )
    }
}

impl ScheduledPage {
    fn list(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let now = Timestamp::now();
        let mut list = div().flex().flex_col().gap(px(GAP));
        let mut any = false;
        let mut reading = false;
        for node in self.host.nodes(cx) {
            let id = node.id.as_str();
            let Some(state) = self.state.nodes.get(id) else {
                continue;
            };
            reading |= state.reading && state.list.is_none();
            let tasks = state.list.as_deref().unwrap_or_default();
            if tasks.is_empty() && state.error.is_none() {
                continue;
            }
            any = true;
            let mut room = div()
                .flex()
                .flex_col()
                .border(px(WALL))
                .border_color(t.identity_of(&node.device))
                .rounded(px(size::RADIUS))
                .bg(t.surface)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(10.))
                        .h(px(size::TAB_BAR))
                        .border_b_1()
                        .border_color(t.divider)
                        .child(place(t, &node.device, &node.environment))
                        .when_some(state.error.clone(), |d, e| {
                            d.child(kit::note(t, e, Some(t.error)))
                        }),
                );
            for task in tasks {
                room = room.child(self.card(t, id, task, now, cx));
            }
            list = list.child(room);
        }
        if !any {
            let words = if reading {
                "Loading scheduled tasks…"
            } else {
                "No scheduled tasks yet. Create one to keep sessions moving while you are away."
            };
            list = list.child(div().p(px(PAD)).child(kit::note(t, words, Some(t.dim))));
        }
        list.into_any_element()
    }

    fn card(
        &self,
        t: &Theme,
        node: &str,
        task: &Task,
        now: Timestamp,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.state.selected.as_ref() == Some(&(node.to_string(), task.id.clone()));
        let muted = !task.live();
        let (pick_node, pick_id) = (node.to_string(), task.id.clone());
        let mut status = div().flex().flex_wrap().text_size(px(size::SMALL)).child(
            div()
                .text_color(if muted { t.dim } else { t.text2 })
                .child(form::status(task)),
        );
        if task.live() && task.next_run.is_some() {
            status = status.child(
                div()
                    .text_color(t.dim)
                    .child(format!(" · next {}", form::countdown(task.next_run, now))),
            );
        }
        if !task.last_status.is_empty() {
            status = status.child(
                div()
                    .text_color(if task.last_status == "success" {
                        t.dim
                    } else {
                        t.error
                    })
                    .child(format!(" · last {}", task.last_status)),
            );
        }
        let selector = format!("task-{node}-{}", task.id);
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(PAD))
            .py(px(10.))
            .border_b_1()
            .border_color(alpha(t.divider, RULE))
            .relative()
            .when(selected, |d| {
                d.bg(alpha(t.accent, SELECTED)).child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(t.accent),
                )
            })
            .when(muted, |d| d.opacity(MUTED))
            .hover(|d| d.bg(alpha(t.text, HOVER)))
            .cursor_pointer()
            .on_click(cx.listener(move |page, e: &ClickEvent, _, cx| {
                page.state.selected = Some((pick_node.clone(), pick_id.clone()));
                if e.click_count() >= 2
                    && let Some(task) = page.state.task(&pick_node, &pick_id).cloned()
                {
                    page.open(TaskForm::edit(&pick_node, &task), cx);
                }
                cx.notify();
            }))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(task.name.clone()),
            )
            .child(
                div()
                    .flex()
                    .text_size(px(size::SMALL))
                    .text_color(t.text2)
                    .child(form::describe(&task.schedule))
                    .child(
                        div()
                            .text_color(t.dim)
                            .child(format!(" · {}", zone_label(&task.schedule.timezone))),
                    ),
            )
            .child(chips(t, &task.targets))
            .child(status)
            .into_any_element()
    }

    fn detail(&self, t: &Theme, node: &str, task: &Task, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.state.busy;
        let armed = self.state.delete_armed(&task.id, now_ms());
        let at = |n: &str| n.to_string();
        let (edit_node, edit) = (at(node), task.clone());
        let (run_node, run) = (at(node), task.clone());
        let (pause_node, pause) = (at(node), task.clone());
        let (dup_node, dup) = (at(node), task.clone());
        let (del_node, del) = (at(node), task.id.clone());
        let actions = div()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .child(kit::button(t, "task-edit", "Edit", Kind::Quiet).on_click(
                cx.listener(move |page, _, _, cx| page.open(TaskForm::edit(&edit_node, &edit), cx)),
            ))
            .child(
                kit::button(t, "task-run", "Run now", Kind::Quiet)
                    .debug_selector(|| "task-run".into())
                    .when(busy, |b| b.opacity(BUSY))
                    .on_click(cx.listener(move |page, _, _, cx| {
                        if !page.state.busy {
                            page.act(&run_node, &run, "run-now", cx)
                        }
                    })),
            )
            .child(
                kit::button(
                    t,
                    "task-pause",
                    if task.paused { "Resume" } else { "Pause" },
                    Kind::Quiet,
                )
                .when(busy, |b| b.opacity(BUSY))
                .on_click(cx.listener(move |page, _, _, cx| {
                    if !page.state.busy {
                        let verb = if pause.paused { "resume" } else { "pause" };
                        page.act(&pause_node, &pause, verb, cx)
                    }
                })),
            )
            .child(
                kit::button(t, "task-duplicate", "Duplicate", Kind::Quiet).on_click(cx.listener(
                    move |page, _, _, cx| page.open(TaskForm::duplicate(&dup_node, &dup), cx),
                )),
            )
            .child(
                kit::button(
                    t,
                    "task-delete",
                    if armed { "Confirm delete" } else { "Delete" },
                    Kind::Quiet,
                )
                .text_color(t.error)
                .when(armed, |b| b.border_color(t.error).bg(alpha(t.error, 0.12)))
                .on_click(cx.listener(move |page, _, _, cx| page.delete(&del_node, &del, cx))),
            );
        let next = if task.live() {
            format!(
                "{} ({})",
                form::when(task.next_run),
                form::countdown(task.next_run, Timestamp::now())
            )
        } else {
            "Paused".into()
        };
        let last = match task.last_run {
            Some(_) => format!(
                "{} · {}",
                form::when(task.last_run),
                if task.last_status.is_empty() {
                    "unknown"
                } else {
                    &task.last_status
                }
            ),
            None => "Never run".into(),
        };
        let meta = div()
            .flex()
            .gap(px(GAP))
            .pt(px(PAD))
            .border_t_1()
            .border_color(t.divider)
            .child(meta_row(t, "Next run", div().child(next)))
            .child(meta_row(t, "Last run", div().child(last)))
            .child(meta_row(t, "Sends to", chips(t, &task.targets)))
            .child(meta_row(t, "Task ID", div().child(task.id.clone())));
        let held = (!task.held.is_empty()).then(|| {
            kit::note(
                t,
                format!("Held back by its node: {}", task.held),
                Some(t.error),
            )
        });
        let mut runs = div()
            .flex()
            .flex_col()
            .p(px(8.))
            .rounded(px(size::RADIUS))
            .bg(t.bg)
            .border_1()
            .border_color(t.divider);
        if task.recent_runs.is_empty() {
            runs = runs.child(kit::note(t, "No runs recorded yet.", Some(t.dim)));
        }
        for r in &task.recent_runs {
            let mut line = div()
                .flex()
                .flex_wrap()
                .py(px(4.))
                .border_b_1()
                .border_color(alpha(t.divider, RULE))
                .text_size(px(size::SMALL))
                .child(
                    div()
                        .text_color(if r.status == "success" {
                            t.text
                        } else {
                            t.error
                        })
                        .child(r.status.clone()),
                )
                .child(div().text_color(t.dim).child(format!(
                    " · {} · {}",
                    form::when(Some(r.started_at)),
                    if r.trigger.is_empty() {
                        "scheduled"
                    } else {
                        &r.trigger
                    }
                )));
            if !r.targets.is_empty() {
                let each: Vec<String> = r
                    .targets
                    .iter()
                    .map(|x| format!("{}: {}", x.session_name, x.status))
                    .collect();
                line = line.child(
                    div()
                        .text_color(t.dim)
                        .child(format!(" · {}", each.join(", "))),
                );
            }
            if !r.message.is_empty() {
                line = line.child(div().text_color(t.error).child(format!(" · {}", r.message)));
            }
            runs = runs.child(line);
        }
        card(t)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(PAD))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(div().font_weight(FontWeight::BOLD).child(task.name.clone()))
                            .child(kit::note(
                                t,
                                format!(
                                    "{} · {} · {} · on {}",
                                    form::describe(&task.schedule),
                                    zone_label(&task.schedule.timezone),
                                    form::status(task),
                                    self.place_name(node, cx)
                                ),
                                Some(t.dim),
                            )),
                    )
                    .child(actions),
            )
            .children(held)
            .child(meta)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(section(t, "Prompt"))
                    .child(
                        div()
                            .p(px(10.))
                            .rounded(px(size::RADIUS))
                            .bg(t.bg)
                            .border_1()
                            .border_color(t.divider)
                            .text_size(px(size::SMALL))
                            .whitespace_normal()
                            .child(task.prompt.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(section(t, "Recent runs"))
                    .child(runs),
            )
            .into_any_element()
    }

    fn form(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = self.state.form.as_ref() else {
            return div().into_any_element();
        };
        let busy = self.state.busy;
        let fields = &self.fields;
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::BOLD)
                    .child(if form.id.is_some() {
                        "Edit task"
                    } else {
                        "New task"
                    }),
            )
            .child(
                kit::button(t, "form-cancel", "Cancel", Kind::Quiet).on_click(cx.listener(
                    |page, _, _, cx| {
                        page.state.close(now_ms());
                        cx.notify();
                    },
                )),
            )
            .child(
                kit::button(
                    t,
                    "form-save",
                    if form.id.is_some() {
                        "Save task"
                    } else {
                        "Create task"
                    },
                    Kind::Primary,
                )
                .when(busy, |b| b.opacity(BUSY))
                .on_click(cx.listener(|page, _, _, cx| {
                    if !page.state.busy {
                        page.save(cx)
                    }
                })),
            );
        card(t)
            .child(head)
            .child(field(t, "Prompt", None, div().child(fields.prompt.clone())))
            .child(self.targets(t, form, cx))
            .child(self.when(t, &form.schedule, cx))
            .child(field(
                t,
                "Name",
                Some("(optional)"),
                div().child(fields.name.clone()),
            ))
            .into_any_element()
    }

    fn targets(&self, t: &Theme, form: &TaskForm, cx: &mut Context<Self>) -> AnyElement {
        let mut chosen = div().flex().flex_wrap().gap(px(6.));
        if form.targets.is_empty() {
            chosen = chosen.child(kit::note(t, "No sessions selected", Some(t.dim)));
        }
        for target in &form.targets {
            let gone = target.clone();
            chosen = chosen.child(
                chip(t, target).child(
                    div()
                        .id(SharedString::from(format!("unpick-{}", target_key(target))))
                        .px(px(2.))
                        .text_color(t.dim)
                        .hover(|d| d.text_color(t.text))
                        .cursor_pointer()
                        .child("×")
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.state.change(|f| f.targets.retain(|x| *x != gone));
                            cx.notify();
                        })),
                ),
            );
        }
        // Each place's sessions; an edited task's own place only.
        let mut picker = div().flex().flex_col().gap(px(8.));
        for node in self.host.nodes(cx) {
            let id = node.id.as_str();
            let Some(sessions) = self.sessions.get(id).filter(|s| !s.is_empty()) else {
                continue;
            };
            if form.id.is_some() && id != form.node {
                continue;
            }
            let mut row = div().flex().flex_wrap().gap(px(6.));
            for s in sessions {
                let target = s.target();
                let on = form.node == id && form.targets.contains(&target);
                let (session, at) = (s.clone(), id.to_string());
                row = row.child(
                    div()
                        .id(SharedString::from(format!(
                            "pick-{id}-{}",
                            target_key(&target)
                        )))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .h(px(size::CONTROL))
                        .px(px(8.))
                        .rounded(px(size::RADIUS))
                        .border_1()
                        .border_color(if on { t.accent } else { t.divider })
                        .when(on, |d| d.bg(alpha(t.accent, PICKED)))
                        .hover(|d| d.border_color(t.accent))
                        .cursor_pointer()
                        .text_size(px(size::SMALL))
                        .when_some(s.user.clone(), |d, u| d.child(kit::user_square(t, &u)))
                        .child(s.name.clone())
                        .on_click(cx.listener(move |page, _, _, cx| page.pick(&at, &session, cx))),
                );
            }
            picker = picker.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .child(place(t, &node.device, &node.environment))
                    .child(row),
            );
        }
        div()
            .id("scheduled-targets")
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .rounded(px(size::RADIUS))
            .border_1()
            .border_dashed()
            .border_color(t.divider)
            .child(section(t, "Send to"))
            .child(chosen)
            .child(picker)
            .into_any_element()
    }

    fn when(&self, t: &Theme, s: &ScheduleForm, cx: &mut Context<Self>) -> AnyElement {
        let fields = &self.fields;
        let preview = format!("{} · {}", form::describe(&s.schedule()), s.timezone);
        let mut modes = div().flex().gap(px(4.));
        for (mode, label) in MODES {
            modes = modes.child(
                tab(t, format!("mode-{label}"), label, s.mode == mode).on_click(cx.listener(
                    move |page, _, _, cx| {
                        page.state.change(|f| f.schedule.mode = mode);
                        cx.notify();
                    },
                )),
            );
        }
        let mut row = div().flex().flex_wrap().items_end().gap(px(PAD));
        match s.mode {
            Mode::Every => {
                row = row
                    .child(compact(
                        t,
                        "Run every",
                        div().w(px(SHORT_FIELD)).child(fields.every.clone()),
                    ))
                    .child(compact(
                        t,
                        "Unit",
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(unit(t, "minutes", false, s.every_hours, cx))
                            .child(unit(t, "hours", true, s.every_hours, cx)),
                    ));
            }
            Mode::Daily | Mode::Weekly => {
                row = row.child(compact(
                    t,
                    "At",
                    div().w(px(SHORT_FIELD)).child(fields.time.clone()),
                ));
            }
            Mode::Cron => {
                row = row.child(compact(
                    t,
                    "Cron expression",
                    div().w(px(CRON_FIELD)).child(fields.cron.clone()),
                ));
            }
        }
        let local = form::local_zone();
        let mut zones: Vec<String> = vec![local.clone()];
        for z in [s.timezone.clone(), "UTC".into()] {
            if !zones.contains(&z) {
                zones.push(z);
            }
        }
        let mut zone_row = div().flex().flex_wrap().gap(px(4.));
        for z in zones {
            let label = if z == local {
                format!("{z} (yours)")
            } else {
                z.clone()
            };
            let on = s.timezone == z;
            zone_row = zone_row.child(toggle(t, format!("zone-{z}"), &label, on).on_click(
                cx.listener(move |page, _, _, cx| {
                    page.state.change(|f| f.schedule.timezone = z.clone());
                    cx.notify();
                }),
            ));
        }
        row = row.child(compact(t, "Timezone", zone_row));
        let mut out = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(8.))
                    .child(section(t, "When"))
                    .child(kit::note(t, preview, Some(t.dim))),
            )
            .child(modes)
            .child(row);
        if s.mode == Mode::Weekly {
            let mut days = div().flex().gap(px(4.));
            for (value, short, _) in WEEKDAYS {
                let on = s.weekdays.contains(&value);
                days = days.child(
                    toggle(t, format!("day-{short}"), short, on)
                        .min_w(px(DAY_W))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.state.change(|f| {
                                let w = &mut f.schedule.weekdays;
                                match w.iter().position(|d| *d == value) {
                                    Some(i) => {
                                        w.remove(i);
                                    }
                                    None => w.push(value),
                                }
                            });
                            cx.notify();
                        })),
                );
            }
            out = out.child(days);
        }
        if let Some(why) = s.error() {
            out = out.child(kit::note(t, why, Some(t.error)));
        }
        out.into_any_element()
    }
}

/// A device and its environment, in the device's colour.
fn place(t: &Theme, device: &str, environment: &str) -> Div {
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
        .child(kit::note(t, environment.to_string(), Some(t.dim)))
}

fn target_key(t: &Target) -> String {
    format!("{}:{}", t.unix_user, t.session_name)
}

fn zone_label(zone: &str) -> &str {
    if zone.is_empty() { "Local" } else { zone }
}

fn banner(text: String, c: gpui::Hsla) -> Div {
    div()
        .mx(px(PAD))
        .mt(px(PAD))
        .px(px(PAD))
        .py(px(8.))
        .rounded(px(size::RADIUS))
        .border_1()
        .border_color(alpha(c, BANNER_EDGE))
        .bg(alpha(c, BANNER))
        .text_color(c)
        .child(text)
}

/// A list or detail panel.
fn panel(t: &Theme) -> Div {
    div()
        .h_full()
        .p(px(8.))
        .rounded(px(size::RADIUS))
        .border_1()
        .border_color(t.divider)
        .bg(t.surface)
}

/// The detail or the form inside its panel.
fn card(_t: &Theme) -> Div {
    div().flex().flex_col().gap(px(GAP)).p(px(4.))
}

/// A section's label, CHROTE's: small, upper case, accent.
fn section(t: &Theme, text: &str) -> Div {
    div()
        .text_size(px(size::SMALL))
        .text_color(t.accent)
        .child(text.to_uppercase())
}

fn chip(t: &Theme, target: &Target) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .px(px(6.))
        .py(px(1.))
        .rounded(px(size::RADIUS))
        .bg(alpha(t.text, CHIP))
        .border_1()
        .border_color(t.divider)
        .text_size(px(size::SMALL))
        .when(!target.unix_user.is_empty(), |d| {
            d.child(kit::user_square(t, &target.unix_user))
        })
        .child(target.session_name.clone())
}

fn chips(t: &Theme, targets: &[Target]) -> Div {
    let mut row = div().flex().flex_wrap().gap(px(4.));
    for target in targets {
        row = row.child(chip(t, target));
    }
    row
}

/// One of the detail's columns: Next run, Last run, Sends to, Task ID.
fn meta_row(t: &Theme, label: &str, value: Div) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(kit::label(t, label.to_string()))
        .child(value)
}

fn field(t: &Theme, label: &str, aside: Option<&str>, input: Div) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(6.))
                .child(kit::label(t, label.to_string()))
                .when_some(aside, |d, a| {
                    d.child(kit::note(t, a.to_string(), Some(t.dim)))
                }),
        )
        .child(input)
}

fn compact(t: &Theme, label: &str, input: Div) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(kit::label(t, label.to_string()))
        .child(input)
}

/// The schedule's mode switch: CHROTE's tabs, underlined when chosen.
fn tab(t: &Theme, id: String, label: &str, on: bool) -> gpui::Stateful<Div> {
    let text = t.text;
    div()
        .id(SharedString::from(id))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.))
        .px(px(10.))
        .pt(px(4.))
        .cursor_pointer()
        .text_color(if on { t.text } else { t.text2 })
        .when(on, |d| d.font_weight(FontWeight::BOLD))
        .hover(move |d| d.text_color(text))
        .child(label.to_string())
        .child(div().h(px(2.)).w_full().when(on, |d| d.bg(t.accent)))
}

/// A pressed-or-not button: a weekday, a zone, a unit.
fn toggle(t: &Theme, id: String, label: &str, on: bool) -> gpui::Stateful<Div> {
    kit::button(t, SharedString::from(id), label.to_string(), Kind::Plain)
        .when(on, |b| {
            b.bg(alpha(t.accent, TOGGLED))
                .border_color(t.accent)
                .text_color(t.text)
        })
        .when(!on, |b| b.text_color(t.text2))
}

fn unit(
    t: &Theme,
    label: &'static str,
    hours: bool,
    now_hours: bool,
    cx: &mut Context<ScheduledPage>,
) -> gpui::Stateful<Div> {
    toggle(t, format!("unit-{label}"), label, hours == now_hours).on_click(cx.listener(
        move |page, _, _, cx| {
            page.state.change(|f| f.schedule.every_hours = hours);
            cx.notify();
        },
    ))
}

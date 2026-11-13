//! Drawing the page on the kit's skeleton: the tasks in the list, soonest
//! first, and beside them the chosen task (when it runs, where it sends,
//! how its last run went) or the form.

use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, SharedString, Window, div, prelude::*, px,
};
use jiff::Timestamp;
use parvi_app_kit::Ask;
use parvi_app_kit::hub::now_ms;
use parvi_app_kit::menu::MenuRow;
use parvi_app_kit::theme::{Theme, size, theme};
use parvi_app_kit::widgets::kit::{self, Kind};
use parvi_app_kit::widgets::pane::{self, Col};
use parvi_core::NodeId;
use parvi_scheduled_contract::{SUCCESS, Target, Task};

use crate::form::{self, MODES, Mode, ScheduleForm, TaskForm, WEEKDAYS};
use crate::page::ScheduledPage;
use crate::tokens::*;

/// Recent runs: when, what started it, how it went.
const RUNS: [Col; 3] = [
    Col::fixed("When", RUN_WHEN_W),
    Col::fixed("By", RUN_BY_W),
    Col::fill("Result"),
];

impl Render for ScheduledPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let shown = self
            .shown_task()
            .map(|(n, task)| (n.to_string(), task.clone()));
        let list_head = pane::head(&t).child(div().flex_1()).child(
            pane::act(&t, "scheduled-new", "New")
                .debug_selector(|| "scheduled-new".into())
                .on_click(cx.listener(|page, _, _, cx| page.new_task(cx))),
        );
        let chosen = shown
            .as_ref()
            .map(|(n, task)| (n.as_str(), task.id.as_str()));
        let list = pane::list(&t, "scheduled-list", list_head, self.list(&t, chosen, cx));
        let detail = if self.state.form.is_some() {
            self.form(&t, cx)
        } else if let Some((node, task)) = &shown {
            self.detail(&t, node, task, cx)
        } else {
            let words = if self.reading() {
                "Reading scheduled tasks…"
            } else {
                "Nothing scheduled."
            };
            let action = (!self.reading()).then(|| {
                kit::button(&t, "scheduled-empty-new", "New task", Kind::Primary)
                    .on_click(cx.listener(|page, _, _, cx| page.new_task(cx)))
                    .into_any_element()
            });
            pane::detail(
                &t,
                "scheduled-detail",
                pane::head(&t),
                div()
                    .children(self.notices(&t))
                    .child(pane::empty(&t, words, action).p_0()),
            )
        };
        pane::page(&t, list, detail).text_color(t.text)
    }
}

impl ScheduledPage {
    /// Some node's first read is still out and nothing is listed yet.
    fn reading(&self) -> bool {
        self.state
            .nodes
            .values()
            .any(|n| n.reading && n.list.is_none())
    }

    /// Every node's tasks in the list's order: the live ones soonest
    /// first, then the paused and the disabled by name.
    fn ordered(&self) -> Vec<(&str, &Task)> {
        let mut all: Vec<(&str, &Task)> = self
            .state
            .nodes
            .iter()
            .flat_map(|(n, s)| s.list.iter().flatten().map(move |task| (n.as_str(), task)))
            .collect();
        all.sort_by(|(_, a), (_, b)| {
            (!a.live(), a.live().then_some(a.next_run).flatten(), &a.name).cmp(&(
                !b.live(),
                b.live().then_some(b.next_run).flatten(),
                &b.name,
            ))
        });
        all
    }

    /// The task the detail shows: the one chosen, or else the first.
    pub(crate) fn shown_task(&self) -> Option<(&str, &Task)> {
        self.state
            .selected_task()
            .or_else(|| self.ordered().into_iter().next())
    }

    fn list(&self, t: &Theme, chosen: Option<(&str, &str)>, cx: &mut Context<Self>) -> Div {
        let now = Timestamp::now();
        let ordered = self.ordered();
        let nodes = self.host.nodes(cx);
        // A device's label only where there are tasks on more than one.
        let several = ordered
            .iter()
            .any(|(n, _)| Some(*n) != ordered.first().map(|f| f.0));
        let mut out = div().flex().flex_col();
        for node in &nodes {
            let id = node.id.as_str();
            let Some(state) = self.state.nodes.get(id) else {
                continue;
            };
            if let Some(e) = &state.error {
                out = out.child(
                    div()
                        .px(px(size::ROW_PAD))
                        .py(px(size::LABEL_BOTTOM))
                        .flex()
                        .flex_col()
                        .child(kit::device_label(t, &node.device, &node.environment))
                        .child(pane::meta(t, format!("Not read: {e}"), Some(t.error))),
                );
            }
        }
        if several {
            for node in &nodes {
                let id = node.id.as_str();
                let rows: Vec<_> = ordered.iter().filter(|(n, _)| *n == id).collect();
                if rows.is_empty() {
                    continue;
                }
                out = out.child(
                    div()
                        .px(px(size::ROW_PAD))
                        .pt(px(size::LABEL_TOP))
                        .pb(px(size::LABEL_BOTTOM))
                        .child(kit::device_label(t, &node.device, &node.environment)),
                );
                for (n, task) in rows {
                    out = out.child(self.row(t, n, task, chosen, now, cx));
                }
            }
        } else {
            for (n, task) in &ordered {
                out = out.child(self.row(t, n, task, chosen, now, cx));
            }
        }
        out
    }

    fn row(
        &self,
        t: &Theme,
        node: &str,
        task: &Task,
        chosen: Option<(&str, &str)>,
        now: Timestamp,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = self.state.form.is_none() && chosen == Some((node, task.id.as_str()));
        let (meta, colour) = if !task.held.is_empty() {
            ("held".to_string(), Some(t.error))
        } else if !task.last_status.is_empty() && task.last_status != SUCCESS {
            ("failed".to_string(), Some(t.error))
        } else if task.paused {
            ("paused".to_string(), None)
        } else if !task.enabled {
            ("off".to_string(), None)
        } else {
            (form::countdown(task.next_run, now), None)
        };
        let (pick_node, pick_id) = (node.to_string(), task.id.clone());
        let selector = format!("task-{node}-{}", task.id);
        let name = task.name.clone();
        pane::row(t, SharedString::from(selector.clone()), on)
            .debug_selector(move || selector)
            .child(
                pane::cell(task.name.clone())
                    .debug_selector(move || format!("scheduled-task-name-{name}")),
            )
            .child(pane::meta(t, meta, colour))
            .on_click(cx.listener(move |page, e: &ClickEvent, _, cx| {
                let key = (pick_node.clone(), pick_id.clone());
                if page.state.form.is_some() {
                    if !page.state.may_leave(now_ms()) {
                        cx.notify();
                        return;
                    }
                    page.state.form = None;
                    page.state.dirty = false;
                }
                if page.state.selected.as_ref() != Some(&key) {
                    page.state.notice = None;
                    page.state.error = None;
                }
                page.state.selected = Some(key);
                if e.click_count() >= 2
                    && let Some(task) = page.state.task(&pick_node, &pick_id).cloned()
                {
                    page.open(TaskForm::edit(&pick_node, &task), cx);
                }
                cx.notify();
            }))
            .into_any_element()
    }

    /// What the last action came to, or why it failed: one line on top of
    /// the detail until something else is done.
    fn notices(&self, t: &Theme) -> Vec<Div> {
        let line = |text: &String, colour| {
            let selector = format!("scheduled-banner-{text}");
            div()
                .debug_selector(move || selector)
                .text_color(colour)
                .child(text.clone())
        };
        let mut out = vec![];
        if let Some(e) = &self.state.error {
            out.push(line(e, t.error));
        }
        if let Some(n) = &self.state.notice {
            out.push(line(n, t.text2));
        }
        out
    }

    fn detail(&self, t: &Theme, node: &str, task: &Task, cx: &mut Context<Self>) -> Div {
        let busy = self.state.busy;
        let at = |n: &str| n.to_string();
        let (edit_node, edit) = (at(node), task.clone());
        let (run_node, run) = (at(node), task.clone());
        let (pause_node, pause) = (at(node), task.clone());
        let paused = task.paused;
        let (more_node, more) = (at(node), task.clone());
        let head = pane::head(t)
            .child(pane::title(t, task.name.clone()))
            .child(
                pane::act(t, "task-edit", "Edit")
                    .debug_selector(|| "task-edit".into())
                    .on_click(cx.listener(move |page, _, _, cx| {
                        page.open(TaskForm::edit(&edit_node, &edit), cx)
                    })),
            )
            .child(
                pane::act(t, "task-run", "Run now")
                    .debug_selector(|| "task-run".into())
                    .tooltip(kit::tip("Send the prompt now"))
                    .when(busy, |b| b.opacity(BUSY))
                    .on_click(cx.listener(move |page, _, _, cx| {
                        if !page.state.busy {
                            page.act(&run_node, &run, "run-now", cx)
                        }
                    })),
            )
            .child(
                pane::act(t, "task-pause", if paused { "Resume" } else { "Pause" })
                    .debug_selector(move || format!("task-pause-{paused}"))
                    .when(busy, |b| b.opacity(BUSY))
                    .on_click(cx.listener(move |page, _, _, cx| {
                        if !page.state.busy {
                            let verb = if pause.paused { "resume" } else { "pause" };
                            page.act(&pause_node, &pause, verb, cx)
                        }
                    })),
            )
            .child(
                pane::act(t, "task-more", "…")
                    .debug_selector(|| "task-more".into())
                    .on_click(cx.listener(move |page, e: &ClickEvent, _, cx| {
                        let rows = page.task_menu(&more_node, &more, cx);
                        page.host.menu(rows, e.position(), cx);
                    })),
            );

        let now = Timestamp::now();
        let mut when = form::describe(&task.schedule);
        let zone = &task.schedule.timezone;
        // "Local" is the node's own zone, as CHROTE writes it.
        if !zone.is_empty() && zone != "Local" && *zone != form::local_zone() {
            when = format!("{when} · {zone}");
        }
        let when = if task.paused {
            format!("{when} · paused")
        } else if !task.enabled {
            format!("{when} · off")
        } else if task.next_run.is_some() {
            format!(
                "{when} · next {} ({})",
                form::when(task.next_run),
                form::countdown(task.next_run, now)
            )
        } else {
            when
        };

        let failed = !task.last_status.is_empty() && task.last_status != SUCCESS;
        let last = match task.last_run {
            None => div().text_color(t.dim).child("Never run"),
            Some(_) => {
                let why = task
                    .recent_runs
                    .first()
                    .filter(|r| r.status != SUCCESS && !r.message.is_empty())
                    .map(|r| format!(": {}", r.message))
                    .unwrap_or_default();
                let status = if task.last_status.is_empty() {
                    "unknown".to_string()
                } else {
                    task.last_status.clone()
                };
                div()
                    .when(failed, |d| d.text_color(t.error))
                    .child(format!("{} · {status}{why}", form::when(task.last_run)))
            }
        };

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(size::SECTION_GAP))
            .children(self.notices(t));
        let mut lines = div().flex().flex_col();
        if !task.held.is_empty() {
            lines = lines.child(pane::line(
                t,
                "Held back",
                div().text_color(t.error).child(task.held.clone()),
            ));
        }
        lines = lines
            .child(pane::line(t, "When", div().child(when)))
            .child(pane::line(
                t,
                "Sends to",
                self.sends_to(t, node, &task.targets),
            ))
            .child(pane::line(t, "Last run", last));
        body = body.child(lines).child(
            pane::section(t, "Prompt").child(
                div()
                    .whitespace_normal()
                    .text_color(t.text)
                    .child(task.prompt.clone()),
            ),
        );
        if task.recent_runs.len() > 1 {
            let rows = task
                .recent_runs
                .iter()
                .map(|r| {
                    let ok = r.status == SUCCESS;
                    let mut result = r.status.clone();
                    if !r.message.is_empty() {
                        result = format!("{result}: {}", r.message);
                    }
                    let missed: Vec<String> = r
                        .targets
                        .iter()
                        .filter(|x| x.status != SUCCESS)
                        .map(|x| format!("{}: {}", x.session_name, x.status))
                        .collect();
                    if !missed.is_empty() {
                        result = format!("{result} ({})", missed.join(", "));
                    }
                    let by = if r.trigger == "manual" {
                        "Run now"
                    } else {
                        "schedule"
                    };
                    pane::table_row(
                        t,
                        &RUNS,
                        vec![
                            pane::cell(form::when(Some(r.started_at))).into_any_element(),
                            pane::cell(by).text_color(t.text2).into_any_element(),
                            pane::cell(result)
                                .text_color(if ok { t.text2 } else { t.error })
                                .into_any_element(),
                        ],
                    )
                    .into_any_element()
                })
                .collect();
            body = body.child(pane::section(t, "Recent runs").child(pane::table(t, &RUNS, rows)));
        }
        pane::detail(t, "scheduled-detail", head, body)
    }

    /// What the head's "…" offers: the actions a task needs now and then.
    fn task_menu(&self, node: &str, task: &Task, cx: &mut Context<Self>) -> Vec<MenuRow> {
        let page = cx.entity().downgrade();
        let (dup_node, dup) = (node.to_string(), task.clone());
        let duplicate = MenuRow::new("Duplicate", {
            let page = page.clone();
            move |_, cx| {
                let _ = page.update(cx, |p, cx| p.open(TaskForm::duplicate(&dup_node, &dup), cx));
            }
        });
        let (del_node, del) = (node.to_string(), task.id.clone());
        let delete = MenuRow::new("Delete", move |_, cx| {
            let _ = page.update(cx, |p, cx| p.delete(&del_node, &del, cx));
        })
        .danger();
        vec![duplicate, delete]
    }

    /// The task's sessions, each a link that shows it in Terminal.
    fn sends_to(&self, t: &Theme, node: &str, targets: &[Target]) -> Div {
        let mut out = div().flex().flex_wrap().gap(px(size::HEAD_GAP));
        for target in targets {
            let ask = Ask::Reveal {
                what: "session".into(),
                id: target.session_name.clone(),
                node: Some(NodeId(node.to_string())),
                part: None,
                beside: false,
            };
            let host = self.host.clone();
            out = out.child(
                div()
                    .id(SharedString::from(format!(
                        "sends-to-{}",
                        target_key(target)
                    )))
                    .cursor_pointer()
                    .text_color(t.text)
                    .hover(|d| d.underline())
                    .tooltip(kit::tip("Show it in Terminal"))
                    .child(target_name(target))
                    .on_click(move |_, _, cx| {
                        let _ = host.ask(ask.clone(), cx);
                    }),
            );
        }
        out
    }

    fn form(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let form = self.state.form.as_ref().expect("the form is open");
        let busy = self.state.busy;
        let fields = &self.fields;
        let head = pane::head(t)
            .child(pane::title(
                t,
                if form.id.is_some() {
                    "Edit task"
                } else {
                    "New task"
                },
            ))
            .child(
                pane::act(t, "form-cancel", "Cancel").on_click(cx.listener(|page, _, _, cx| {
                    page.state.close(now_ms());
                    cx.notify();
                })),
            )
            .child(
                kit::button(
                    t,
                    "form-save",
                    if form.id.is_some() { "Save" } else { "Create" },
                    Kind::Primary,
                )
                .debug_selector(|| "form-save".into())
                .when(busy, |b| b.opacity(BUSY))
                .on_click(cx.listener(|page, _, _, cx| {
                    if !page.state.busy {
                        page.save(cx)
                    }
                })),
            );
        let body = div()
            .flex()
            .flex_col()
            .gap(px(size::SECTION_GAP))
            .children(self.notices(t))
            .child(
                pane::section(t, "Prompt").child(
                    div()
                        .debug_selector(|| "scheduled-prompt".into())
                        .child(fields.prompt.clone()),
                ),
            )
            .child(pane::section(t, "Send to").child(self.targets(t, form, cx)))
            .child(pane::section(t, "When").child(self.when(t, &form.schedule, cx)))
            .child(pane::section(t, "Name").child(fields.name.clone()));
        pane::detail(t, "scheduled-form", head, body)
    }

    /// The sessions a task may send to, each pressed or not; a task's
    /// sessions are all on one node, so an edited task offers its own.
    fn targets(&self, t: &Theme, form: &TaskForm, cx: &mut Context<Self>) -> Div {
        let nodes: Vec<_> = self
            .host
            .nodes(cx)
            .into_iter()
            .filter(|n| form.id.is_none() || n.id.as_str() == form.node)
            .map(|n| {
                let sessions = self.sessions_on(n.id.as_str(), cx);
                (n, sessions)
            })
            .filter(|(n, s)| !s.is_empty() || n.id.as_str() == form.node)
            .collect();
        let several = nodes.len() > 1;
        let mut out = div().flex().flex_col().gap(px(size::HEAD_GAP));
        for (node, sessions) in nodes {
            let id = node.id.as_str();
            let mut row = div().flex().flex_wrap().gap(px(TOGGLE_GAP));
            let mut offered: Vec<Target> = sessions.iter().map(|s| s.target()).collect();
            // A chosen session that is not running now stays on show.
            if form.node == id {
                for chosen in &form.targets {
                    if !offered.contains(chosen) {
                        offered.push(chosen.clone());
                    }
                }
            }
            if offered.is_empty() {
                row = row.child(div().text_color(t.dim).child("No session is running."));
            }
            for target in offered {
                let running = sessions.iter().any(|s| s.target() == target);
                let on = form.node == id && form.targets.contains(&target);
                let selector = format!("pick-{id}-{}", target_key(&target));
                let chosen = format!("chosen-{}", target_key(&target));
                let (at, session) = (id.to_string(), target.clone());
                row = row.child(
                    toggle(t, selector.clone(), "", on)
                        .debug_selector(move || selector)
                        .when(!running, |b| b.tooltip(kit::tip("Not running now")))
                        .child(
                            div()
                                .when(on, |d| d.debug_selector(move || chosen))
                                .when(!running, |d| d.text_color(t.dim))
                                .child(target_name(&target)),
                        )
                        .on_click(cx.listener(move |page, _, _, cx| page.pick(&at, &session, cx))),
                );
            }
            out = out.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(TOGGLE_GAP))
                    .when(several, |d| {
                        d.child(kit::device_label(t, &node.device, &node.environment))
                    })
                    .child(row),
            );
        }
        out
    }

    fn when(&self, t: &Theme, s: &ScheduleForm, cx: &mut Context<Self>) -> Div {
        let fields = &self.fields;
        let mut modes = div().flex().flex_wrap().gap(px(TOGGLE_GAP));
        for (mode, label) in MODES {
            let selector = format!("mode-{label}");
            modes = modes.child(
                toggle(t, selector.clone(), label, s.mode == mode)
                    .debug_selector(move || selector)
                    .on_click(cx.listener(move |page, _, _, cx| {
                        page.state.change(|f| f.schedule.mode = mode);
                        cx.notify();
                    })),
            );
        }
        let mut out = div().flex().flex_col().gap(px(size::HEAD_GAP)).child(modes);
        let measure = match s.mode {
            Mode::Every => div()
                .flex()
                .items_center()
                .gap(px(TOGGLE_GAP))
                .child("Every")
                .child(div().w(px(SHORT_FIELD)).child(fields.every.clone()))
                .child(unit(t, "minutes", false, s.every_hours, cx))
                .child(unit(t, "hours", true, s.every_hours, cx)),
            Mode::Daily | Mode::Weekly => div()
                .flex()
                .items_center()
                .gap(px(TOGGLE_GAP))
                .child("At")
                .child(div().w(px(SHORT_FIELD)).child(fields.time.clone())),
            Mode::Cron => div().child(
                div()
                    .debug_selector(|| "scheduled-cron".into())
                    .w(px(CRON_FIELD))
                    .child(fields.cron.clone()),
            ),
        };
        out = out.child(measure);
        if s.mode == Mode::Weekly {
            let mut days = div().flex().flex_wrap().gap(px(TOGGLE_GAP));
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
        // The zone is said only to be changed: this computer's, the task's,
        // or UTC.
        let local = form::local_zone();
        let mut zones: Vec<String> = vec![local.clone()];
        for z in [s.timezone.clone(), "UTC".into()] {
            if !z.is_empty() && !zones.contains(&z) {
                zones.push(z);
            }
        }
        let mut zone_row = div().flex().flex_wrap().gap(px(TOGGLE_GAP));
        for z in zones {
            let on = s.timezone == z;
            zone_row = zone_row.child(toggle(t, format!("zone-{z}"), &z, on).on_click(
                cx.listener(move |page, _, _, cx| {
                    page.state.change(|f| f.schedule.timezone = z.clone());
                    cx.notify();
                }),
            ));
        }
        out = out.child(zone_row);
        match s.error() {
            Some(why) => out.child(div().text_color(t.error).child(why)),
            None => out.child(div().text_color(t.dim).child(form::describe(&s.schedule()))),
        }
    }
}

fn target_key(t: &Target) -> String {
    format!("{}:{}", t.unix_user, t.session_name)
}

/// A session's name, with its user's when it is not the node's own.
fn target_name(t: &Target) -> String {
    if t.unix_user.is_empty() {
        t.session_name.clone()
    } else {
        format!("{}:{}", t.unix_user, t.session_name)
    }
}

/// A choice pressed or not: a mode, a session, a weekday, a zone, a unit.
/// Pressed reads on a quiet fill in weight, as a chosen row does.
fn toggle(t: &Theme, id: String, label: &str, on: bool) -> gpui::Stateful<Div> {
    kit::button(t, SharedString::from(id), label.to_string(), Kind::Plain)
        .when(on, |b| {
            b.bg(t.raised)
                .border_color(t.text2)
                .text_color(t.text)
                .font_weight(FontWeight::SEMIBOLD)
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

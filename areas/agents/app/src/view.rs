//! Agent setup on the kit's skeleton. The first screen says whether the
//! agents can run with their skills current: one line when they can, and
//! otherwise only what is off, each with its fix. The list reaches every
//! harness, tool and skills checkout for a closer look or a sync.

use crate::state::{self as agents, Checkout, Chosen, Fix, Off, Report, Tone, Tool};
use crate::{AgentsPage, tokens};
use gpui::{AnyElement, Context, Div, Hsla, SharedString, Window, div, prelude::*, px};
use parvi_app_kit::theme::{Theme, rgb, size, theme};
use parvi_app_kit::widgets::kit::{self, Kind};
use parvi_app_kit::widgets::pane;

impl AgentsPage {
    pub(crate) fn render_agents(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.library.open {
            return self.render_role_library(window, cx);
        }
        let t = theme(cx).clone();
        let reports = self.agents_reports();
        let several = reports.len() > 1;
        // Every tool and checkout any place reports, once, by id and name.
        let mut tools: Vec<&Tool> = vec![];
        let mut checkouts: Vec<&Checkout> = vec![];
        for (_, r) in &reports {
            for tl in &r.tools {
                if tools.iter().all(|x| x.id != tl.id) {
                    tools.push(tl);
                }
            }
            for c in &r.checkouts {
                if checkouts.iter().all(|x| x.name != c.name) {
                    checkouts.push(c);
                }
            }
        }
        let off: Vec<(&str, Off)> = reports
            .iter()
            .flat_map(|(n, r)| agents::off(r).into_iter().map(move |o| (*n, o)))
            .collect();

        let list_head = pane::list_head(&t, None)
            .child(
                pane::act(&t, "agents-refresh", "Check again")
                    .debug_selector(|| "agents-refresh".into())
                    .tooltip(kit::tip("Read the tools and checkouts again"))
                    .on_click(cx.listener(|s, _, _, cx| s.agents_refresh(cx))),
            )
            .child(
                pane::act(&t, "agents-library", "Roles")
                    .debug_selector(|| "agents-library".into())
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.library.open = true;
                        s.load_role_library(cx);
                    })),
            );
        let mut rows = div().flex().flex_col();
        if !reports.is_empty() {
            let (meta, colour) = if off.is_empty() {
                ("ready".to_string(), None)
            } else {
                (
                    format!("{} to fix", off.len()),
                    Some(tone_colour(&t, worst_tone(off.iter().map(|(_, o)| o.tone)))),
                )
            };
            rows = rows.child(self.row(
                &t,
                "agents-overview",
                "Overview",
                None,
                Some((meta, colour)),
                Chosen::Overview,
                cx,
            ));
        }
        for (label, harness) in [("Harnesses", true), ("Tools", false)] {
            let group: Vec<&&Tool> = tools
                .iter()
                .filter(|x| x.skills.is_some() == harness)
                .collect();
            if group.is_empty() {
                continue;
            }
            rows = rows.child(pane::label(&t, label));
            for tool in group {
                let states: Vec<(String, Tone)> = reports
                    .iter()
                    .filter_map(|(_, r)| {
                        let mine = r.tools.iter().find(|x| x.id == tool.id)?;
                        Some(agents::tool_state(r, mine))
                    })
                    .collect();
                let (word, _) = worst(states.into_iter());
                let signed_out = reports.iter().any(|(_, r)| {
                    r.tools
                        .iter()
                        .any(|x| x.id == tool.id && x.path.is_some() && x.signed_in == Some(false))
                });
                let meta = if signed_out {
                    Some((
                        "not signed in".to_string(),
                        Some(tone_colour(&t, Tone::Behind)),
                    ))
                } else if word != "available" {
                    Some((word.clone(), None))
                } else {
                    None
                };
                rows = rows.child(self.row(
                    &t,
                    &format!("agents-tool-{}", tool.id),
                    &tool.name,
                    Some(format!("agents-tool-state-{}-{word}", tool.id)),
                    meta,
                    Chosen::Tool(tool.id.clone()),
                    cx,
                ));
            }
        }
        if !checkouts.is_empty() {
            rows = rows.child(pane::label(&t, "Skills from"));
            for c in &checkouts {
                let word = worst_word(reports.iter().filter_map(|(_, r)| {
                    let mine = r.checkouts.iter().find(|x| x.name == c.name)?;
                    agents::checkout_word(r, mine)
                }));
                let meta = word.map(|(w, tone)| (w, Some(tone_colour(&t, tone))));
                rows = rows.child(self.row(
                    &t,
                    &format!("agents-checkout-{}", c.name),
                    &c.name,
                    None,
                    meta,
                    Chosen::Checkout(c.name.clone()),
                    cx,
                ));
            }
        }
        let list = pane::list(&t, "agents-list", list_head, rows);

        let detail = if reports.is_empty() {
            let places: Vec<String> = self.places.keys().map(|n| self.place_name(n)).collect();
            let words = if places.is_empty() {
                "No device answers yet.".to_string()
            } else {
                format!("Reading the agents on {}…", places.join(", "))
            };
            pane::detail(
                &t,
                "agents-detail",
                pane::head(&t),
                pane::empty(&t, words, None).p_0(),
            )
        } else {
            match self.state.chosen.clone() {
                Chosen::Tool(id) if tools.iter().any(|x| x.id == id) => {
                    self.tool_detail(&t, &reports, &id, several, cx)
                }
                Chosen::Checkout(name) if checkouts.iter().any(|c| c.name == name) => {
                    self.checkout_detail(&t, &reports, &name, several, cx)
                }
                _ => self.overview(&t, &tools, &off, several, cx),
            }
        };
        pane::page(&t, list, detail)
            .text_color(t.text)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        t: &Theme,
        id: &str,
        name: &str,
        // What the name tells tests, beside what it shows.
        name_selector: Option<String>,
        meta: Option<(String, Option<Hsla>)>,
        pick: Chosen,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let chosen = self.state.chosen == pick;
        let selector = id.to_string();
        pane::row(t, SharedString::from(id.to_string()), chosen)
            .debug_selector(move || selector)
            .child(
                pane::cell(name.to_string())
                    .when_some(name_selector, |d, sel| d.debug_selector(move || sel)),
            )
            .children(meta.map(|(m, c)| pane::meta(t, m, c)))
            .on_click(cx.listener(move |s, _, _, cx| {
                s.state.chosen = pick.clone();
                cx.notify();
            }))
    }

    /// The first screen: one line when all is in order; otherwise each
    /// thing that is off with its fix.
    fn overview(
        &self,
        t: &Theme,
        tools: &[&Tool],
        off: &[(&str, Off)],
        several: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let head = pane::head(t).child(pane::title(t, "Overview"));
        let body = if off.is_empty() {
            let running: Vec<String> = tools
                .iter()
                .filter(|x| x.skills.is_some() && x.path.is_some())
                .map(|x| match &x.version {
                    Some(v) => format!("{} {v}", x.name),
                    None => x.name.clone(),
                })
                .collect();
            div()
                .debug_selector(|| "agents-ready".into())
                .child(format!(
                    "Ready: {}. Every skill is linked and level with its checkout.",
                    running.join(", ")
                ))
        } else {
            let count = off.len();
            let mut rows = div()
                .debug_selector(move || format!("agents-off-{count}"))
                .flex()
                .flex_col();
            for (node, o) in off {
                rows = rows.child(self.off_row(t, node, o, several, cx));
            }
            rows
        };
        pane::detail(t, "agents-detail", head, body)
    }

    /// One thing that is off: its name, what is wrong, and its fix (a
    /// Sync, or the words of what to do by hand).
    fn off_row(
        &self,
        t: &Theme,
        node: &str,
        o: &Off,
        several: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let subject = if several {
            format!("{} on {}", o.subject, self.place_name(node))
        } else {
            o.subject.clone()
        };
        let mut words = div()
            .text_color(tone_colour(t, o.tone))
            .child(o.what.clone());
        if let Some(f) = &o.failure {
            let selector = format!("agents-sync-message-{f}");
            words = words.debug_selector(move || selector);
        }
        let fix: AnyElement = match &o.fix {
            Fix::Sync { path, setup } => {
                let name = o.checkout.clone().unwrap_or_default();
                div()
                    .flex()
                    .child(self.sync_button(t, node, &name, path, *setup, cx))
                    .into_any_element()
            }
            Fix::Words(w) => div()
                .text_color(t.text2)
                .child(w.clone())
                .into_any_element(),
        };
        let row = div()
            .flex()
            .items_start()
            .gap(px(size::HEAD_GAP))
            .py(px(tokens::OFF_PAD))
            .border_b_1()
            .border_color(t.rule())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_color(t.text).child(subject))
                    .child(words),
            )
            .child(div().flex_none().max_w(px(tokens::FIX_W)).child(fix));
        match (&o.failure, &o.checkout) {
            (Some(_), Some(name)) => {
                let selector = format!("agents-sync-failure-{node}-{name}");
                row.debug_selector(move || selector)
            }
            _ => row,
        }
    }

    fn sync_button(
        &self,
        t: &Theme,
        node: &str,
        name: &str,
        path: &str,
        setup: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let busy = self.state.starting(node, path);
        let why = if setup {
            "Fetch, fast-forward to its upstream and run its setup, in a new session"
        } else {
            "Fetch and fast-forward to its upstream, in a new session"
        };
        let (n, p) = (node.to_string(), path.to_string());
        let selector = format!("agents-sync-{node}-{name}");
        let b = kit::button(
            t,
            SharedString::from(format!("sync-{node}-{name}")),
            if busy { "Syncing…" } else { "Sync" },
            Kind::Plain,
        )
        .debug_selector(move || selector)
        .tooltip(kit::tip(why));
        if busy {
            b.opacity(tokens::DIMMED)
        } else {
            b.on_click(cx.listener(move |s, _, _, cx| s.agents_sync(&n, &p, cx)))
        }
    }

    fn tool_detail(
        &self,
        t: &Theme,
        reports: &[(&str, &Report)],
        id: &str,
        several: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let at: Vec<(&str, &Report, &Tool)> = reports
            .iter()
            .filter_map(|(n, r)| Some((*n, *r, r.tools.iter().find(|x| x.id == id)?)))
            .collect();
        let head = pane::head(t).child(pane::title(t, at[0].2.name.clone()));
        let mut body = div().flex().flex_col().gap(px(size::SECTION_GAP));
        for (node, r, tl) in &at {
            let mut lines = div().flex().flex_col();
            lines = lines.child(pane::line(
                t,
                "Command",
                match &tl.path {
                    Some(p) => div().child(p.clone()),
                    None => div()
                        .text_color(t.dim)
                        .child(format!("{} is not installed", tl.command)),
                },
            ));
            if tl.path.is_some() {
                lines = lines
                    .child(pane::line(
                        t,
                        "Version",
                        div().child(tl.version.clone().unwrap_or_else(|| "unknown".into())),
                    ))
                    .child(pane::line(
                        t,
                        "Signed in",
                        match tl.signed_in {
                            Some(true) => div().child("yes"),
                            Some(false) => div()
                                .text_color(tone_colour(t, Tone::Behind))
                                .child(format!("no: run {} and sign in", tl.command)),
                            None => div().text_color(t.text2).child("unknown"),
                        },
                    ))
                    .child(pane::line(
                        t,
                        "Credentials",
                        div().child(if tl.credentials_present {
                            "on this device"
                        } else {
                            "not found"
                        }),
                    ));
            }
            if let Some(folder) = &tl.skills {
                let linked = agents::tool_skills(r, tl)
                    .iter()
                    .filter(|s| s.links.get(folder).is_some_and(|l| l != "missing"))
                    .count();
                let path = r
                    .folders
                    .iter()
                    .find(|f| &f.id == folder)
                    .map_or(folder.clone(), |f| f.path.clone());
                lines = lines.child(pane::line(
                    t,
                    "Skills",
                    div().child(format!("{} in {path}", agents::skills(linked))),
                ));
            }
            if !tl.settings.is_empty() {
                let mut files = div().flex().flex_col();
                for st in &tl.settings {
                    files = files.child(div().when(!st.present, |d| d.text_color(t.dim)).child(
                        if st.present {
                            st.path.clone()
                        } else {
                            format!("{} (none)", st.path)
                        },
                    ));
                }
                lines = lines.child(pane::line(t, "Settings", files));
            }
            // What is off in the checkouts its skills come from.
            let feeds: Vec<String> = agents::tool_checkouts(r, tl)
                .iter()
                .map(|c| c.name.clone())
                .collect();
            let mine: Vec<Off> = agents::off(r)
                .into_iter()
                .filter(|o| {
                    o.subject == tl.name || o.checkout.as_ref().is_some_and(|c| feeds.contains(c))
                })
                .collect();
            let mut part = if several {
                pane::section(t, self.place_name(node))
            } else {
                div().flex().flex_col()
            };
            part = part.child(lines);
            if !mine.is_empty() {
                let mut rows = div().flex().flex_col();
                for o in &mine {
                    rows = rows.child(self.off_row(t, node, o, false, cx));
                }
                part = part.child(
                    pane::section(t, "To fix")
                        .pt(px(size::SECTION_GAP))
                        .child(rows),
                );
            }
            body = body.child(part);
        }
        pane::detail(t, "agents-detail", head, body)
    }

    fn checkout_detail(
        &self,
        t: &Theme,
        reports: &[(&str, &Report)],
        name: &str,
        several: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let at: Vec<(&str, &Report, &Checkout)> = reports
            .iter()
            .filter_map(|(n, r)| Some((*n, *r, r.checkouts.iter().find(|c| c.name == name)?)))
            .collect();
        let mut head = pane::head(t).child(pane::title(t, name.to_string()));
        for (node, _, c) in &at {
            head = head.child(self.sync_button(t, node, &c.name, &c.path, c.setup.is_some(), cx));
        }
        let mut body = div().flex().flex_col().gap(px(size::SECTION_GAP));
        for (node, r, c) in &at {
            let mut lines = div()
                .flex()
                .flex_col()
                .child(pane::line(t, "Path", div().child(c.path.clone())))
                .child(pane::line(
                    t,
                    "At",
                    div()
                        .flex()
                        .gap(px(size::HEAD_GAP))
                        .child(format!(
                            "{} {}",
                            c.branch.as_deref().unwrap_or("detached"),
                            c.commit.as_deref().unwrap_or("")
                        ))
                        .children(c.subject.clone().map(|s| pane::cell(s).text_color(t.text2))),
                ))
                .child(pane::line(
                    t,
                    "Upstream",
                    div().child(agents::upstream_words(c)),
                ));
            if c.dirty {
                lines = lines.child(pane::line(t, "Local changes", div().child("yes")));
            }
            if let Some(s) = &c.setup {
                lines = lines.child(pane::line(t, "Setup", div().child(format!("./{s}"))));
            }
            lines = lines.child(pane::line(
                t,
                "Skills",
                div().child(agents::skills(agents::checkout_skills(r, c).len())),
            ));
            let mine: Vec<Off> = agents::off(r)
                .into_iter()
                .filter(|o| o.checkout.as_deref() == Some(name))
                .collect();
            // What is off, first; its Sync is in the head.
            for o in &mine {
                let mut fix = div()
                    .text_color(tone_colour(t, o.tone))
                    .child(o.what.clone());
                if let Some(f) = &o.failure {
                    let selector = format!("agents-sync-message-{f}");
                    fix = fix.debug_selector(move || selector);
                }
                let mut value = div().flex().flex_col().child(fix);
                if let Fix::Words(w) = &o.fix {
                    value = value.child(div().text_color(t.text2).child(w.clone()));
                }
                let mut line = pane::line(t, "To fix", value);
                if o.failure.is_some() {
                    let selector = format!("agents-sync-failure-{node}-{name}");
                    line = line.debug_selector(move || selector);
                }
                lines = div().flex().flex_col().child(line).child(lines);
            }
            body = body.child(if several {
                pane::section(t, self.place_name(node)).child(lines)
            } else {
                lines
            });
        }
        pane::detail(t, "agents-detail", head, body)
    }
}

/// Needs the operator in the theme's warm colour (ANSI yellow), broken in
/// its error colour, the rest quiet.
fn tone_colour(t: &Theme, tone: Tone) -> Hsla {
    match tone {
        Tone::Behind | Tone::Unlinked => rgb(t.ansi[3]),
        Tone::Error => t.error,
        Tone::Plain => t.dim,
        Tone::Ok => t.text2,
    }
}

fn rank(t: Tone) -> u8 {
    match t {
        Tone::Error => 0,
        Tone::Behind | Tone::Unlinked => 1,
        Tone::Plain => 2,
        Tone::Ok => 3,
    }
}

fn worst_tone(tones: impl Iterator<Item = Tone>) -> Tone {
    tones.min_by_key(|t| rank(*t)).unwrap_or(Tone::Ok)
}

/// The word of the place that most needs the operator.
fn worst(states: impl Iterator<Item = (String, Tone)>) -> (String, Tone) {
    states
        .min_by_key(|(_, t)| rank(*t))
        .unwrap_or(("—".into(), Tone::Plain))
}

fn worst_word(states: impl Iterator<Item = (String, Tone)>) -> Option<(String, Tone)> {
    states.min_by_key(|(_, t)| rank(*t))
}

use crate::state::{self as agents, Checkout, Chosen, Drift, Report, Skill, Tone, Tool};
use crate::{AgentsPage, tokens};
use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, Hsla, SharedString, Window, div, prelude::*,
    px,
};
use parvi_app_kit::theme::{Theme, alpha, board, rgb, size, theme};
use parvi_app_kit::widgets::kit::{self, Kind};

impl AgentsPage {
    pub(crate) fn render_agents(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx).clone();
        if self.library.open {
            return self.render_role_library(window, cx);
        }
        let filter = self.agents_filter(window, cx);
        let reports = self.agents_reports();
        if reports.is_empty() {
            let places: Vec<String> = self.places.keys().map(|n| self.place_name(n)).collect();
            let words = if places.is_empty() {
                "No place answers; the agents are read once one does.".to_string()
            } else {
                format!("Reading the agents on {}…", places.join(", "))
            };
            return div()
                .size_full()
                .bg(t.bg)
                .p(px(board::PAD))
                .child(kit::note(&t, words, Some(t.dim)))
                .child(
                    kit::button(&t, "agents-library-empty", "Roles library", Kind::Plain).on_click(
                        cx.listener(|s, _, _, cx| {
                            s.library.open = true;
                            s.load_role_library(cx);
                        }),
                    ),
                )
                .into_any_element();
        }
        let words = filter.read(cx).text.trim().to_lowercase();
        let shown = |s: &str| s.to_lowercase().contains(&words);
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
        let harnesses = tools.iter().filter(|x| x.skills.is_some()).count();
        let mut rail = div()
            .id("agents-rail")
            .w(px(tokens::RAIL_W))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .bg(t.surface)
            .border_r_1()
            .border_color(t.divider)
            .child(
                div()
                    .h(px(size::TAB_BAR))
                    .px(px(tokens::P12))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(t.divider)
                    .child(
                        div()
                            .text_size(px(board::HEADING))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Agents"),
                    )
                    .child(div().flex_1())
                    .child(kit::note(&t, format!("{harnesses} harnesses"), Some(t.dim))),
            )
            .child(div().p(px(tokens::P8)).child(filter))
            .child(
                kit::button(&t, "agents-library", "Roles library", Kind::Plain)
                    .debug_selector(|| "agents-library".into())
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.library.open = true;
                        s.load_role_library(cx);
                    })),
            );
        let groups: [(&str, Vec<&Tool>); 2] = [
            (
                "Harnesses",
                tools
                    .iter()
                    .copied()
                    .filter(|x| x.skills.is_some())
                    .collect(),
            ),
            (
                "Tools",
                tools
                    .iter()
                    .copied()
                    .filter(|x| x.skills.is_none())
                    .collect(),
            ),
        ];
        for (label, list) in groups {
            let list: Vec<&Tool> = list
                .into_iter()
                .filter(|x| shown(&x.name) || shown(&x.command))
                .collect();
            if list.is_empty() {
                continue;
            }
            rail = rail.child(group_label(&t, label));
            for tool in list {
                let (word, tone) = worst(reports.iter().filter_map(|(_, r)| {
                    let mine = r.tools.iter().find(|x| x.id == tool.id)?;
                    Some(agents::tool_state(r, mine))
                }));
                let chosen = self.state.chosen == Chosen::Tool(tool.id.clone());
                let sub = div()
                    .flex()
                    .gap(px(tokens::P4))
                    .child(tool.command.clone())
                    .child("·")
                    .child(
                        div()
                            .debug_selector({
                                let id = tool.id.clone();
                                let word = word.clone();
                                move || format!("agents-tool-state-{id}-{word}")
                            })
                            .text_color(tone_colour(&t, tone))
                            .child(word),
                    );
                let pick = Chosen::Tool(tool.id.clone());
                rail = rail.child(self.rail_row(
                    &t,
                    &format!("tool-{}", tool.id),
                    &tool.name,
                    sub,
                    chosen,
                    pick,
                    cx,
                ));
            }
        }
        let list: Vec<&Checkout> = checkouts.into_iter().filter(|c| shown(&c.name)).collect();
        if !list.is_empty() {
            rail = rail.child(group_label(&t, "Skills from"));
            for c in list {
                let mut count = 0;
                let mut behind = 0;
                let mut unlinked = 0;
                let mut broken = 0;
                for (_, r) in &reports {
                    if let Some(mine) = r.checkouts.iter().find(|x| x.name == c.name) {
                        count = count.max(agents::checkout_skills(r, mine).len());
                        for drift in agents::checkout_drift(r, mine) {
                            match drift.tone {
                                Tone::Behind => behind += 1,
                                Tone::Unlinked => unlinked += 1,
                                Tone::Error => broken += 1,
                                _ => {}
                            }
                        }
                    }
                }
                let chosen = self.state.chosen == Chosen::Checkout(c.name.clone());
                let sub = div()
                    .flex()
                    .gap(px(tokens::P4))
                    .child(format!("{count} skills"))
                    .when(behind > 0, |d| {
                        d.child("·").child(
                            div()
                                .text_color(tone_colour(&t, Tone::Behind))
                                .child(format!("{behind} behind")),
                        )
                    })
                    .when(unlinked > 0, |d| {
                        d.child("·").child(
                            div()
                                .text_color(tone_colour(&t, Tone::Unlinked))
                                .child(format!("{unlinked} not linked")),
                        )
                    })
                    .when(broken > 0, |d| {
                        d.child("·")
                            .child(div().text_color(t.error).child(format!("{broken} broken")))
                    });
                let pick = Chosen::Checkout(c.name.clone());
                rail = rail.child(self.rail_row(
                    &t,
                    &format!("checkout-{}", c.name),
                    &c.name,
                    sub,
                    chosen,
                    pick,
                    cx,
                ));
            }
        }
        let detail = match self.state.chosen.clone() {
            Chosen::Tool(id) if tools.iter().any(|x| x.id == id) => {
                self.tool_detail(&t, &reports, &id, cx)
            }
            Chosen::Checkout(name)
                if reports
                    .iter()
                    .any(|(_, r)| r.checkouts.iter().any(|c| c.name == name)) =>
            {
                self.checkout_detail(&t, &reports, &name, cx)
            }
            _ => kit::note(&t, "Choose a harness, a tool or a checkout.", Some(t.dim)),
        };
        let mut sync_failures = div().flex().flex_col().gap(px(tokens::P8));
        for (node, report) in &reports {
            for checkout in &report.checkouts {
                if let Some(message) = agents::sync_failure(checkout) {
                    let selector = format!("agents-sync-failure-{node}-{}", checkout.name);
                    let message_selector = format!("agents-sync-message-{message}");
                    sync_failures = sync_failures.child(
                        div()
                            .debug_selector(move || selector.clone())
                            .text_color(t.error)
                            .child(format!("{} · {} · ", self.place_name(node), checkout.name))
                            .child(
                                div()
                                    .debug_selector(move || message_selector.clone())
                                    .child(message),
                            ),
                    );
                }
            }
        }
        div()
            .size_full()
            .flex()
            .bg(t.bg)
            .child(rail)
            .child(
                div()
                    .id("agents-detail")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .p(px(board::PAD))
                            .max_w(px(tokens::DETAIL_MAX))
                            .child(sync_failures)
                            .child(detail),
                    ),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn rail_row(
        &self,
        t: &Theme,
        id: &str,
        name: &str,
        sub: Div,
        chosen: bool,
        pick: Chosen,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!("agents-{id}")))
            .debug_selector({
                let id = format!("agents-{id}");
                move || id.clone()
            })
            .flex()
            .items_center()
            .gap(px(tokens::P10))
            .px(px(tokens::P12))
            .py(px(tokens::P6))
            .cursor_pointer()
            .when(chosen, |d| d.bg(alpha(t.accent, tokens::SELECTED)))
            .hover(|d| d.bg(alpha(t.text, tokens::HOVER)))
            .on_click(cx.listener(move |s, _: &ClickEvent, _, cx| {
                s.state.chosen = pick.clone();
                cx.notify();
            }))
            .child(avatar(t, name))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name.to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(size::SMALL))
                            .text_color(t.dim)
                            .child(sub),
                    ),
            )
            .into_any_element()
    }

    fn tool_detail(
        &self,
        t: &Theme,
        reports: &[(&str, &Report)],
        id: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let at: Vec<(&str, &Report, &Tool)> = reports
            .iter()
            .filter_map(|(n, r)| Some((*n, *r, r.tools.iter().find(|x| x.id == id)?)))
            .collect();
        let (_, first_r, tool) = at[0];
        let mut d = head(t, &tool.name, &tool.command);
        // Versions: one row per place.
        let mut versions = table_head(
            t,
            &[
                "Place",
                "CLI",
                "Signed in",
                "Credentials",
                "Skills",
                "State",
            ],
        );
        for (node, r, tl) in &at {
            let (word, tone) = agents::tool_state(r, tl);
            let skills = match &tl.skills {
                Some(f) => agents::tool_skills(r, tl)
                    .iter()
                    .filter(|s| s.links.get(f).is_some_and(|l| l != "missing"))
                    .count()
                    .to_string(),
                None => "—".into(),
            };
            let signed = match (&tl.path, tl.signed_in) {
                (None, _) => text(t, "—", None),
                (_, None) => text(t, "unknown", None),
                (_, Some(true)) => text(t, "yes", None),
                (_, Some(false)) => text(t, "no", Some(tone_colour(t, Tone::Behind))),
            };
            versions = versions.child(row(
                t,
                vec![
                    self.place_tag(t, node).into_any_element(),
                    text(t, tl.version.as_deref().unwrap_or("—"), None),
                    signed,
                    text(
                        t,
                        if tl.credentials_present {
                            "present"
                        } else {
                            "not detected"
                        },
                        None,
                    ),
                    text(t, &skills, None),
                    text(t, &word, Some(tone_colour(t, tone))),
                ],
            ));
        }
        d = d.child(section(t, "Versions").child(versions));
        for (node, r, tl) in &at {
            let place = self.place_name(node);
            let mut s = section(t, &format!("Drift on {place}")).child(drift_list(
                t,
                &place,
                &agents::tool_drift(r, tl),
            ));
            let checkouts = agents::tool_checkouts(r, tl);
            if !checkouts.is_empty() {
                s = s.child(self.sync_controls(t, node, &place, &checkouts, cx));
            }
            d = d.child(s);
        }
        let mut settings = table_head(t, &["File", "Place", "State"]);
        for (node, _, tl) in &at {
            for st in &tl.settings {
                settings = settings.child(row(
                    t,
                    vec![
                        text(t, &st.path, None),
                        self.place_tag(t, node).into_any_element(),
                        if st.present {
                            text(t, "present", None)
                        } else {
                            text(t, "missing", Some(t.dim))
                        },
                    ],
                ));
            }
        }
        d = d.child(section(t, "Settings").child(settings));
        if let Some(folder) = &tool.skills {
            let folders: Vec<&agents::Folder> =
                first_r.folders.iter().filter(|f| &f.id == folder).collect();
            let skills = agents::tool_skills(first_r, tool);
            d = d.child(
                section(t, &format!("Skills on {}", self.place_name(at[0].0)))
                    .child(skills_table(t, &skills, &folders)),
            );
        }
        d
    }

    fn checkout_detail(
        &self,
        t: &Theme,
        reports: &[(&str, &Report)],
        name: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let at: Vec<(&str, &Report, &Checkout)> = reports
            .iter()
            .filter_map(|(n, r)| Some((*n, *r, r.checkouts.iter().find(|c| c.name == name)?)))
            .collect();
        let (first_node, first_r, first) = at[0];
        let mut d = head(t, &first.name, &first.path);
        for (node, r, c) in &at {
            let place = self.place_name(node);
            let tone = if c.behind > 0 {
                Tone::Behind
            } else if c.fetch == "failed" {
                Tone::Error
            } else {
                Tone::Plain
            };
            let summary = div()
                .flex()
                .flex_col()
                .gap(px(tokens::P4))
                .child(
                    div()
                        .flex()
                        .gap(px(tokens::P8))
                        .items_center()
                        .child(self.place_tag(t, node))
                        .child(format!(
                            "{} at {}",
                            c.branch.as_deref().unwrap_or("detached"),
                            c.commit.as_deref().unwrap_or("—")
                        ))
                        .when_some(c.subject.clone(), |d, s| {
                            d.child(div().text_color(t.dim).child(s))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(tokens::P4))
                        .child(
                            div()
                                .text_color(tone_colour(t, tone))
                                .child(agents::upstream_words(c)),
                        )
                        .when(c.dirty, |d| d.child(" · local changes"))
                        .child(format!(" · {} skills", agents::checkout_skills(r, c).len()))
                        .when_some(c.setup.clone(), |d, s| d.child(format!(" · setup ./{s}"))),
                );
            d = d.child(
                section(t, &format!("Drift on {place}"))
                    .child(summary)
                    .child(drift_list(t, &place, &agents::checkout_drift(r, c)))
                    .child(self.sync_controls(t, node, &place, &[c], cx)),
            );
        }
        let skills = agents::checkout_skills(first_r, first);
        let folders: Vec<&agents::Folder> = first_r.folders.iter().collect();
        d.child(
            section(t, &format!("Skills on {}", self.place_name(first_node)))
                .child(skills_table(t, &skills, &folders)),
        )
    }

    fn sync_controls(
        &self,
        t: &Theme,
        node: &str,
        place: &str,
        checkouts: &[&Checkout],
        cx: &mut Context<Self>,
    ) -> Div {
        let mut buttons = div().flex().gap(px(tokens::P8)).items_center();
        for (i, c) in checkouts.iter().enumerate() {
            let label = if checkouts.len() == 1 {
                if c.setup.is_some() {
                    format!("Sync and run setup on {place}")
                } else {
                    format!("Sync {place}")
                }
            } else {
                format!("Sync {}", c.name)
            };
            let busy = self.state.starting(node, &c.path);
            let why = format!(
                "git pull{} in {}, in a new session",
                c.setup
                    .as_deref()
                    .map(|s| format!(" and ./{s}"))
                    .unwrap_or_default(),
                c.path
            );
            let (n, path) = (node.to_string(), c.path.clone());
            let b = kit::button(
                t,
                SharedString::from(format!("sync-{node}-{}", c.name)),
                label,
                if i == 0 { Kind::Primary } else { Kind::Plain },
            )
            .tooltip(kit::tip(why));
            buttons = buttons.child(if busy {
                b.opacity(tokens::DIMMED)
            } else {
                b.on_click(
                    cx.listener(move |s, _: &ClickEvent, _, cx| s.agents_sync(&n, &path, cx)),
                )
            });
        }
        buttons = buttons.child(
            kit::button(
                t,
                SharedString::from(format!("agents-refresh-{node}")),
                "Refresh",
                Kind::Plain,
            )
            .tooltip(kit::tip("Read everything again and fetch every checkout"))
            .on_click(cx.listener(|s, _: &ClickEvent, _, cx| s.agents_refresh(cx))),
        );
        div()
            .flex()
            .flex_col()
            .gap(px(tokens::P6))
            .pt(px(tokens::P4))
            .child(buttons)
            .child(kit::note(
                t,
                "Secrets stay on each machine; Parvi only sees whether they are there.",
                Some(t.dim),
            ))
    }
}

/// Gold needs the operator (`--window-orange`, ANSI yellow), error is
/// broken, plain is dim.
fn tone_colour(t: &Theme, tone: Tone) -> Hsla {
    match tone {
        Tone::Behind | Tone::Unlinked => rgb(t.ansi[3]),
        Tone::Error => t.error,
        Tone::Plain => t.dim,
        Tone::Ok => t.text2,
    }
}

/// The word of the place that most needs the operator.
fn worst(states: impl Iterator<Item = (String, Tone)>) -> (String, Tone) {
    let rank = |t: Tone| match t {
        Tone::Error => 0,
        Tone::Behind | Tone::Unlinked => 1,
        Tone::Plain => 2,
        Tone::Ok => 3,
    };
    states
        .min_by_key(|(_, t)| rank(*t))
        .unwrap_or(("—".into(), Tone::Plain))
}

fn avatar(t: &Theme, name: &str) -> Div {
    div()
        .flex_none()
        .size(px(tokens::AVATAR))
        .rounded(px(size::RADIUS))
        .border_1()
        .border_color(t.divider)
        .bg(t.raised)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(size::SMALL))
        .font_weight(FontWeight::BOLD)
        .text_color(t.text2)
        .child(agents::initials(name))
}

fn group_label(t: &Theme, label: &str) -> Div {
    div()
        .px(px(tokens::P12))
        .pt(px(tokens::P12))
        .pb(px(tokens::P4))
        .child(kit::label(t, label.to_string()))
}

fn head(t: &Theme, name: &str, sub: &str) -> Div {
    div().flex().flex_col().gap(px(tokens::P20)).child(
        div()
            .flex()
            .items_center()
            .gap(px(tokens::P12))
            .child(avatar(t, name).size(px(tokens::LARGE_AVATAR)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(tokens::TITLE))
                            .font_weight(FontWeight::BOLD)
                            .child(name.to_string()),
                    )
                    .child(kit::note(t, sub.to_string(), Some(t.dim))),
            ),
    )
}

fn section(t: &Theme, label: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(tokens::P8))
        .child(kit::label(t, label.to_string()))
}

fn text(t: &Theme, s: &str, colour: Option<Hsla>) -> AnyElement {
    div()
        .text_color(colour.unwrap_or(t.text))
        .overflow_hidden()
        .whitespace_nowrap()
        .child(s.to_string())
        .into_any_element()
}

fn table_head(t: &Theme, labels: &[&str]) -> Div {
    let head = row(
        t,
        labels
            .iter()
            .map(|l| kit::label(t, l.to_string()).into_any_element())
            .collect(),
    );
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(t.divider)
        .rounded(px(size::RADIUS))
        .child(head.bg(t.surface))
}

fn row(t: &Theme, cells: Vec<AnyElement>) -> Div {
    let mut r = div()
        .flex()
        .items_center()
        .gap(px(tokens::P12))
        .min_h(px(tokens::TABLE_ROW))
        .px(px(tokens::P10))
        .border_b_1()
        .border_color(alpha(t.divider, tokens::LINE));
    for c in cells {
        r = r.child(div().flex_1().min_w_0().overflow_hidden().child(c));
    }
    r
}

fn drift_list(t: &Theme, place: &str, drift: &[Drift]) -> Div {
    if drift.is_empty() {
        return kit::note(t, format!("Nothing out of date on {place}."), Some(t.dim));
    }
    let mut list = div().flex().flex_col().gap(px(tokens::P2));
    for d in drift {
        let mut at = vec![place.to_string()];
        at.extend(d.path.iter().cloned());
        list = list.child(
            div()
                .flex()
                .gap(px(tokens::P6))
                .child(div().text_color(t.text2).child(at.join(" › ")))
                .child(
                    div()
                        .text_color(tone_colour(t, d.tone))
                        .child(d.what.clone()),
                ),
        );
    }
    if drift.iter().any(|d| d.tone == Tone::Unlinked) {
        list = list.child(kit::note(
            t,
            "To add missing links, run the checkout's setup if it provides one, or link the skill into the folder.",
            Some(t.dim),
        ));
    }
    list
}

fn skills_table(t: &Theme, skills: &[&Skill], folders: &[&agents::Folder]) -> Div {
    let mut labels = vec!["Name", "Description"];
    labels.extend(folders.iter().map(|f| f.path.as_str()));
    let mut tbl = table_head(t, &labels);
    if skills.is_empty() {
        return tbl.child(
            div()
                .p(px(tokens::P10))
                .child(kit::note(t, "No skills.", Some(t.dim))),
        );
    }
    for s in skills {
        let mut cells = vec![
            text(t, &s.name, None),
            text(t, s.description.as_deref().unwrap_or(""), Some(t.text2)),
        ];
        for f in folders {
            let (word, tone) = agents::skill_cell(s, &f.id);
            cells.push(text(t, word, Some(tone_colour(t, tone))));
        }
        tbl = tbl.child(row(t, cells));
    }
    tbl
}

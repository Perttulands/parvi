//! A mission's Input cards and steps in execution order, carried from the
//! original Flow view. Device hops are outside this release's scope.
use super::*;
use gpui::SharedString;
use parvi_app_kit::theme::Theme;
use parvi_missions_contract::Mission;

impl MissionsPage {
    /// What an empty mission says in Flow and on the canvas: how to begin.
    pub(super) fn empty_mission(
        &self,
        t: &Theme,
        selector: &'static str,
        cx: &mut Context<Self>,
    ) -> Div {
        let button = format!("{selector}-input");
        card(t)
            .debug_selector(move || selector.into())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("This mission is empty"),
            )
            .child(div().text_color(t.text2).child(
                "Start with an Input: what you hand the mission when you run it. \
                 Then add a step with the + on its card, and an End where the work is done.",
            ))
            .child(
                div().flex().gap(px(TAB_GAP)).child(
                    kit::button(
                        t,
                        SharedString::from(button.clone()),
                        "Add the Input",
                        Kind::Primary,
                    )
                    .debug_selector(move || button.clone())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.mission_canvas_mode = true;
                        page.add_card(board::Kind::Input, cx);
                    })),
                ),
            )
    }

    pub(super) fn render_mission_flow(
        &self,
        t: &Theme,
        mission: &Mission,
        cx: &mut Context<Self>,
    ) -> Div {
        let flow = flow::build(mission);
        let mut body = div().flex().flex_col().gap(px(PAD)).p(px(PAD)).w_full();
        if mission.input_cards.is_empty() && mission.formations.is_empty() {
            return body.child(self.empty_mission(t, "flow-empty", cx));
        }
        for section in &flow.sections {
            if let Some(id) = &section.input
                && let Some(input) = mission.input_cards.iter().find(|c| &c.id == id)
            {
                let start = id.clone();
                let edit = id.clone();
                let selector = format!("flow-input-{id}");
                let actions = div()
                    .flex()
                    .gap(px(TAB_GAP))
                    .child(
                        kit::button(
                            t,
                            SharedString::from(format!("flow-edit-{id}")),
                            "Edit",
                            Kind::Quiet,
                        )
                        .on_click(cx.listener(move |page, _, _, cx| page.edit_step(&edit, cx))),
                    )
                    .child(
                        kit::button(
                            t,
                            SharedString::from(format!("flow-start-{id}")),
                            "Run…",
                            if self.mission_input_runnable(id) {
                                Kind::Plain
                            } else {
                                Kind::Quiet
                            },
                        )
                        .debug_selector({
                            let id = id.clone();
                            move || format!("flow-start-{id}")
                        })
                        .on_click(
                            cx.listener(move |page, _, _, cx| page.prepare_card_run(&start, cx)),
                        ),
                    );
                let mut card = card(t)
                    .debug_selector(move || selector.clone())
                    .child(header(
                        t,
                        kit::label(t, "Input"),
                        input.title.clone(),
                        actions,
                    ));
                if !input.goal.trim().is_empty() {
                    card = card.child(div().text_color(t.text2).child(input.goal.clone()));
                }
                if input.inputs.is_empty() {
                    card = card.child(fact(t, "asks for", "nothing".into()));
                }
                for value in &input.inputs {
                    let mut words = format!(
                        "{} · {}{}",
                        value.name,
                        value.kind,
                        if value.required { "" } else { ", optional" }
                    );
                    if !value.description.is_empty() {
                        words.push_str(&format!(" — {}", value.description));
                    }
                    card = card.child(fact(t, "asks for", words));
                }
                if input.outputs.is_empty() {
                    card = card.child(fact(t, "gives back", "nothing declared".into()));
                }
                for value in &input.outputs {
                    let from = value
                        .from
                        .split_once(':')
                        .map(|(step, _)| format!(" from {}", mission.node_name(step)))
                        .unwrap_or_else(|| " (not wired yet)".into());
                    let kind = if value.kind.is_empty() {
                        "text"
                    } else {
                        &value.kind
                    };
                    card = card.child(fact(
                        t,
                        "gives back",
                        format!("{} · {kind}{from}", value.name),
                    ));
                }
                if super::canvas::asks_you(mission) {
                    card = card.child(fact(t, "human gates", "notify me".into()));
                }
                body = body.child(card);
            }
            for step in &section.steps {
                let id = step.id.clone();
                let selector = format!("flow-step-{id}");
                let kind = match &step.kind {
                    flow::StepKind::Formation { .. } => mission
                        .formations
                        .iter()
                        .find(|f| f.id == step.id)
                        .map(|f| f.kind.clone())
                        .unwrap_or_default(),
                    flow::StepKind::Gate { .. } => "gate".into(),
                };
                let edit = kit::button(
                    t,
                    SharedString::from(format!("flow-edit-{id}")),
                    "Edit",
                    Kind::Quiet,
                )
                .debug_selector({
                    let id = id.clone();
                    move || format!("flow-edit-{id}")
                })
                .on_click(cx.listener(move |page, _, _, cx| page.edit_step(&id, cx)));
                let mut card = card(t)
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .child(header(
                        t,
                        div()
                            .flex_none()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(step.number.to_string()),
                        format!("{}  ·  {kind}", mission.node_name(&step.id)),
                        div().child(edit),
                    ));
                match &step.kind {
                    flow::StepKind::Formation { next } => {
                        if let Some(formation) = mission.formations.iter().find(|f| f.id == step.id)
                        {
                            if let Some(brief) = &formation.brief
                                && !brief.goal.trim().is_empty()
                            {
                                card = card.child(
                                    div()
                                        .text_color(t.text2)
                                        .line_clamp(3)
                                        .child(brief.goal.trim().to_owned()),
                                );
                            }
                            if formation.slots.is_empty() {
                                card = card.child(
                                    div().text_color(t.error).child("Nobody staffs this step."),
                                );
                            } else {
                                for slot in &formation.slots {
                                    card = card.child(fact(
                                        t,
                                        "staffing",
                                        crate::store::words::staffing(slot),
                                    ));
                                }
                            }
                        }
                        card = card.child(fact(t, "then", run::target_words(next)));
                    }
                    flow::StepKind::Gate {
                        deciders,
                        judges,
                        pass,
                        fail,
                    } => {
                        let who = deciders
                            .iter()
                            .map(|d| match d {
                                flow::Decider::You => "you",
                                flow::Decider::Judge => "a judge",
                                flow::Decider::Code => "a check",
                            })
                            .collect::<Vec<_>>()
                            .join(" and ");
                        if let Some(gate) = mission.gates.iter().find(|g| g.id == step.id)
                            && !gate.criterion.is_empty()
                        {
                            card =
                                card.child(div().text_color(t.text2).child(gate.criterion.clone()));
                        }
                        card = card.child(fact(t, "decided by", who));
                        for judge in judges {
                            if let Some(formation) =
                                mission.formations.iter().find(|f| &f.id == judge)
                            {
                                card = card.child(fact(
                                    t,
                                    "judge",
                                    format!(
                                        "{} · {}",
                                        mission.node_name(judge),
                                        formation
                                            .slots
                                            .iter()
                                            .map(crate::store::words::staffing)
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    ),
                                ));
                            }
                        }
                        card = card
                            .child(fact(t, "on pass", run::target_words(pass)))
                            .child(fact(t, "on fail", run::target_words(fail)));
                    }
                }
                if step.parallel.len() > 1 {
                    card = card.child(fact(
                        t,
                        "alongside",
                        step.parallel
                            .iter()
                            .filter(|other| **other != step.id)
                            .map(|other| mission.node_name(other))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ));
                }
                for limit in mission
                    .limits
                    .iter()
                    .filter(|limit| limit.target == step.id)
                {
                    let mut words = Vec::new();
                    if let Some(n) = limit.rounds {
                        words.push(format!("{n} rounds"));
                    }
                    if let Some(n) = limit.seconds {
                        words.push(crate::store::words::duration_text(n));
                    }
                    if let Some(n) = limit.tokens {
                        words.push(format!("{n} tokens"));
                    }
                    card = card.child(fact(
                        t,
                        "limit",
                        format!("{}, then {}", words.join(", "), limit.when_spent),
                    ));
                }
                body = body.child(card);
            }
        }
        body
    }
}

/// A card on the Flow view.
fn card(t: &Theme) -> Div {
    div()
        .p(px(PAD))
        .rounded(px(size::RADIUS))
        .border_1()
        .border_color(t.divider)
        .bg(t.surface)
        .flex()
        .flex_col()
        .gap(px(TAB_GAP))
}

/// A card's first line: a lead, its title and its actions on the right.
fn header(t: &Theme, lead: Div, title: String, actions: Div) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(PAD))
        .child(lead)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_ellipsis()
                .text_color(t.text)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(actions)
}

/// One named fact on a card, the name in a narrow dim column.
fn fact(t: &Theme, name: &'static str, value: String) -> Div {
    div()
        .flex()
        .gap(px(PAD))
        .child(
            div()
                .w(px(size::LABEL_COL / 2.))
                .flex_none()
                .text_color(t.dim)
                .child(name),
        )
        .child(div().flex_1().min_w_0().child(value))
}

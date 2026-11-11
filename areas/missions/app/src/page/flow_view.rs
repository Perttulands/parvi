//! A mission's Input cards and steps in execution order, carried from the
//! original Flow view. Device hops are outside this release's scope.
use super::*;
use gpui::SharedString;
use parvi_app_kit::theme::Theme;
use parvi_missions_contract::Mission;

impl MissionsPage {
    pub(super) fn render_mission_flow(
        &self,
        t: &Theme,
        mission: &Mission,
        cx: &mut Context<Self>,
    ) -> Div {
        let flow = flow::build(mission);
        let mut body = div().flex().flex_col().gap(px(PAD)).p(px(PAD)).w_full();
        for section in &flow.sections {
            if let Some(id) = &section.input
                && let Some(input) = mission.input_cards.iter().find(|c| &c.id == id)
            {
                let start = id.clone();
                let edit = id.clone();
                let selector = format!("flow-input-{id}");
                let mut card = div()
                    .debug_selector(move || selector.clone())
                    .p(px(PAD))
                    .border_1()
                    .border_color(t.divider)
                    .bg(t.surface)
                    .flex()
                    .flex_col()
                    .gap(px(TAB_GAP))
                    .child(kit::label(t, "Input"))
                    .child(input.title.clone())
                    .child(input.goal.clone());
                if input.inputs.is_empty() {
                    card = card.child("Inputs · None declared");
                }
                for value in &input.inputs {
                    card = card.child(format!(
                        "Input · {} · {} · {}",
                        value.name,
                        value.kind,
                        if value.required {
                            "required"
                        } else {
                            "optional"
                        }
                    ));
                    if !value.description.is_empty() {
                        card = card.child(div().text_color(t.dim).child(value.description.clone()));
                    }
                }
                if input.outputs.is_empty() {
                    card = card.child("Outputs · None declared");
                }
                for value in &input.outputs {
                    card = card.child(format!("Output · {}", value.name));
                }
                card = card.child("Human gates · Notify me");
                card = card
                    .child(
                        kit::button(
                            t,
                            SharedString::from(format!("flow-start-{id}")),
                            "Run",
                            Kind::Primary,
                        )
                        .debug_selector({
                            let id = id.clone();
                            move || format!("flow-start-{id}")
                        })
                        .on_click(
                            cx.listener(move |page, _, _, cx| page.prepare_card_run(&start, cx)),
                        ),
                    )
                    .child(
                        kit::button(
                            t,
                            SharedString::from(format!("flow-edit-{id}")),
                            "Edit",
                            Kind::Quiet,
                        )
                        .on_click(cx.listener(move |page, _, _, cx| page.edit_step(&edit, cx))),
                    );
                body = body.child(card);
            }
            if !section.start.is_empty() {
                body = body.child(format!("Starts at {}", run::target_words(&section.start)));
            }
            for step in &section.steps {
                let id = step.id.clone();
                let selector = format!("flow-step-{id}");
                let mut card = div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .p(px(PAD))
                    .border_1()
                    .border_color(t.divider)
                    .bg(t.surface)
                    .flex()
                    .flex_col()
                    .gap(px(TAB_GAP))
                    .child(format!("{} · {}", step.number, mission.node_name(&step.id)));
                match &step.kind {
                    flow::StepKind::Formation { next } => {
                        if let Some(formation) = mission.formations.iter().find(|f| f.id == step.id)
                        {
                            card = card.child(kit::label(t, formation.kind.clone()));
                            if let Some(brief) = &formation.brief
                                && !brief.goal.trim().is_empty()
                            {
                                card = card.child(brief.goal.clone());
                            }
                            if formation.slots.is_empty() {
                                card = card.child(
                                    div().text_color(t.error).child("Nobody staffs this step."),
                                );
                            } else {
                                for slot in &formation.slots {
                                    card = card.child(format!(
                                        "Staffing · {}",
                                        crate::store::words::staffing(slot)
                                    ));
                                }
                            }
                        }
                        card = card.child(format!("Next · {}", run::target_words(next)));
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
                                flow::Decider::Judge => "judge",
                                flow::Decider::Code => "code",
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        card = card
                            .child(kit::label(t, "Gate"))
                            .child(format!("Decided by · {who}"));
                        if let Some(gate) = mission.gates.iter().find(|g| g.id == step.id)
                            && !gate.criterion.is_empty()
                        {
                            card = card.child(gate.criterion.clone());
                        }
                        for judge in judges {
                            if let Some(formation) =
                                mission.formations.iter().find(|f| &f.id == judge)
                            {
                                card = card.child(format!(
                                    "Judge · {} · {}",
                                    mission.node_name(judge),
                                    formation
                                        .slots
                                        .iter()
                                        .map(crate::store::words::staffing)
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ));
                            }
                        }
                        card = card
                            .child(format!("Pass · {}", run::target_words(pass)))
                            .child(format!("Fail · {}", run::target_words(fail)));
                    }
                }
                if step.parallel.len() > 1 {
                    card = card.child(format!(
                        "Alongside · {}",
                        step.parallel
                            .iter()
                            .filter(|other| **other != step.id)
                            .map(|other| mission.node_name(other))
                            .collect::<Vec<_>>()
                            .join(", ")
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
                    card = card.child(format!(
                        "Limit · {} · {}",
                        words.join(", "),
                        limit.when_spent
                    ));
                }
                card = card.child(
                    kit::button(
                        t,
                        SharedString::from(format!("flow-edit-{id}")),
                        "Edit",
                        Kind::Quiet,
                    )
                    .debug_selector({
                        let id = id.clone();
                        move || format!("flow-edit-{id}")
                    })
                    .on_click(cx.listener(move |page, _, _, cx| page.edit_step(&id, cx))),
                );
                body = body.child(card);
            }
        }
        body
    }
}

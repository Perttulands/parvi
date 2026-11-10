//! Find an unoccupied grid position without moving saved cards.
use crate::{AuthoringError, Layout, LayoutNode, Mission, Store};
use std::collections::HashMap;

pub fn free_layout_position(
    mission: &Mission,
    layout: Option<&Layout>,
    desired_x: i64,
    desired_y: i64,
) -> Result<LayoutNode, AuthoringError> {
    if layout.is_some_and(|layout| layout.mission_id != mission.id) {
        return Err(AuthoringError::new(
            "CONFLICT",
            "layout belongs to another mission",
        ));
    }
    let saved: HashMap<_, _> = layout
        .into_iter()
        .flat_map(|l| &l.nodes)
        .map(|n| (n.id.as_str(), n))
        .collect();
    let ids = mission
        .input_cards
        .iter()
        .map(|n| &n.id)
        .chain(mission.formations.iter().map(|n| &n.id))
        .chain(mission.gates.iter().map(|n| &n.id))
        .chain(mission.ends.iter().map(|n| &n.id))
        .chain(mission.limits.iter().map(|n| &n.id));
    let occupied: Vec<_> = ids
        .enumerate()
        .map(|(i, id)| {
            saved
                .get(id.as_str())
                .map_or((140 + i as i64 * 308, 168 + (i % 2) as i64 * 196), |n| {
                    (n.x, n.y)
                })
        })
        .collect();
    fn snap(n: i64) -> i64 {
        (n.max(112).saturating_add(14) / 28) * 28
    }
    let (mut x, mut y) = (snap(desired_x), snap(desired_y));
    for _ in 0..24 {
        if occupied
            .iter()
            .all(|(ox, oy)| ox.abs_diff(x) >= 308 || oy.abs_diff(y) >= 280)
        {
            return Ok(LayoutNode {
                id: String::new(),
                x,
                y,
            });
        }
        x = x.saturating_add(336);
        if x > 1900 {
            x = 112;
            y = y.saturating_add(336);
        }
    }
    Err(AuthoringError::new(
        "CONFLICT",
        "no free layout position within bounded search",
    ))
}
impl Store {
    pub fn find_free_layout_position(
        &self,
        selector: &str,
        x: i64,
        y: i64,
    ) -> Result<LayoutNode, AuthoringError> {
        let scope = crate::write::locks(self.root(), "scope");
        let _reference_guard = scope.references.read().unwrap_or_else(|e| e.into_inner());
        let slug = self.resolve(selector)?;
        let locks = crate::write::locks(self.root(), &format!("mission:{slug}"));
        let _document_guard = locks.document.lock().unwrap_or_else(|e| e.into_inner());
        if self.resolve(selector)? != slug {
            return Err(AuthoringError::new("CONFLICT", "mission selector changed"));
        }
        free_layout_position(
            &self.read_mission(&slug)?,
            self.read_layout(&slug)?.as_ref(),
            x,
            y,
        )
    }
}

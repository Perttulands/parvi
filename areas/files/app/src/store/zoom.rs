//! How big a picture is drawn (CHROTE `imageZoom.ts`): one level, fit or a
//! percentage, for every picture this app draws, because the operator is
//! setting how they look at pictures, not annotating one.
//!
//! Fit draws the picture at 1:1 when it fits its room and down at its own
//! ratio when it does not, never up. A percentage is obeyed literally, so a
//! picture can be bigger than the room and scroll inside it. Stepping lands
//! on the next stop of a ladder, from what is on screen now.

/// A picture's or a room's size in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Px {
    pub w: f32,
    pub h: f32,
}

/// The level every picture is drawn at: `None` is fit, `Some` a percentage.
pub type Level = Option<u32>;

pub const MIN: u32 = 10;
pub const MAX: u32 = 800;

/// The stops a step lands on, smallest first.
const LADDER: [u32; 14] = [
    10, 25, 33, 50, 67, 75, 100, 125, 150, 200, 300, 400, 600, 800,
];

/// A level as kept: a percentage outside the range comes back inside it.
pub fn settle(level: Level) -> Level {
    level.map(|p| p.clamp(MIN, MAX))
}

/// At 1:1 when it fits the room, down at its own ratio when it does not.
pub fn fit(natural: Px, room: Px) -> Px {
    let scale = 1f32.min(room.w / natural.w).min(room.h / natural.h).max(0.);
    Px {
        w: (natural.w * scale).floor().max(1.),
        h: (natural.h * scale).floor().max(1.),
    }
}

/// The picture as the level draws it, in whole pixels and never under one.
pub fn draw(natural: Px, room: Px, level: Level) -> Px {
    match level {
        None => fit(natural, room),
        Some(p) => {
            let scale = p as f32 / 100.;
            Px {
                w: (natural.w * scale).round().max(1.),
                h: (natural.h * scale).round().max(1.),
            }
        }
    }
}

/// The percent the picture is drawn at, as the header says it.
pub fn drawn_percent(natural: Px, room: Px, level: Level) -> u32 {
    match level {
        Some(p) => p,
        None => ((fit(natural, room).w / natural.w * 100.).round() as u32).max(1),
    }
}

/// The next stop up (`up`) or down the ladder from where the picture is
/// drawn now; at either end it stays.
pub fn step(level: Level, natural: Px, room: Px, up: bool) -> Level {
    let here = drawn_percent(natural, room, level);
    let stop = if up {
        LADDER.iter().copied().find(|&s| s > here)
    } else {
        LADDER.iter().rev().copied().find(|&s| s < here)
    };
    Some(stop.unwrap_or(here).clamp(MIN, MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: Px = Px { w: 400., h: 300. };

    fn px(w: f32, h: f32) -> Px {
        Px { w, h }
    }

    // CHROTE FilePanelViewer.test: fits a tall picture and a wide one whole
    // inside the room, and keeps a small one at 1:1.
    #[test]
    fn fit_shrinks_to_the_room_and_never_grows() {
        assert_eq!(draw(px(300., 1200.), ROOM, None), px(75., 300.));
        assert_eq!(draw(px(1600., 400.), ROOM, None), px(400., 100.));
        assert_eq!(draw(px(40., 30.), ROOM, None), px(40., 30.));
    }

    // CHROTE: refits when the room changes size.
    #[test]
    fn fit_follows_the_room() {
        assert_eq!(draw(px(300., 1200.), px(400., 600.), None), px(150., 600.));
    }

    // CHROTE: draws a picture at a percentage zoom literally, bigger than
    // the room.
    #[test]
    fn a_percentage_is_literal() {
        assert_eq!(draw(px(300., 1200.), ROOM, Some(200)), px(600., 2400.));
        assert_eq!(
            draw(px(1., 1.), ROOM, Some(10)),
            px(1., 1.),
            "never under a pixel"
        );
    }

    #[test]
    fn the_first_step_from_fit_starts_from_what_is_on_screen() {
        // Fit draws this one at 25%: up is the next stop, down the one before.
        let big = px(1600., 1200.);
        assert_eq!(drawn_percent(big, ROOM, None), 25);
        assert_eq!(step(None, big, ROOM, true), Some(33));
        assert_eq!(step(None, big, ROOM, false), Some(10));
        // Between stops, a step lands on the nearest one that way.
        assert_eq!(step(Some(140), big, ROOM, true), Some(150));
        assert_eq!(step(Some(140), big, ROOM, false), Some(125));
    }

    #[test]
    fn steps_stop_at_the_ends() {
        let p = px(100., 100.);
        assert_eq!(step(Some(800), p, ROOM, true), Some(800));
        assert_eq!(step(Some(10), p, ROOM, false), Some(10));
        assert_eq!(settle(Some(5000)), Some(800));
        assert_eq!(settle(Some(0)), Some(10));
        assert_eq!(settle(None), None);
    }
}

//! How big a diagram beside the terminals is drawn, and where it is moved
//! to: fit (the panel's width, up to a ceiling so a small diagram is not
//! blown up and down to a floor so its labels stay readable), or a
//! percentage; and the pan that keeps it in its room.

/// A size or a place, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Px {
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Level {
    #[default]
    Fit,
    Percent(u32),
}

/// Fit grows a diagram to the panel's width up to this many times its size.
pub const FIT_MOST: f32 = 2.;
/// And shrinks it no further than this, where mermaid's 16 px labels are
/// still read at a glance; a wider diagram is dragged along instead.
pub const FIT_LEAST: f32 = 0.7;
const LADDER: [u32; 13] = [10, 25, 33, 50, 67, 75, 100, 125, 150, 200, 300, 400, 600];

/// The scale the level draws a diagram of `natural` size at in `room`.
pub fn scale(level: Level, natural: Px, room: Px) -> f32 {
    match level {
        Level::Fit => (room.w / natural.w.max(1.)).clamp(FIT_LEAST, FIT_MOST),
        Level::Percent(p) => p as f32 / 100.,
    }
}

pub fn percent(level: Level, natural: Px, room: Px) -> u32 {
    (scale(level, natural, room) * 100.).round().max(1.) as u32
}

/// The next stop up or down the ladder from what is drawn now.
pub fn step(level: Level, natural: Px, room: Px, up: bool) -> Level {
    let here = percent(level, natural, room);
    let stop = if up {
        LADDER.iter().copied().find(|&s| s > here)
    } else {
        LADDER.iter().rev().copied().find(|&s| s < here)
    };
    Level::Percent(stop.unwrap_or(here))
}

/// Where the drawing's top-left goes in the room, given how far it was
/// dragged: centred along a side it fits, kept covering the room along a
/// side it overflows. Returns the place and the pan as kept.
pub fn place(drawn: Px, room: Px, pan: Px) -> (Px, Px) {
    let side = |d: f32, r: f32, p: f32| {
        if d <= r {
            ((r - d) / 2., 0.)
        } else {
            let p = p.clamp(r - d, 0.);
            (p, p)
        }
    };
    let (x, px) = side(drawn.w, room.w, pan.w);
    let (y, py) = side(drawn.h, room.h, pan.h);
    (Px { w: x, h: y }, Px { w: px, h: py })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: Px = Px { w: 500., h: 600. };

    fn px(w: f32, h: f32) -> Px {
        Px { w, h }
    }

    #[test]
    fn fit_fills_the_width_within_a_floor_and_a_ceiling() {
        assert_eq!(scale(Level::Fit, px(625., 300.), ROOM), 0.8);
        assert_eq!(
            scale(Level::Fit, px(1000., 300.), ROOM),
            FIT_LEAST,
            "a wide diagram keeps readable labels and is dragged along"
        );
        assert_eq!(scale(Level::Fit, px(400., 100.), ROOM), 1.25);
        assert_eq!(scale(Level::Fit, px(100., 50.), ROOM), FIT_MOST);
        assert_eq!(scale(Level::Percent(100), px(100., 50.), ROOM), 1.);
    }

    #[test]
    fn steps_start_from_what_is_on_screen_and_stop_at_the_ends() {
        let big = px(625., 300.);
        assert_eq!(percent(Level::Fit, big, ROOM), 80);
        assert_eq!(step(Level::Fit, big, ROOM, true), Level::Percent(100));
        assert_eq!(step(Level::Fit, big, ROOM, false), Level::Percent(75));
        assert_eq!(
            step(Level::Percent(600), big, ROOM, true),
            Level::Percent(600)
        );
    }

    #[test]
    fn a_drawing_that_fits_is_centred_and_one_that_overflows_pans_within_bounds() {
        let (at, pan) = place(px(300., 200.), ROOM, px(-50., 40.));
        assert_eq!(at, px(100., 200.));
        assert_eq!(pan, px(0., 0.));
        let (at, pan) = place(px(1000., 900.), ROOM, px(-800., 40.));
        assert_eq!(
            at,
            px(-500., 0.),
            "stops where its far edge meets the room's"
        );
        assert_eq!(pan, at);
    }
}

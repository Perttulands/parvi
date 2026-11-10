//! A selection drag that leaves the grid, as xterm's SelectionService has
//! it: the selection keeps following the pointer, held to the grid's edge;
//! above the grid it reaches the start of the top row, below it the end of
//! the bottom row, and where there is history to read the view scrolls,
//! faster the further out the pointer is.

use std::time::Duration;

/// How often a drag held outside the grid scrolls (xterm's
/// DRAG_SCROLL_INTERVAL).
pub const SCROLL_EVERY: Duration = Duration::from_millis(50);
/// Pixels outside the grid at which scrolling is fastest.
const MAX_THRESHOLD: f32 = 50.;
/// Lines per tick at its fastest.
const MAX_SPEED: f32 = 15.;

/// Lines to scroll per tick for a pointer `y` pixels below the top of a view
/// `height` tall: none inside, toward the bottom positive, from one line
/// just outside to fifteen at 50 px out.
pub fn scroll_amount(y: f32, height: f32) -> i32 {
    if (0.0..=height).contains(&y) {
        return 0;
    }
    let out = if y > height { y - height } else { y };
    let out = out.clamp(-MAX_THRESHOLD, MAX_THRESHOLD) / MAX_THRESHOLD;
    (out.signum() + (out * (MAX_SPEED - 1.)).round()) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_the_view_nothing_scrolls() {
        assert_eq!(scroll_amount(0., 300.), 0);
        assert_eq!(scroll_amount(150., 300.), 0);
        assert_eq!(scroll_amount(300., 300.), 0);
    }

    #[test]
    fn outside_it_scrolls_faster_further_out_up_to_fifteen_lines() {
        assert_eq!(scroll_amount(-1., 300.), -1);
        assert_eq!(scroll_amount(301., 300.), 1);
        assert_eq!(scroll_amount(-25., 300.), -8);
        assert_eq!(scroll_amount(350., 300.), 15);
        assert_eq!(scroll_amount(-400., 300.), -15);
    }
}

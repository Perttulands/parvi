//! Pictures a session's output names, read once from the session's device,
//! and the mermaid diagrams it prints, drawn once here; kept small for the
//! terminal to draw at the line that names them. Only the latest few are
//! kept; what falls out is handed back to be freed.

use gpui::ImageFormat;
use std::collections::{HashMap, VecDeque};

/// A picture: a file by its device and its path there, or a diagram by
/// its source.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    File { node: String, path: String },
    Diagram(String),
}

/// The most pictures kept at once.
pub const KEEP: usize = 24;

/// The format a path names a picture in, by its extension.
pub fn format(path: &str) -> Option<ImageFormat> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "gif" => ImageFormat::Gif,
        "webp" => ImageFormat::Webp,
        "svg" => ImageFormat::Svg,
        _ => return None,
    })
}

pub enum Picture<I> {
    Reading,
    Ready(I),
    /// Not on the device, too big, outside what it shares, or not a
    /// picture after all: not asked for again.
    Failed,
}

pub struct Pictures<I> {
    kept: HashMap<Key, Picture<I>>,
    /// Least recently wanted first.
    order: VecDeque<Key>,
    gone: Vec<I>,
}

impl<I> Default for Pictures<I> {
    fn default() -> Self {
        Self {
            kept: HashMap::new(),
            order: VecDeque::new(),
            gone: Vec::new(),
        }
    }
}

impl<I> Pictures<I> {
    /// The picture is in view: true the first time, when it is to be read.
    pub fn want(&mut self, key: &Key) -> bool {
        if let Some(at) = self.order.iter().position(|k| k == key) {
            let k = self.order.remove(at).expect("a position found");
            self.order.push_back(k);
            return false;
        }
        self.kept.insert(key.clone(), Picture::Reading);
        self.order.push_back(key.clone());
        while self.order.len() > KEEP {
            let old = self.order.pop_front().expect("more than KEEP");
            if let Some(Picture::Ready(i)) = self.kept.remove(&old) {
                self.gone.push(i);
            }
        }
        true
    }

    /// A read came back: the picture, or nothing to show.
    pub fn arrived(&mut self, key: &Key, picture: Option<I>) {
        match (self.kept.get_mut(key), picture) {
            (Some(p), Some(i)) => *p = Picture::Ready(i),
            (Some(p), None) => *p = Picture::Failed,
            // Fallen out while it was read.
            (None, Some(i)) => self.gone.push(i),
            (None, None) => {}
        }
    }

    pub fn get(&self, key: &Key) -> Option<&I> {
        match self.kept.get(key)? {
            Picture::Ready(i) => Some(i),
            _ => None,
        }
    }

    /// Pictures no longer kept, to free.
    pub fn take_gone(&mut self) -> Vec<I> {
        std::mem::take(&mut self.gone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: usize) -> Key {
        Key::File {
            node: "node".into(),
            path: format!("/p/{n}.png"),
        }
    }

    #[test]
    fn a_picture_is_read_once_and_kept_until_newer_ones_push_it_out() {
        let mut p = Pictures::<usize>::default();
        assert!(p.want(&key(0)));
        assert!(!p.want(&key(0)), "asked again while reading");
        assert_eq!(p.get(&key(0)), None);
        p.arrived(&key(0), Some(100));
        assert_eq!(p.get(&key(0)), Some(&100));
        for n in 1..KEEP {
            p.want(&key(n));
        }
        // Wanted again, so the oldest is now 1.
        p.want(&key(0));
        p.want(&key(KEEP));
        assert!(p.take_gone().is_empty(), "1 was still reading");
        assert_eq!(p.get(&key(0)), Some(&100));
        for n in KEEP + 1..2 * KEEP {
            p.want(&key(n));
        }
        assert_eq!(p.take_gone(), vec![100]);
        assert_eq!(p.get(&key(0)), None);
    }

    #[test]
    fn a_failed_read_is_not_asked_again_and_a_late_one_is_freed() {
        let mut p = Pictures::<usize>::default();
        p.want(&key(0));
        p.arrived(&key(0), None);
        assert!(!p.want(&key(0)));
        p.arrived(&key(9), Some(7));
        assert_eq!(p.take_gone(), vec![7]);
    }

    #[test]
    fn pictures_are_known_by_their_extension() {
        assert_eq!(format("/a/Shot.PNG"), Some(ImageFormat::Png));
        assert_eq!(format("plan.svg"), Some(ImageFormat::Svg));
        assert_eq!(format("notes.md"), None);
        assert_eq!(format("png"), None);
    }
}

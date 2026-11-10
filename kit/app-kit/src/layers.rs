//! One owner of dismissal (CHROTE's keys/dismiss.ts): every layer over
//! the page is on one stack. A new layer opens above the others, Escape
//! closes the top one, and a key reaches the page under them only when
//! the stack is empty. A layer of a kind already open moves to the top
//! rather than opening twice, and a passing one (a menu) closes when
//! anything else opens.

pub trait Layer {
    /// Which layer this is; the stack holds one of each.
    fn kind(&self) -> &str;
    /// A moment: anything else opening closes it.
    fn passing(&self) -> bool;
}

#[derive(Clone, Debug)]
pub struct Stack<T>(Vec<T>);

impl<T> Default for Stack<T> {
    fn default() -> Self {
        Self(vec![])
    }
}

impl<T: Layer> Stack<T> {
    pub fn open(&mut self, s: T) {
        let kind = s.kind();
        self.0.retain(|x| x.kind() != kind && !x.passing());
        self.0.push(s);
    }

    /// Closes the layer of this kind and everything opened above it,
    /// top first.
    pub fn close(&mut self, kind: &str) -> Vec<T> {
        match self.0.iter().position(|s| s.kind() == kind) {
            Some(i) => self.0.drain(i..).rev().collect(),
            None => vec![],
        }
    }

    /// Escape: the top layer, or `None` when the key is the page's.
    pub fn dismiss(&mut self) -> Option<T> {
        self.0.pop()
    }

    pub fn is_open(&self, kind: &str) -> bool {
        self.0.iter().any(|s| s.kind() == kind)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn top(&self) -> Option<&T> {
        self.0.last()
    }

    pub fn top_mut(&mut self) -> Option<&mut T> {
        self.0.last_mut()
    }

    /// Bottom first, the order they draw in.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct S(&'static str);
    const MENU: &str = "menu";

    impl Layer for S {
        fn kind(&self) -> &str {
            self.0
        }
        fn passing(&self) -> bool {
            self.0 == MENU
        }
    }

    fn kinds(s: &Stack<S>) -> Vec<&'static str> {
        s.iter().map(|x| x.0).collect()
    }

    #[test]
    fn escape_closes_the_newest_first_and_then_reaches_the_page() {
        let mut s = Stack::default();
        s.open(S("1"));
        s.open(S("2"));
        s.open(S("3"));
        assert_eq!(s.dismiss(), Some(S("3")));
        assert_eq!(s.dismiss(), Some(S("2")));
        assert_eq!(s.dismiss(), Some(S("1")));
        assert_eq!(s.dismiss(), None, "nothing open: Escape is the page's");
        assert!(s.is_empty());
    }

    #[test]
    fn opening_again_moves_a_surface_to_the_top() {
        let mut s = Stack::default();
        s.open(S("1"));
        s.open(S("2"));
        s.open(S("1"));
        assert_eq!(kinds(&s), ["2", "1"]);
    }

    #[test]
    fn a_menu_closes_when_anything_opens_over_it() {
        let mut s = Stack::default();
        s.open(S("2"));
        s.open(S(MENU));
        s.open(S("4"));
        assert_eq!(kinds(&s), ["2", "4"]);
        // A menu opens over the others, and goes first on Escape.
        s.open(S(MENU));
        assert_eq!(s.dismiss(), Some(S(MENU)));
        assert_eq!(s.top(), Some(&S("4")));
    }

    #[test]
    fn closing_one_closes_what_opened_above_it() {
        let mut s = Stack::default();
        s.open(S("1"));
        s.open(S("2"));
        s.open(S("3"));
        assert_eq!(s.close("2"), [S("3"), S("2")]);
        assert_eq!(kinds(&s), ["1"]);
        assert!(s.close("5").is_empty());
        assert!(s.is_open("1") && !s.is_open("2"));
    }
}

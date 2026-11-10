//! The socket's frame envelope: one kind byte, then the payload. Each area's
//! contract declares the kinds it owns, in both directions; the node routes
//! a client's frame to the area whose kinds hold its kind byte.

use std::ops::RangeInclusive;

use crate::Error;

/// The kinds an area owns.
pub type Kinds = &'static [RangeInclusive<u8>];

/// The node's own first frame on every connection; no area may own it.
pub const HELLO: u8 = 0x00;
/// The serving node's peer list, and each change to it, down.
pub const NODES: u8 = 0x03;
/// Asks for the peer list and its changes, up.
pub const NODES_ON: u8 = 0x1b;
/// The app's view control, both ways: what it shows, and what it is told to.
pub const VIEW: Kinds = &[0xb0..=0xbf];

/// The kinds no area or page takes, each with what owns it: `parvi kinds`
/// lists them with the areas'.
pub const RESERVED: &[(&str, Kinds)] = &[
    ("the node's hello", &[HELLO..=HELLO]),
    (
        "the fleet's peer list",
        &[NODES..=NODES, NODES_ON..=NODES_ON],
    ),
    ("view control", VIEW),
];

/// What owns the reserved kinds `kinds` takes, if it takes any.
pub fn reserved(kinds: Kinds) -> Option<&'static str> {
    RESERVED
        .iter()
        .find(|(_, owned)| overlap(kinds, owned))
        .map(|(owner, _)| *owner)
}

/// `kinds` as a reader writes them: `0x01-0x02, 0x10`.
pub fn describe(kinds: Kinds) -> String {
    kinds
        .iter()
        .map(|range| match range.start() == range.end() {
            true => format!("{:#04x}", range.start()),
            false => format!("{:#04x}-{:#04x}", range.start(), range.end()),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether `kinds` holds `kind`.
pub fn owns(kinds: Kinds, kind: u8) -> bool {
    kinds.iter().any(|range| range.contains(&kind))
}

/// Whether two declarations share a kind.
pub fn overlap(a: Kinds, b: Kinds) -> bool {
    a.iter().any(|x| {
        b.iter()
            .any(|y| x.start() <= y.end() && y.start() <= x.end())
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let (&kind, payload) = bytes
            .split_first()
            .ok_or_else(|| Error::new("empty_frame", "frame has no kind"))?;
        Ok(Self {
            kind,
            payload: payload.to_vec(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(1 + self.payload.len());
        bytes.push(self.kind);
        bytes.extend_from_slice(&self.payload);
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_frame_round_trip_and_empty_input() {
        let bytes = [0x10, 0, 0xff, 3];
        assert_eq!(Frame::decode(&bytes).unwrap().encode(), bytes);
        assert_eq!(Frame::decode(&[]).unwrap_err().code, "empty_frame");
    }

    #[test]
    fn kinds_own_their_ranges_and_overlap_where_they_share_one() {
        const A: Kinds = &[0x01..=0x02, 0x10..=0x1a];
        const B: Kinds = &[0x1b..=0x1d];
        const C: Kinds = &[0x1a..=0x1a];
        assert!(owns(A, 0x02) && owns(A, 0x1a) && !owns(A, 0x1b) && !owns(A, 0x00));
        assert!(!overlap(A, B));
        assert!(overlap(A, C) && overlap(C, A));
        assert!(!overlap(&[], A));
        assert_eq!(describe(A), "0x01-0x02, 0x10-0x1a");
        assert_eq!(reserved(B), Some("the fleet's peer list"));
        assert_eq!(reserved(&[0xb8..=0xb8]), Some("view control"));
        assert_eq!(reserved(A), None);
    }
}

//! The socket's frame envelope: one kind byte, then the payload. Each area's
//! contract declares the kinds it owns, in both directions; the node routes
//! a client's frame to the area whose kinds hold its kind byte.

use std::ops::RangeInclusive;

use crate::Error;

/// The kinds an area owns.
pub type Kinds = &'static [RangeInclusive<u8>];

/// The node's own first frame on every connection; no area may own it.
pub const HELLO: u8 = 0x00;

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
    }
}

//! The frames the link speaks itself (docs/protocol.md): one kind byte, then
//! the body, numbers little-endian. Every other kind belongs to a page,
//! which decodes its own. Pure: decoding and encoding only.

use parvi_core::frame::HELLO;
use serde::Deserialize;

/// The serving node's peer list, and each change to it.
pub const NODES: u8 = 0x03;
const NODES_ON: u8 = 0x1b;

/// The kinds the link owns: no page may take them.
pub const OWN: parvi_core::frame::Kinds = &[HELLO..=HELLO, NODES..=NODES, NODES_ON..=NODES_ON];

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Peer {
    pub node: String,
    pub device: String,
    pub environment: String,
    pub version: String,
    pub protocol: u32,
    pub state: String,
    /// Where it answered on the tailnet, to dial it directly; empty from a
    /// node that does not say.
    #[serde(default)]
    pub address: String,
}

/// A nodes frame's body as the peer list.
pub fn nodes(body: &[u8]) -> Option<Vec<Peer>> {
    serde_json::from_slice(body).ok()
}

/// Turns a channel on (1) or off (0): its kind byte, then on.
pub fn channel(kind: u8, on: bool) -> Vec<u8> {
    vec![kind, u8::from(on)]
}

/// Asks the serving node for its peer list, and for each change to it.
pub fn nodes_on() -> Vec<u8> {
    vec![NODES_ON, 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_peer_list() {
        let list = nodes(br#"[{"node":"b","device":"lap","environment":"windows","version":"1","protocol":16,"state":"online"}]"#).unwrap();
        assert_eq!((list[0].node.as_str(), list[0].address.as_str()), ("b", ""));
    }

    #[test]
    fn a_channel_is_its_kind_then_on() {
        assert_eq!(channel(0x80, true), [0x80, 1]);
        assert_eq!(channel(0x80, false), [0x80, 0]);
        assert_eq!(nodes_on(), [NODES_ON, 1]);
    }
}

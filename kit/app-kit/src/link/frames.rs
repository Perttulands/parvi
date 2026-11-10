//! The frames the link speaks itself (docs/protocol.md): one kind byte, then
//! the body, numbers little-endian. Every other kind belongs to a page,
//! which decodes its own. Pure: decoding and encoding only.

use serde::Deserialize;

/// The node's first frame on every socket.
pub const HELLO: u8 = 0x00;
/// The serving node's peer list, and each change to it.
pub const NODES: u8 = 0x03;
const NODES_ON: u8 = 0x1b;

/// The kinds the link owns: no page may take them.
pub const OWN: parvi_core::frame::Kinds = &[HELLO..=HELLO, NODES..=NODES, NODES_ON..=NODES_ON];

/// The protocol this client speaks. Bump it with the node's hello.
pub const PROTOCOL: u32 = 16;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Hello {
    pub node: String,
    pub device: String,
    pub environment: String,
    pub version: String,
    pub protocol: u32,
    /// The areas this node has turned off.
    #[serde(default)]
    pub off: Vec<String>,
}

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

/// A hello frame's hello; `None` when it is not one or does not read.
pub fn hello(b: &[u8]) -> Option<Hello> {
    match b.split_first()? {
        (&HELLO, body) => serde_json::from_slice(body).ok(),
        _ => None,
    }
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
    fn reads_a_hello_and_a_peer_list() {
        let mut f = vec![HELLO];
        f.extend_from_slice(
            br#"{"node":"n","device":"rig","environment":"wsl","version":"1","protocol":16,"off":["dark"]}"#,
        );
        let h = hello(&f).unwrap();
        assert_eq!(
            (h.device.as_str(), h.off.as_slice()),
            ("rig", &["dark".to_string()][..])
        );
        assert_eq!(hello(&[NODES]), None);
        assert_eq!(hello(&[]), None);
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

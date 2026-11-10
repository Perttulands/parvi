//! The hello: the first frame on every `/ws` connection, saying which node
//! answered and which protocol it speaks. Nodes send it and apps read it;
//! there is no cross-version compatibility, so a client that speaks another
//! protocol stops here.

use serde::Deserialize;

use crate::frame::HELLO;

/// The protocol version. Bump it with any change to the `/ws` frames.
pub const PROTOCOL: u32 = 16;

/// A hello as a node sent it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Hello {
    pub node: String,
    pub device: String,
    pub environment: String,
    pub version: String,
    pub protocol: u32,
    /// The areas it has turned off.
    #[serde(default)]
    pub off: Vec<String>,
}

impl Hello {
    /// Reads a hello frame, or `None` when the frame is not one or does not read.
    pub fn decode(frame: &[u8]) -> Option<Hello> {
        let (&HELLO, json) = frame.split_first()? else {
            return None;
        };
        serde_json::from_slice(json).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_hello_and_nothing_else() {
        let mut f = vec![HELLO];
        f.extend_from_slice(
            br#"{"node":"n","device":"rig","environment":"wsl","version":"1","protocol":16,"off":["dark"]}"#,
        );
        let h = Hello::decode(&f).unwrap();
        assert_eq!(
            (h.device.as_str(), h.off.as_slice()),
            ("rig", &["dark".to_string()][..])
        );
        assert_eq!(Hello::decode(b"\x01{}"), None);
        assert_eq!(Hello::decode(&[]), None);
    }
}

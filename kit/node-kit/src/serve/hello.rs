//! The hello: the first frame on every `/ws` connection, saying which node
//! answered and which protocol it speaks. There is no cross-version
//! compatibility, so a client that speaks another protocol stops here.

use serde::Deserialize;

use crate::identity::Identity;
use parvi_core::frame::HELLO;
use parvi_core::json::push_string as push_json_string;

/// The protocol version. Bump it with any change to the `/ws` frames.
pub const PROTOCOL: u32 = 16;

/// `0x00` and JSON: `{"node":…,"device":…,"environment":"wsl","version":"0.0.1","protocol":16}`.
pub fn hello(node: &Identity, off: &[&str]) -> Vec<u8> {
    let mut json = String::from("{\"node\":");
    push_json_string(&mut json, node.id.as_str());
    json.push_str(",\"device\":");
    push_json_string(&mut json, &node.device);
    json.push_str(",\"environment\":");
    push_json_string(&mut json, node.environment.name());
    json.push_str(",\"version\":");
    push_json_string(&mut json, env!("CARGO_PKG_VERSION"));
    json.push_str(&format!(",\"protocol\":{PROTOCOL}"));
    // Listed only when some are off, so an older reader sees all on.
    if !off.is_empty() {
        json.push_str(&format!(",\"off\":{}", serde_json::json!(off)));
    }
    json.push('}');
    let mut frame = vec![HELLO];
    frame.extend_from_slice(json.as_bytes());
    frame
}

/// A hello as another node sent it.
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
    /// Reads a hello frame, or `None` when the frame is not one.
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
    use crate::identity::Environment;

    #[test]
    fn reads_back_the_hello_it_sends() {
        let node = Identity {
            id: "0123abcd".into(),
            device: "rig".into(),
            environment: Environment::Windows,
        };
        let hello = Hello::decode(&hello(&node, &["lights"])).unwrap();
        assert_eq!(hello.node, "0123abcd");
        assert_eq!(hello.device, "rig");
        assert_eq!(hello.environment, "windows");
        assert_eq!(hello.protocol, PROTOCOL);
        assert_eq!(hello.off, ["lights"]);
        assert_eq!(Hello::decode(b"\x01{}"), None);
        assert_eq!(Hello::decode(b""), None);
    }

    #[test]
    fn says_who_answered_and_which_protocol() {
        let node = Identity {
            id: "0123abcd".into(),
            device: "rig \"main\"".into(),
            environment: Environment::Wsl,
        };
        let frame = hello(&node, &[]);
        assert_eq!(frame[0], 0x00);
        assert_eq!(
            std::str::from_utf8(&frame[1..]).unwrap(),
            format!(
                r#"{{"node":"0123abcd","device":"rig \"main\"","environment":"wsl","version":"{}","protocol":16}}"#,
                env!("CARGO_PKG_VERSION")
            )
        );
    }
}

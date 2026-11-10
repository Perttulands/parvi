//! The same arrangement goldens Archon's Go store and dashboard use.
use parvi_missions::{Layout, LayoutNode, Mission, arrange};

#[test]
fn arrangements_match_archons_goldens() {
    for name in ["arrange-golden.json", "arrange-long-golden.json"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/parity")
            .join(name);
        let mut fixture: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let board = fixture["board"].as_object_mut().unwrap();
        board.remove("etag");
        board.remove("tools");
        for (from, to) in [
            ("inputCards", "inputCard"),
            ("formations", "formation"),
            ("gates", "gate"),
            ("ends", "end"),
            ("limits", "limit"),
            ("connections", "connection"),
        ] {
            if let Some(value) = board.remove(from) {
                board.insert(to.into(), value);
            }
        }
        if let Some(formations) = board.get_mut("formation").and_then(|v| v.as_array_mut()) {
            for formation in formations {
                let object = formation.as_object_mut().unwrap();
                for (from, to) in [
                    ("inputs", "input"),
                    ("outputs", "output"),
                    ("slots", "slot"),
                ] {
                    if let Some(value) = object.remove(from) {
                        object.insert(to.into(), value);
                    }
                }
            }
        }
        let mission: Mission = serde_json::from_value(fixture["board"].clone()).unwrap();
        let expected: Vec<LayoutNode> =
            serde_json::from_value(fixture["positions"].clone()).unwrap();
        let result = arrange(&mission, &Layout::default());
        assert_eq!(result.nodes, expected, "{name}");
    }
}

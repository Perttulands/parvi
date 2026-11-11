//! Mission wire types shared by the node and its clients. This crate does no I/O.

use serde::{Deserialize, Serialize};

/// Subscribe to mission summaries or run projections; the payload is one byte,
/// zero to stop watching and nonzero to start.
pub const WATCH: u8 = 0xa8;
pub const WATCH_RUNS: u8 = 0xa9;
/// Mission summary and run projection snapshots, respectively.
pub const SNAPSHOT: u8 = 0xa0;
pub const RUNS_SNAPSHOT: u8 = 0xa1;
pub const KINDS: parvi_core::frame::Kinds = &[0xa0..=0xa1, 0xa8..=0xa9];

pub const RUN_ADMISSION_FAILED: &str = "run_admission_failed";

/// A validation finding as it crosses the node boundary. The node maps each
/// engine finding into this shape, preserving its code, location and path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub code: String,
    pub node_id: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A rejected start has the common error shape and all admission findings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionRejection {
    pub error: parvi_core::Error,
    pub findings: Vec<Finding>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_start_preserves_all_findings() {
        let response = AdmissionRejection {
            error: parvi_core::Error::new(RUN_ADMISSION_FAILED, "run admission failed"),
            findings: vec![
                Finding {
                    code: "missing_input".into(),
                    node_id: "input".into(),
                    message: "input required".into(),
                    path: None,
                },
                Finding {
                    code: "missing_file".into(),
                    node_id: "step".into(),
                    message: "file missing".into(),
                    path: Some("brief.md".into()),
                },
            ],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert_eq!(
            serde_json::from_str::<AdmissionRejection>(&json).unwrap(),
            response
        );
    }
}

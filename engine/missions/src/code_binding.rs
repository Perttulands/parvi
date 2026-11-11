//! Immutable identity and limits for a run's pure code-gate profile.
use crate::{Gate, GateEvidence, JudgeVerdict};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const ENCODING: &str = "decision-result-jcs-v1";
const IMPLEMENTATION: &str = "c1b0017bd2707a31c35bc85c95d01f2bca9330ac592ff543867a5cc47346be11";
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeGateBinding {
    pub gate_binding_id: String,
    pub gate_id: String,
    pub profile_id: String,
    pub profile_version: String,
    pub profile_sha256: String,
    pub evaluator_bundle_sha256: String,
    pub parameters: BTreeMap<String, String>,
    pub parameters_sha256: String,
    pub policy_sha256: String,
    pub determinism_policy_sha256: String,
    pub max_input_bytes: usize,
    pub max_result_bytes: usize,
    pub max_operations: usize,
    pub result_encoding: String,
}
impl CodeGateBinding {
    pub fn new(run: &str, gate: &Gate) -> Result<Self, crate::Error> {
        (|| -> Result<Self, String> {
        let profile = gate.check.trim();
        let version = gate.check_version.trim();
        let operation = match (profile, version) {
            ("output_contains", "1") => "string_contains",
            ("output_absent", "1") => "string_absent",
            _ => return Err("unknown code gate profile".into()),
        };
        if gate.check_value.trim().is_empty() {
            return Err("code gate requires a nonempty check value".into());
        }
        let manifest = format!(
            "code-gate-profile-v1\nid={profile}\nversion={version}\nparameter=value:string:nonempty\nexecution=in_process\neffects=none\ndeterministic=true\ncertifiedTotal=true\nmaxInputBytes=262144\nmaxResultBytes=8192\nmaxOperations=524288\nresultEncoding={ENCODING}\nevaluatorImplementationSha256={IMPLEMENTATION}\noperation={operation}"
        );
        let policy = "code-gate-policy-v1\nexecution=in_process\neffects=none\nmaxInputBytes=262144\nmaxResultBytes=8192\nmaxOperations=524288";
        let determinism = format!(
            "code-gate-determinism-v1\ndeterministic=true\ncertifiedTotal=true\nresultEncoding={ENCODING}"
        );
        let parameters = BTreeMap::from([("value".into(), gate.check_value.clone())]);
        let encoded = serde_json::to_string(&parameters).map_err(|e| e.to_string())?;
        Ok(Self {
            gate_binding_id: format!("gbd_{}", &hash(&format!("{run}\0{}", gate.id))[..24]),
            gate_id: gate.id.clone(),
            profile_id: profile.into(),
            profile_version: version.into(),
            profile_sha256: hash(&manifest),
            evaluator_bundle_sha256: hash(&format!(
                "code-gate-evaluator-bundle-v1\nimplementationSha256={IMPLEMENTATION}\noperation={operation}"
            )),
            parameters,
            parameters_sha256: hash(&encoded),
            policy_sha256: hash(policy),
            determinism_policy_sha256: hash(&determinism),
            max_input_bytes: 256 * 1024,
            max_result_bytes: 8 * 1024,
            max_operations: 512 * 1024,
            result_encoding: ENCODING.into(),
        })
     })().map_err(|message| crate::Error::new("invalid_contract", message))
    }
    pub fn validate(&self, run: &str, gate: &Gate) -> Result<(), crate::Error> {
        (|| -> Result<(), String> {
            if *self != Self::new(run, gate).map_err(|error| error.message)? {
                return Err(format!(
                    "gate {:?} frozen evaluator binding mismatch",
                    gate.id
                ));
            }
            Ok(())
        })()
        .map_err(|message| crate::Error::new("invalid_contract", message))
    }
    pub(crate) fn result_data(
        &self,
        input: &str,
        result: &JudgeVerdict,
    ) -> Result<serde_json::Value, String> {
        let evidence = vec![GateEvidence {
            kind: "text".into(),
            text: result.reason.clone(),
        }];
        let canonical = serde_json::json!({"evidence":evidence,"reason":result.reason,"verdict":result.verdict}).to_string();
        if canonical.len() > self.max_result_bytes {
            return Err("code gate exhausted maxResultBytes".into());
        }
        Ok(
            serde_json::json!({"kind":"code","verdict":result.verdict,"reason":result.reason,"evidence":evidence,"resultEncoding":self.result_encoding,"resultSha256":hash(&canonical),"gateBindingId":self.gate_binding_id,"inputSha256":hash(input),"profileId":self.profile_id,"profileVersion":self.profile_version,"profileSha256":self.profile_sha256,"evaluatorBundleSha256":self.evaluator_bundle_sha256,"parametersSha256":self.parameters_sha256,"policySha256":self.policy_sha256,"determinismPolicySha256":self.determinism_policy_sha256,"maxInputBytes":self.max_input_bytes,"maxResultBytes":self.max_result_bytes,"maxOperations":self.max_operations}),
        )
    }
}

/// Reject a recorded code result whose binding, input or canonical decision
/// differs from the pure evaluator before a later route can use it.
pub(crate) fn validate_results(
    run: &str,
    mission: &crate::Mission,
    bindings: &crate::BindingsSnapshot,
    events: &[crate::RunEvent],
) -> Result<(), String> {
    for event in events.iter().filter(|event| {
        event.kind == "gate_kind_result"
            && event.data.get("kind").and_then(|v| v.as_str()) == Some("code")
    }) {
        let gate = mission
            .gate(&event.gate_id)
            .ok_or_else(|| "code result names an unknown gate".to_string())?;
        let binding = bindings
            .gate_binding(run, mission, gate)
            .map_err(|error| error.message)?;
        let evaluation = events
            .iter()
            .rev()
            .find(|prior| {
                prior.seq < event.seq
                    && prior.kind == "gate_evaluating"
                    && prior.gate_id == event.gate_id
            })
            .ok_or_else(|| "code result has no evaluation".to_string())?;
        if evaluation.data.get("inputRef") != event.data.get("inputRef") {
            return Err("code result input differs from its evaluation".into());
        }
        let input: crate::RoutedInput =
            serde_json::from_value(event.data.get("inputRef").cloned().unwrap_or_default())
                .map_err(|e| e.to_string())?;
        let result =
            crate::evaluate_code_check(gate, &input.text).map_err(|error| error.message)?;
        let expected = binding.result_data(&input.text, &result)?;
        for (key, value) in expected.as_object().expect("result data is an object") {
            if event.data.get(key) != Some(value) {
                return Err(format!("code gate durable {key} mismatch"));
            }
        }
    }
    for aggregate in events
        .iter()
        .filter(|event| event.kind == "human_input_requested" || event.kind == "gate_verdict")
    {
        let Some(gate) = mission.gate(&aggregate.gate_id) else {
            continue;
        };
        if !gate.kinds.iter().any(|kind| kind == "code") {
            continue;
        }
        let through = aggregate
            .data
            .get("requestedSeq")
            .and_then(|v| v.as_u64())
            .filter(|seq| *seq > 0)
            .unwrap_or(aggregate.seq);
        let evaluation = events
            .iter()
            .rev()
            .find(|event| {
                event.seq < through && event.kind == "gate_evaluating" && event.gate_id == gate.id
            })
            .ok_or_else(|| "aggregate has no evaluation".to_string())?;
        let code = events
            .iter()
            .find(|event| {
                event.seq > evaluation.seq
                    && event.seq < through
                    && event.kind == "gate_kind_result"
                    && event.gate_id == gate.id
                    && event.data.get("kind").and_then(|v| v.as_str()) == Some("code")
            })
            .ok_or_else(|| "aggregate is missing its code result".to_string())?;
        if aggregate.data.get("inputRef") != code.data.get("inputRef")
            || aggregate
                .data
                .get("kindResultSeqs")
                .and_then(|v| v.get("code"))
                .and_then(|v| v.as_u64())
                != Some(code.seq)
        {
            return Err("aggregate code result identity mismatch".into());
        }
        for (source, target) in [
            ("verdict", "codeVerdict"),
            ("reason", "codeReason"),
            ("resultEncoding", "resultEncoding"),
            ("resultSha256", "resultSha256"),
            ("gateBindingId", "gateBindingId"),
        ] {
            if code.data.get(source) != aggregate.data.get(target) {
                return Err(format!("aggregate {target} mismatch"));
            }
        }
        if aggregate.kind == "gate_verdict" {
            let code_verdict = code
                .data
                .get("verdict")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if aggregate
                .data
                .get("perKind")
                .and_then(|v| v.get("code"))
                .and_then(|v| v.as_str())
                != Some(code_verdict)
                || (code_verdict == "fail"
                    && aggregate.data.get("verdict").and_then(|v| v.as_str()) != Some("fail"))
            {
                return Err("aggregate code verdict mismatch".into());
            }
        }
        let expected: Vec<_> = events
            .iter()
            .filter(|event| {
                event.seq > evaluation.seq
                    && event.seq < through
                    && event.kind == "gate_kind_result"
                    && event.gate_id == gate.id
            })
            .flat_map(|event| {
                event
                    .data
                    .get("evidence")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .cloned()
            })
            .collect();
        if aggregate.data.get("evidence").and_then(|v| v.as_array()) != Some(&expected) {
            return Err("aggregate evidence mismatch".into());
        }
    }
    Ok(())
}

pub(crate) fn validate_run(
    c: &crate::Coordinator,
    run: &str,
    events: &[crate::RunEvent],
) -> std::io::Result<()> {
    let has_code = events.iter().any(|event| {
        event.kind == "gate_kind_result"
            && event.data.get("kind").and_then(|v| v.as_str()) == Some("code")
    });
    let root = c.inner.root.join("runs");
    let snapshot = root.join(format!("{run}.snapshot.toml"));
    let has_aggregate = events
        .iter()
        .any(|event| event.kind == "gate_verdict" || event.kind == "human_input_requested");
    if !has_code && (!has_aggregate || !snapshot.exists()) {
        return Ok(());
    }
    let mission = crate::Mission::parse(&std::fs::read_to_string(snapshot)?)
        .map_err(std::io::Error::other)?;
    let bindings: crate::BindingsSnapshot = toml::from_str(&std::fs::read_to_string(
        root.join(format!("{run}.bindings.toml")),
    )?)
    .map_err(std::io::Error::other)?;
    validate_results(run, &mission, &bindings, events).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binding_and_canonical_result_match_archons_profile() {
        let gate = Gate {
            id: "gate_review".into(),
            check: "output_contains".into(),
            check_version: "1".into(),
            check_value: "hello".into(),
            ..Default::default()
        };
        let binding = CodeGateBinding::new("run_test", &gate).unwrap();
        assert_eq!(
            binding.profile_sha256,
            "1d73e0d78d608937b9ec85ae3fed4c84eed125a5505cb4e0e3d2101864e710b5"
        );
        assert_eq!(
            binding.evaluator_bundle_sha256,
            "d41146ea519371f0c2fc76ad849ecd382553c681a881198f55f527580be6c830"
        );
        assert_eq!(
            binding.policy_sha256,
            "0919eaf230027f10e2c75bef1d04bf81924310d272b6abff8319efc890a3359c"
        );
        assert_eq!(
            binding.determinism_policy_sha256,
            "2adbf0611f766c47658fca6c02305b8a56926e492c3734754a03616adde711d5"
        );
        let result = crate::evaluate_code_check(&gate, "hello").unwrap();
        assert_eq!(
            binding.result_data("hello", &result).unwrap()["resultSha256"],
            "d55a83d3b0dbd57234e432fe0c73d0b44aad0bbe8d0678e93c4289c5443a126c"
        );
        let mut tampered = binding.clone();
        tampered.max_operations += 1;
        assert!(tampered.validate("run_test", &gate).is_err());
    }
}

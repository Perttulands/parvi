//! The code checks a gate can run: pure, in-process profiles pinned by name
//! and exact version.

pub struct CodeCheck {
    pub id: &'static str,
    pub version: &'static str,
    /// What the check's one parameter, `checkValue`, holds.
    pub parameter: &'static str,
}

pub const CODE_CHECKS: &[CodeCheck] = &[
    CodeCheck {
        id: "output_absent",
        version: "1",
        parameter: "Forbidden text",
    },
    CodeCheck {
        id: "output_contains",
        version: "1",
        parameter: "Required text",
    },
];

/// Evaluate a gate against its routed input without changing the ledger.
pub trait GateEvaluator: Send + Sync {
    fn evaluate(
        &self,
        gate: &parvi_mission_model::Gate,
        input: &crate::RoutedInput,
    ) -> Result<crate::JudgeVerdict, crate::Error>;
}

pub struct CodeGateEvaluator;
impl GateEvaluator for CodeGateEvaluator {
    fn evaluate(
        &self,
        gate: &parvi_mission_model::Gate,
        input: &crate::RoutedInput,
    ) -> Result<crate::JudgeVerdict, crate::Error> {
        evaluate_code_check(gate, &input.text)
    }
}

/// The registered check with this exact name and version.
pub fn code_check(id: &str, version: &str) -> Option<&'static CodeCheck> {
    (CODE_CHECKS.iter()).find(|check| check.id == id.trim() && check.version == version.trim())
}

/// Every registered check as `name@version`, sorted.
pub fn code_check_names() -> Vec<String> {
    CODE_CHECKS
        .iter()
        .map(|check| format!("{}@{}", check.id, check.version))
        .collect()
}

/// Evaluate a registered profile against the exact routed text.
pub fn evaluate_code_check(
    gate: &parvi_mission_model::Gate,
    output: &str,
) -> Result<crate::JudgeVerdict, crate::Error> {
    let check = gate.check.trim();
    if code_check(check, &gate.check_version).is_none() || gate.check_value.trim().is_empty() {
        return Err(crate::Error::new(
            "invalid_code_check",
            format!("gate {:?} has an invalid code check", gate.id),
        ));
    }
    if output.len() > 256 * 1024 || output.len().saturating_mul(gate.check_value.len()) > 512 * 1024
    {
        return Err(crate::Error::new(
            "evaluator_budget_exceeded",
            format!("gate {:?} evaluator exhausted its input budget", gate.id),
        ));
    }
    let contains = output.contains(&gate.check_value);
    let passed = if check == "output_contains" {
        contains
    } else {
        !contains
    };
    let detail = if contains && check == "output_absent" {
        "unexpectedly contains"
    } else if contains {
        "contains"
    } else {
        "does not contain"
    };
    let reason = format!("check {check}: output {detail} {:?}", gate.check_value);
    Ok(crate::JudgeVerdict {
        verdict: if passed { "pass" } else { "fail" }.into(),
        evidence: vec![reason.clone()],
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::evaluate_code_check;
    use parvi_mission_model::Gate;

    #[test]
    fn checks_the_exact_output_with_bounded_input() {
        let mut gate = Gate {
            id: "review".into(),
            check: "output_contains".into(),
            check_version: "1".into(),
            check_value: "PASS".into(),
            ..Default::default()
        };
        assert_eq!(
            evaluate_code_check(&gate, "PASS\n").unwrap().verdict,
            "pass"
        );
        assert_eq!(evaluate_code_check(&gate, "pass").unwrap().verdict, "fail");
        gate.check = "output_absent".into();
        assert_eq!(evaluate_code_check(&gate, "PASS").unwrap().verdict, "fail");
        assert_eq!(evaluate_code_check(&gate, "ok").unwrap().verdict, "pass");
        assert!(evaluate_code_check(&gate, &"x".repeat(256 * 1024 + 1)).is_err());
        gate.check_version = "2".into();
        assert!(evaluate_code_check(&gate, "ok").is_err());
    }
}

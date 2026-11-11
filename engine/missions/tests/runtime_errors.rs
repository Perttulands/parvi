use parvi_missions::{Error, Harnesses, Ledger, parse_verdict};

fn assert_error(error: Error, code: &str) {
    assert_eq!(error.code, code);
    assert!(!error.message.is_empty());
    let value = serde_json::to_value(error).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 2);
    assert_eq!(value["code"], code);
    assert!(
        value["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );
}

#[test]
fn runtime_errors_use_the_shared_shape() {
    let missing = std::env::temp_dir()
        .join(format!("parvi-error-test-{}", std::process::id()))
        .join("missing.ndjson");
    assert_error(
        Ledger::new(missing, "run_test").read().unwrap_err(),
        "not_found",
    );
    assert_error(parse_verdict("no verdict").unwrap_err(), "invalid_contract");
    assert_error(
        Harnesses::default()
            .check("slot", "unknown", "", "low")
            .unwrap_err(),
        "invalid_contract",
    );
}

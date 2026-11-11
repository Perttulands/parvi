//! Strict parsing of the final judge's fenced verdict.
use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JudgeVerdict {
    pub verdict: String,
    pub reason: String,
    pub evidence: Vec<String>,
}

impl<'de> Deserialize<'de> for JudgeVerdict {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = JudgeVerdict;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a judge verdict object")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                use serde::de::Error;
                let (mut verdict, mut reason, mut evidence) = (None, None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "verdict" if verdict.is_none() => {
                            verdict = Some(map.next_value::<String>()?)
                        }
                        "reason" if reason.is_none() => reason = Some(map.next_value::<String>()?),
                        "evidence" if evidence.is_none() => {
                            evidence = Some(map.next_value::<Vec<String>>()?)
                        }
                        "verdict" | "reason" | "evidence" => {
                            return Err(M::Error::custom(format!(
                                "judge verdict repeats key {key:?}"
                            )));
                        }
                        _ => {
                            return Err(M::Error::custom(format!(
                                "judge verdict has extra key {key:?}"
                            )));
                        }
                    }
                }
                let verdict = verdict.ok_or_else(|| M::Error::missing_field("verdict"))?;
                if verdict != "pass" && verdict != "fail" {
                    return Err(M::Error::custom(
                        "judge verdict must be exactly pass or fail",
                    ));
                }
                Ok(JudgeVerdict {
                    verdict,
                    reason: reason.ok_or_else(|| M::Error::missing_field("reason"))?,
                    evidence: evidence.ok_or_else(|| M::Error::missing_field("evidence"))?,
                })
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

pub fn parse_verdict(text: &str) -> Result<JudgeVerdict, crate::Error> {
    (|| -> Result<JudgeVerdict, String> {
        let marker = format!("```{}", crate::contract_names::VERDICT);
        let mut body = String::new();
        let (mut blocks, mut inside, mut closed) = (0, false, false);
        for line in text.lines().map(str::trim) {
            if line.starts_with(&marker) {
                blocks += 1;
                if line != marker || inside || blocks != 1 {
                    return Err(format!(
                        "expected exactly one {} block",
                        crate::contract_names::VERDICT
                    ));
                }
                inside = true;
            } else if inside && line == "```" {
                inside = false;
                closed = true;
            } else if inside {
                body.push_str(line);
                body.push('\n');
            }
        }
        if blocks != 1 || inside || !closed {
            return Err(format!(
                "missing or unterminated {} block",
                crate::contract_names::VERDICT
            ));
        }
        serde_json::from_str(&body).map_err(|e| e.to_string())
    })()
    .map_err(|message| crate::Error::new("invalid_contract", message))
}

#[cfg(test)]
mod tests {
    use super::parse_verdict;
    #[test]
    fn validates_the_whole_contract() {
        let wrap = |body: &str| format!("```parvi-verdict\n{body}\n```");
        assert_eq!(
            parse_verdict(&wrap(r#"{"verdict":"pass","reason":"ok","evidence":[]}"#))
                .unwrap()
                .verdict,
            "pass"
        );
        for body in [
            r#"{"verdict":"pass","reason":"ok"}"#,
            r#"{"verdict":"pass","verdict":"fail","reason":"ok","evidence":[]}"#,
            r#"{"verdict":"pass","reason":"ok","evidence":[null]}"#,
            r#"{"verdict":"pass","reason":"ok","evidence":[],"extra":1}"#,
        ] {
            assert!(parse_verdict(&wrap(body)).is_err(), "{body}");
        }
        assert!(parse_verdict(&format!("{}\n{}", wrap("{}"), wrap("{}"))).is_err());
    }
}

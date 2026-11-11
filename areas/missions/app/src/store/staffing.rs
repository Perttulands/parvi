//! Clamp slot choices to the catalog published by the node's role library.
use parvi_agents_contract::roles::Roles;

const EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultra"];
fn rank(e: &str) -> usize {
    EFFORTS
        .iter()
        .position(|item| *item == e)
        .unwrap_or(EFFORTS.len())
}
fn models<'a>(catalog: &'a Roles, harness: &str) -> &'a [serde_json::Value] {
    catalog
        .harnesses
        .iter()
        .find(|h| h.id == harness)
        .map(|h| h.models.as_slice())
        .unwrap_or_default()
}
fn clamp(catalog: &Roles, staff: &mut [String; 4]) {
    let model = models(catalog, &staff[1])
        .iter()
        .find(|m| m["id"].as_str() == Some(staff[2].as_str()));
    let model_efforts: Vec<String> = model
        .and_then(|m| m["efforts"].as_array())
        .map(|v| {
            v.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let efforts = if model_efforts.is_empty() {
        catalog
            .harnesses
            .iter()
            .find(|h| h.id == staff[1])
            .map(|h| h.efforts.as_slice())
            .unwrap_or_default()
    } else {
        &model_efforts
    };
    if staff[3].is_empty() || efforts.is_empty() || efforts.contains(&staff[3]) {
        return;
    }
    let want = rank(&staff[3]);
    staff[3] = efforts
        .iter()
        .rfind(|e| rank(e) <= want)
        .or(efforts.first())
        .cloned()
        .unwrap_or_default();
}
pub fn pick_harness(catalog: &Roles, staff: &mut [String; 4], harness: &str) {
    staff[1] = harness.to_owned();
    if !models(catalog, harness)
        .iter()
        .any(|m| m["id"].as_str() == Some(staff[2].as_str()))
    {
        staff[2].clear();
    }
    clamp(catalog, staff);
}
pub fn pick_model(catalog: &Roles, staff: &mut [String; 4], model: &str) {
    staff[2] = model.to_owned();
    clamp(catalog, staff);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_harness_change_keeps_what_it_can() {
        let c: Roles = serde_json::from_str(r#"{"roles":[{"id":"planner","displayName":"Planner"}],"harnesses":[{"id":"claude-code","efforts":["low","medium","high","xhigh","max"],"models":[{"id":"opus"}]},{"id":"openai-codex","efforts":["low","medium","high","xhigh","max","ultra"],"models":[{"id":"gpt-6","efforts":["low","high"]}]}]}"#).unwrap();
        let mut s = [
            "planner".into(),
            "openai-codex".into(),
            "gpt-6".into(),
            "high".into(),
        ];
        pick_harness(&c, &mut s, "claude-code");
        assert_eq!(s[2], "");
        assert_eq!(s[3], "high");
        s[3] = "ultra".into();
        pick_harness(&c, &mut s, "claude-code");
        assert_eq!(s[3], "max");
        pick_harness(&c, &mut s, "openai-codex");
        pick_model(&c, &mut s, "gpt-6");
        assert_eq!(s[3], "high");
    }
}

//! Frozen seat settings and complete role sources for a run.
use crate::{Mission, RoleStore, Slot};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingsSnapshot {
    pub schema: u8,
    pub run_id: String,
    pub mission_id: String,
    pub mission_slug: String,
    pub mission_rev: i64,
    pub input_card_id: String,
    #[serde(default, rename = "binding")]
    pub bindings: Vec<RunBinding>,
    #[serde(default, rename = "gateBinding", skip_serializing_if = "Vec::is_empty")]
    pub gate_bindings: Vec<crate::CodeGateBinding>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunBinding {
    pub node_id: String,
    pub slot_id: String,
    pub agent_id: String,
    pub harness: String,
    pub session_stem: String,
    pub model: String,
    pub effort: String,
    #[serde(default)]
    pub card_path: String,
    #[serde(default)]
    pub card_toml: String,
    #[serde(default)]
    pub card_sha256: String,
}
impl BindingsSnapshot {
    pub fn capture(
        run_id: &str,
        mission: &Mission,
        input_card_id: &str,
        roles: &RoleStore,
    ) -> Result<Self, crate::Error> {
        (|| -> Result<Self, String> {
            let mut bindings = Vec::new();
            for formation in &mission.formations {
                for slot in &formation.slots {
                    if !slot.staffed() {
                        continue;
                    }
                    let card_toml = if slot.agent_id.is_empty() {
                        String::new()
                    } else {
                        let root = roles.dir().parent().unwrap_or(roles.dir());
                        let locks = crate::write::locks(root, &format!("role:{}", slot.agent_id));
                        let _role = locks.document.lock().unwrap_or_else(|e| e.into_inner());
                        roles.source(&slot.agent_id).map_err(|e| e.to_string())?
                    };
                    let card_sha256 = if card_toml.is_empty() {
                        String::new()
                    } else {
                        hash(&card_toml)
                    };
                    bindings.push(RunBinding {
                        node_id: formation.id.clone(),
                        slot_id: slot.id.clone(),
                        agent_id: slot.agent_id.clone(),
                        harness: slot.harness.clone(),
                        session_stem: if slot.agent_id.is_empty() {
                            slot.id.clone()
                        } else {
                            slot.agent_id.clone()
                        },
                        model: slot.model.clone(),
                        effort: slot.effort.clone(),
                        card_path: if slot.agent_id.is_empty() {
                            String::new()
                        } else {
                            roles
                                .dir()
                                .join(format!("{}.toml", slot.agent_id))
                                .to_string_lossy()
                                .into_owned()
                        },
                        card_toml,
                        card_sha256,
                    });
                }
            }
            let gate_bindings = mission
                .gates
                .iter()
                .filter(|gate| gate.kinds.iter().any(|kind| kind == "code"))
                .filter_map(|gate| crate::CodeGateBinding::new(run_id, gate).ok())
                .collect();
            Ok(Self {
                schema: 3,
                run_id: run_id.into(),
                mission_id: mission.id.clone(),
                mission_slug: mission.slug.clone(),
                mission_rev: mission.rev,
                input_card_id: input_card_id.into(),
                bindings,
                gate_bindings,
            })
        })()
        .map_err(|message| crate::Error::new("invalid_contract", message))
    }
    pub fn gate_binding(
        &self,
        run_id: &str,
        mission: &Mission,
        gate: &crate::Gate,
    ) -> Result<&crate::CodeGateBinding, crate::Error> {
        (|| -> Result<&crate::CodeGateBinding, String> {
            if self.schema != 3
                || self.run_id != run_id
                || self.mission_id != mission.id
                || self.mission_slug != mission.slug
                || self.mission_rev != mission.rev
            {
                return Err("code gate snapshot identity mismatch".into());
            }
            let mut matches = self
                .gate_bindings
                .iter()
                .filter(|binding| binding.gate_id == gate.id);
            let binding = matches
                .next()
                .ok_or_else(|| "code gate binding missing".to_string())?;
            if matches.next().is_some() {
                return Err("duplicate code gate binding".into());
            }
            binding
                .validate(run_id, gate)
                .map_err(|error| error.message)?;
            Ok(binding)
        })()
        .map_err(|message| crate::Error::new("invalid_contract", message))
    }
    /// A mismatched or altered binding must never be used to launch a seat.
    pub fn binding(
        &self,
        mission: &Mission,
        run_id: &str,
        input_card_id: &str,
        node_id: &str,
        slot: &Slot,
    ) -> Result<&RunBinding, crate::Error> {
        let invalid = || {
            crate::Error::new(
                "persona_snapshot_invalid",
                "persona snapshot does not match the requested seat",
            )
        };
        if self.schema != 3
            || self.run_id != run_id
            || self.mission_id != mission.id
            || self.mission_slug != mission.slug
            || self.mission_rev != mission.rev
            || self.input_card_id != input_card_id
        {
            return Err(invalid());
        }
        let mut matches = self
            .bindings
            .iter()
            .filter(|binding| binding.node_id == node_id && binding.slot_id == slot.id);
        let Some(binding) = matches.next() else {
            return Err(invalid());
        };
        if matches.next().is_some()
            || binding.agent_id != slot.agent_id
            || binding.harness != slot.harness
            || binding.session_stem.as_str()
                != if slot.agent_id.is_empty() {
                    slot.id.as_str()
                } else {
                    slot.agent_id.as_str()
                }
            || binding.model != slot.model
            || binding.effort != slot.effort
            || binding.effort.is_empty()
            || (!binding.agent_id.is_empty()
                && (binding.card_toml.is_empty()
                    || hash(&binding.card_toml) != binding.card_sha256
                    || crate::roles::parse_card(&binding.agent_id, &binding.card_toml).is_err()))
        {
            return Err(invalid());
        }
        Ok(binding)
    }
}
fn hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}

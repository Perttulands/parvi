//! Devices coming and going, as the status line tells them. A device is
//! away when none of its environments answers.

use crate::settings::Place;
use std::collections::{BTreeMap, HashSet};

/// Devices whose last answering environment stopped answering, and
/// devices none of whose environments answered before and one does now,
/// given the nodes heard before and now: (gone, came), each by name once.
pub fn changed(
    places: &BTreeMap<String, Place>,
    before: &HashSet<String>,
    now: &HashSet<String>,
) -> (Vec<String>, Vec<String>) {
    let live = |heard: &HashSet<String>, device: &str| {
        places
            .iter()
            .any(|(n, p)| p.device == device && heard.contains(n))
    };
    let devices: std::collections::BTreeSet<&str> =
        places.values().map(|p| p.device.as_str()).collect();
    let (mut gone, mut came) = (vec![], vec![]);
    for d in devices {
        match (live(before, d), live(now, d)) {
            (true, false) => gone.push(d.to_string()),
            (false, true) => came.push(d.to_string()),
            _ => {}
        }
    }
    (gone, came)
}

/// What the app says of a device that went away, `since` its clock time.
pub fn away_line(device: &str, since: Option<&str>) -> String {
    let since = since.map(|at| format!(" since {at}")).unwrap_or_default();
    format!("{device} offline{since} · its sessions resume when it returns")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(device: &str, environment: &str) -> Place {
        Place {
            device: device.into(),
            environment: environment.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_device_goes_when_its_last_environment_does_and_comes_with_its_first() {
        let places = BTreeMap::from([
            ("rig-win".to_string(), place("rig", "windows")),
            ("rig-wsl".to_string(), place("rig", "wsl")),
            ("vps".to_string(), place("vps", "linux")),
        ]);
        let heard =
            |nodes: &[&str]| -> HashSet<String> { nodes.iter().map(|n| n.to_string()).collect() };
        let all = heard(&["rig-win", "rig-wsl", "vps"]);
        let none: (Vec<String>, Vec<String>) = (vec![], vec![]);
        assert_eq!(
            changed(&places, &all, &heard(&["rig-wsl", "vps"])),
            none,
            "one environment of a device going is not the device going"
        );
        assert_eq!(
            changed(&places, &heard(&["rig-wsl", "vps"]), &heard(&["rig-wsl"])),
            (vec!["vps".to_string()], vec![])
        );
        assert_eq!(
            changed(&places, &heard(&["rig-wsl"]), &heard(&[])),
            (vec!["rig".to_string()], vec![])
        );
        assert_eq!(
            changed(&places, &heard(&[]), &all),
            (vec![], vec!["rig".to_string(), "vps".to_string()])
        );
    }

    #[test]
    fn an_away_line_names_the_device_and_since_when() {
        assert_eq!(
            away_line("vps", Some("14:02")),
            "vps offline since 14:02 · its sessions resume when it returns"
        );
        assert_eq!(
            away_line("vps", None),
            "vps offline · its sessions resume when it returns"
        );
    }
}

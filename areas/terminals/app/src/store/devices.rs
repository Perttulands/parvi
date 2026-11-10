//! The rail's first two levels: each device, then each of its environments
//! (one node each), with the sessions it listed. A node that does not answer
//! keeps its place and its last list, so nothing moves when it comes back.

use super::settings::Kept;
use parvi_app_kit::settings::Place;
use parvi_terminals_contract::Session;
use std::collections::BTreeMap;

#[derive(Debug, PartialEq)]
pub struct NodeRows {
    pub node: String,
    pub environment: String,
    /// The node answers now.
    pub live: bool,
    pub sessions: Vec<Session>,
}

#[derive(Debug, PartialEq)]
pub struct DeviceRows {
    pub device: String,
    /// Some environment of it answers.
    pub live: bool,
    pub nodes: Vec<NodeRows>,
}

/// What the app hears from a node now: whether it answers, and its
/// sessions if it has listed them on this link.
pub type Heard<'a> = (bool, Option<&'a [Session]>);

/// Whether a device has anything to show for a search. With no search,
/// retain empty and offline devices so their status remains visible.
pub fn visible_in_search(device: &DeviceRows, query: &str) -> bool {
    query.trim().is_empty()
        || device
            .nodes
            .iter()
            .any(|node| !super::sessions::search(&node.sessions, query).is_empty())
}

/// Every remembered node, by device: the serving node's device first, then
/// the others by name; environments by name. A node not heard from now
/// shows the sessions it listed last (`kept`).
pub fn devices<'a>(
    places: &BTreeMap<String, Place>,
    kept: &BTreeMap<String, Vec<Kept>>,
    here: Option<&str>,
    heard: impl Fn(&str) -> Heard<'a>,
) -> Vec<DeviceRows> {
    let mut out: Vec<DeviceRows> = Vec::new();
    for (node, place) in places {
        let (live, listed) = heard(node);
        let sessions = match listed {
            Some(list) => list.to_vec(),
            None => kept
                .get(node)
                .into_iter()
                .flatten()
                .map(|k| Session {
                    node: node.clone(),
                    name: k.name.clone(),
                    user: (!k.user.is_empty()).then(|| k.user.clone()),
                    ..Default::default()
                })
                .collect(),
        };
        let rows = NodeRows {
            node: node.clone(),
            environment: place.environment.clone(),
            live,
            sessions,
        };
        match out.iter_mut().find(|d| d.device == place.device) {
            Some(d) => {
                d.live |= live;
                d.nodes.push(rows);
            }
            None => out.push(DeviceRows {
                device: place.device.clone(),
                live,
                nodes: vec![rows],
            }),
        }
    }
    let home = here.and_then(|h| places.get(h)).map(|p| p.device.clone());
    out.sort_by(|a, b| {
        (Some(&b.device) == home.as_ref(), &a.device)
            .cmp(&(Some(&a.device) == home.as_ref(), &b.device))
    });
    for d in &mut out {
        d.nodes.sort_by(|a, b| a.environment.cmp(&b.environment));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Remembered nodes, each (node, device, environment, sessions kept).
    type Fixture<'a> = &'a [(&'a str, &'a str, &'a str, &'a [&'a str])];

    fn fleet(nodes: Fixture) -> (BTreeMap<String, Place>, BTreeMap<String, Vec<Kept>>) {
        let places = nodes
            .iter()
            .map(|(node, device, environment, _)| {
                let place = Place {
                    device: device.to_string(),
                    environment: environment.to_string(),
                    ..Default::default()
                };
                (node.to_string(), place)
            })
            .collect();
        let kept = nodes
            .iter()
            .map(|(node, .., names)| {
                let kept = names
                    .iter()
                    .map(|n| Kept {
                        name: n.to_string(),
                        user: String::new(),
                    })
                    .collect();
                (node.to_string(), kept)
            })
            .collect();
        (places, kept)
    }

    #[test]
    fn an_away_device_keeps_its_place_and_last_sessions() {
        let (places, kept) = fleet(&[
            ("a-win", "rig", "windows", &["pwsh"]),
            ("a-wsl", "rig", "wsl", &["claude"]),
            ("b", "devbox", "wsl", &["stale"]),
        ]);
        let live = [Session {
            node: "b".into(),
            name: "shell".into(),
            ..Default::default()
        }];
        let heard = |n: &str| -> Heard<'_> {
            if n == "b" {
                (true, Some(&live[..]))
            } else {
                (false, None)
            }
        };
        let d = devices(&places, &kept, Some("b"), heard);
        let names = |d: &DeviceRows| -> Vec<(String, Vec<String>)> {
            d.nodes
                .iter()
                .map(|n| {
                    (
                        n.environment.clone(),
                        n.sessions.iter().map(|s| s.name.clone()).collect(),
                    )
                })
                .collect()
        };
        assert_eq!(
            d.iter()
                .map(|d| (d.device.as_str(), d.live))
                .collect::<Vec<_>>(),
            [("devbox", true), ("rig", false)],
            "the serving node's device first"
        );
        assert_eq!(
            names(&d[0]),
            [("wsl".to_string(), vec!["shell".to_string()])]
        );
        assert_eq!(
            names(&d[1]),
            [
                ("windows".to_string(), vec!["pwsh".to_string()]),
                ("wsl".to_string(), vec!["claude".to_string()])
            ],
            "an away device lists what it listed last"
        );
        assert!(
            d[1].nodes
                .iter()
                .all(|n| !n.live && n.sessions[0].node == n.node)
        );
    }

    #[test]
    fn search_hides_devices_without_matches_but_keeps_empty_devices_when_cleared() {
        let (places, kept) = fleet(&[
            ("win", "rig", "windows", &["pwsh"]),
            ("wsl", "rig", "wsl", &["claude-2"]),
            ("vps", "vps", "linux", &["shell"]),
            ("empty", "empty", "linux", &[]),
        ]);
        let devices = devices(&places, &kept, None, |_| (false, None));
        let visible = |query: &str| {
            devices
                .iter()
                .filter(|d| visible_in_search(d, query))
                .map(|d| d.device.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(visible("CLAUDE 2"), ["rig"]);
        assert_eq!(visible("missing"), Vec::<&str>::new());
        assert_eq!(visible("  "), ["empty", "rig", "vps"]);
    }
}

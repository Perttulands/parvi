//! Which node the app enters the fleet through (ARCHITECTURE.md, Clients and
//! the serving node). The configured node serves while it answers. When its
//! link has missed [`MISSES`] dials in a row, the app dials another node it
//! last saw directly, the same device's other environment first, then nodes
//! last listed online. While away it keeps the configured node's own link
//! redialling with backoff, and goes home as soon as that answers.

/// Dials missed in a row before a serving link counts as gone: the drop
/// itself and the first redial, about a second later.
pub const MISSES: u32 = 2;

/// A node the app could serve through: one it last saw, with the address a
/// client dials it at.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub node: String,
    pub device: String,
    pub address: String,
    /// Its serving node listed it online last.
    pub online: bool,
}

/// Where the app stands now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Situation<'a> {
    /// The configured node's id and device, once it has said hello.
    pub home: Option<(&'a str, &'a str)>,
    /// The configured node's own link answers now.
    pub home_open: bool,
    /// Dials the configured node's link has missed in a row.
    pub home_misses: u32,
    /// The node serving now, when it is not the configured one.
    pub away: Option<&'a str>,
    /// Dials the away serving link has missed in a row.
    pub away_misses: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Move {
    Stay,
    /// Serve through this node, dialled at its address.
    Through {
        node: String,
        address: String,
    },
    /// The configured node answers again: serve through it.
    Home,
}

/// What to do now, given the nodes last seen.
pub fn decide(s: &Situation, candidates: &[Candidate]) -> Move {
    match s.away {
        Some(_) if s.home_open => Move::Home,
        Some(_) if s.away_misses < MISSES => Move::Stay,
        None if s.home_misses < MISSES => Move::Stay,
        _ => next(s, candidates),
    }
}

/// The candidate to try after the one serving now (or the first), in order
/// of preference; `Stay` when there is no other.
fn next(s: &Situation, candidates: &[Candidate]) -> Move {
    let home = s.home.map(|(node, _)| node);
    let device = s.home.map(|(_, device)| device);
    let mut order: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| !c.address.is_empty() && Some(c.node.as_str()) != home)
        .collect();
    order.sort_by_key(|c| (Some(c.device.as_str()) != device, !c.online));
    let after = s
        .away
        .and_then(|a| order.iter().position(|c| c.node == a))
        .map_or(0, |i| i + 1);
    order
        .iter()
        .cycle()
        .skip(after)
        .take(order.len())
        .find(|c| Some(c.node.as_str()) != s.away)
        .map_or(Move::Stay, |c| Move::Through {
            node: c.node.clone(),
            address: c.address.clone(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(node: &str, device: &str, online: bool) -> Candidate {
        Candidate {
            node: node.into(),
            device: device.into(),
            address: format!("100.64.0.9:{}", node.len()),
            online,
        }
    }

    fn through(c: &Candidate) -> Move {
        Move::Through {
            node: c.node.clone(),
            address: c.address.clone(),
        }
    }

    const HOME: Option<(&str, &str)> = Some(("wsl", "rig"));

    #[test]
    fn the_configured_node_serves_until_it_misses_a_redial() {
        let fleet = [node("win", "rig", true)];
        let lost = |misses| Situation {
            home: HOME,
            home_misses: misses,
            ..Default::default()
        };
        assert_eq!(decide(&lost(0), &fleet), Move::Stay);
        assert_eq!(decide(&lost(1), &fleet), Move::Stay, "a blip is not gone");
        assert_eq!(decide(&lost(MISSES), &fleet), through(&fleet[0]));
    }

    #[test]
    fn the_same_devices_other_environment_comes_first_then_nodes_online() {
        let fleet = [
            node("vps", "vps", true),
            node("laptop", "laptop", false),
            node("win", "rig", false),
            node("wsl", "rig", true),
        ];
        let gone = Situation {
            home: HOME,
            home_misses: MISSES,
            ..Default::default()
        };
        // Home itself is never a way in to home.
        assert_eq!(decide(&gone, &fleet), through(&fleet[2]));
        // Before home ever said hello, nodes last listed online lead.
        let cold = Situation { home: None, ..gone };
        assert_eq!(decide(&cold, &fleet), through(&fleet[0]));
    }

    #[test]
    fn a_node_never_heard_at_an_address_is_no_way_in() {
        let mut win = node("win", "rig", true);
        win.address.clear();
        let gone = Situation {
            home: HOME,
            home_misses: MISSES,
            ..Default::default()
        };
        assert_eq!(decide(&gone, &[win]), Move::Stay);
        assert_eq!(decide(&gone, &[]), Move::Stay);
    }

    #[test]
    fn an_away_node_that_goes_too_gives_way_to_the_next() {
        let fleet = [
            node("win", "rig", true),
            node("vps", "vps", true),
            node("laptop", "laptop", false),
        ];
        let away = |at, misses| Situation {
            home: HOME,
            home_misses: 5,
            away: Some(at),
            away_misses: misses,
            ..Default::default()
        };
        assert_eq!(decide(&away("win", 1), &fleet), Move::Stay);
        assert_eq!(decide(&away("win", MISSES), &fleet), through(&fleet[1]));
        assert_eq!(decide(&away("vps", MISSES), &fleet), through(&fleet[2]));
        assert_eq!(decide(&away("laptop", MISSES), &fleet), through(&fleet[0]));
        // The only way in keeps redialling.
        assert_eq!(decide(&away("win", MISSES), &fleet[..1]), Move::Stay);
    }

    #[test]
    fn home_answering_again_ends_the_detour() {
        let fleet = [node("win", "rig", true)];
        let back = Situation {
            home: HOME,
            home_open: true,
            away: Some("win"),
            away_misses: MISSES,
            ..Default::default()
        };
        assert_eq!(decide(&back, &fleet), Move::Home);
        let home = Situation {
            away: None,
            home_misses: 0,
            ..back
        };
        assert_eq!(decide(&home, &fleet), Move::Stay);
    }
}

//! Where a step runs (#124, #231): a formation's `device` names an
//! environment as `<device>/<environment>`, exactly as the node validates and
//! places it, and blank runs it on the run's coordinator.

/// One environment the app knows of: a node and where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Env {
    pub node: String,
    pub device: String,
    pub environment: String,
    pub online: bool,
}

/// Where a step runs, as the itinerary draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub device: String,
    /// Blank when the step names a device with several environments.
    pub environment: String,
    /// The node, when the step's place is one the app knows.
    pub node: Option<String>,
    pub online: bool,
    /// The step names its place; otherwise it runs on the coordinator.
    pub named: bool,
}

impl Place {
    fn of(env: &Env, named: bool) -> Self {
        Self {
            device: env.device.clone(),
            environment: env.environment.clone(),
            node: Some(env.node.clone()),
            online: env.online,
            named,
        }
    }

    /// Two steps on the same environment: no hop between them.
    pub fn same(&self, other: &Place) -> bool {
        self.device == other.device && self.environment == other.environment
    }
}

/// Where a step with this `device` runs, for a run coordinated by `here`.
pub fn resolve(device: &str, here: &str, fleet: &[Env]) -> Place {
    let device = device.trim();
    if device.is_empty() {
        return match fleet.iter().find(|e| e.node == here) {
            Some(e) => Place::of(e, false),
            None => Place {
                device: String::new(),
                environment: String::new(),
                node: Some(here.to_string()),
                online: false,
                named: false,
            },
        };
    }
    // The node's rule: exactly `<device>/<environment>`, as validation and
    // placement read it. Anything else is a place no node will take.
    let (name, environment) = device.split_once('/').unwrap_or((device, ""));
    match fleet
        .iter()
        .find(|e| e.device == name && e.environment == environment)
    {
        Some(e) => Place::of(e, true),
        None => Place {
            device: name.to_string(),
            environment: environment.to_string(),
            node: None,
            online: false,
            named: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fleet() -> Vec<Env> {
        let env = |node: &str, device: &str, environment: &str, online| Env {
            node: node.into(),
            device: device.into(),
            environment: environment.into(),
            online,
        };
        vec![
            env("a1", "desktop", "wsl", true),
            env("a2", "desktop", "windows", true),
            env("b1", "notebook", "wsl", true),
            env("c1", "server", "linux", false),
        ]
    }

    #[test]
    fn a_step_is_placed_exactly_as_the_node_places_it() {
        let f = fleet();
        let p = resolve("", "b1", &f);
        assert_eq!((p.device.as_str(), p.named), ("notebook", false));
        assert_eq!(
            resolve("desktop/windows", "b1", &f).node.as_deref(),
            Some("a2")
        );
        assert!(!resolve("server/linux", "b1", &f).online);
        // A step named on the coordinator's own environment makes no hop.
        assert!(resolve("desktop/wsl", "b1", &f).same(&resolve("", "a1", &f)));
        // Forms the node refuses are not taken for a place in reach.
        for loose in ["a2", "desktop", "desktop:wsl", "Desktop/wsl", "moon/wsl"] {
            let p = resolve(loose, "b1", &f);
            assert!(p.node.is_none() && !p.online, "{loose}");
        }
    }
}

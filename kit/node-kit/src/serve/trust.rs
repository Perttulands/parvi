//! The trust check every node makes on every HTTP request and socket
//! upgrade (ARCHITECTURE.md, Trust). `Host` must name this node: an IP
//! literal, `localhost`, or one of its own names, which defeats DNS
//! rebinding. A present `Origin` must name the same host and port as `Host`,
//! so a page from another site cannot drive the node. Clients that send no
//! `Origin` are not browsers and pass.
//!
//! Tailscale serves learned MagicDNS names over HTTPS on port 443; direct
//! connections to the node use HTTP.

use std::sync::{Arc, RwLock};

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::authority::{Authority, Host};
use crate::identity::hostname;

/// The names this node answers to, besides IP literals and `localhost`:
/// those it was given, and the MagicDNS names discovery learns.
#[derive(Debug, Default)]
pub struct Trust {
    names: Vec<String>,
    tailnet: RwLock<Vec<String>>,
}

impl Trust {
    /// Trusts this machine's hostname and each of `names`.
    pub fn local(names: impl IntoIterator<Item = String>) -> Trust {
        Trust::new(hostname().into_iter().chain(names))
    }

    pub fn new(names: impl IntoIterator<Item = String>) -> Trust {
        Trust {
            names: host_names(names),
            tailnet: RwLock::default(),
        }
    }

    /// Trusts these tailnet names from now on, instead of those learnt before.
    pub fn learn(&self, names: impl IntoIterator<Item = String>) {
        *self.tailnet.write().unwrap() = host_names(names);
    }

    /// Why the request is refused, or `None` when it may proceed.
    pub fn refusal(&self, headers: &HeaderMap) -> Option<&'static str> {
        let Some(host) = text(headers, header::HOST).and_then(Authority::parse) else {
            return Some("a Host header naming this node is required");
        };
        if !self.names_this_node(&host.host) {
            return Some("the Host header does not name this node");
        }
        match headers.get(header::ORIGIN) {
            None => None,
            Some(origin) => {
                let same = origin
                    .to_str()
                    .ok()
                    .and_then(Authority::parse_origin)
                    .is_some_and(|origin| {
                        let https = matches!(&host.host, Host::Name(name) if self.tailnet.read().unwrap().contains(name))
                            && matches!(host.port, None | Some(443));
                        let scheme = if https { "https://" } else { "http://" };
                        text(headers, header::ORIGIN).is_some_and(|value| value.starts_with(scheme))
                            && same_origin(&origin, &host, if https { 443 } else { 80 })
                    });
                (!same).then_some("the Origin header is not this node's own")
            }
        }
    }

    fn names_this_node(&self, host: &Host) -> bool {
        match host {
            Host::Ip(_) => true,
            Host::Name(name) => {
                name == "localhost"
                    || self.names.contains(name)
                    || self.tailnet.read().unwrap().contains(name)
            }
        }
    }
}

/// The names among `names`, normalised as a `Host` header's are.
fn host_names(names: impl IntoIterator<Item = String>) -> Vec<String> {
    names
        .into_iter()
        .filter_map(|name| match Authority::parse(&name)?.host {
            Host::Name(name) => Some(name),
            Host::Ip(_) => None,
        })
        .collect()
}

/// Middleware refusing every request [`Trust`] does not admit with 403.
pub async fn guard(State(trust): State<Arc<Trust>>, request: Request, next: Next) -> Response {
    match trust.refusal(request.headers()) {
        Some(reason) => (StatusCode::FORBIDDEN, reason).into_response(),
        None => next.run(request).await,
    }
}

/// An origin carries its port; a `Host` without one means the scheme's
/// default.
fn same_origin(origin: &Authority, host: &Authority, default_port: u16) -> bool {
    origin.host == host.host && origin.port == Some(host.port.unwrap_or(default_port))
}

fn text(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name)?.to_str().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(host: Option<&str>, origin: Option<&str>) -> Option<&'static str> {
        let mut headers = HeaderMap::new();
        if let Some(host) = host {
            headers.insert(header::HOST, host.parse().unwrap());
        }
        if let Some(origin) = origin {
            headers.insert(header::ORIGIN, origin.parse().unwrap());
        }
        let trust = Trust::new(["Laptop".to_owned()]);
        trust.learn(["laptop.tail1.ts.net".to_owned()]);
        trust.refusal(&headers)
    }

    #[test]
    fn own_hosts_pass() {
        for host in [
            "127.0.0.1:7300",
            "[::1]:7300",
            "100.64.0.1",
            "localhost:7300",
            "laptop",
            "LAPTOP.tail1.ts.net.",
        ] {
            assert_eq!(refusal(Some(host), None), None, "{host}");
        }
    }

    #[test]
    fn learnt_tailnet_names_pass_until_replaced() {
        let trust = Trust::new([]);
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "rig.tail1.ts.net".parse().unwrap());
        assert!(trust.refusal(&headers).is_some());
        trust.learn(["rig".to_owned(), "RIG.tail1.ts.net".to_owned()]);
        assert_eq!(trust.refusal(&headers), None);
        trust.learn(["rig2.tail1.ts.net".to_owned()]);
        assert!(trust.refusal(&headers).is_some());
    }

    #[test]
    fn rebinding_hosts_are_refused() {
        for host in [
            "evil.example:7300",
            "laptop.evil.example",
            "localhost.evil.example",
            "",
            "a b",
        ] {
            assert!(refusal(Some(host), None).is_some(), "{host}");
        }
        assert!(refusal(None, None).is_some());
        assert!(refusal(Some("evil.example:7300"), Some("http://evil.example:7300")).is_some());
    }

    #[test]
    fn own_origin_passes() {
        assert_eq!(
            refusal(Some("127.0.0.1:7300"), Some("http://127.0.0.1:7300")),
            None
        );
        assert_eq!(
            refusal(Some("localhost:5173"), Some("http://localhost:5173")),
            None
        );
        assert_eq!(
            refusal(
                Some("laptop.tail1.ts.net"),
                Some("https://laptop.tail1.ts.net")
            ),
            None
        );
        assert_eq!(refusal(Some("laptop"), Some("http://laptop")), None);
    }

    #[test]
    fn scheme_must_match_the_served_endpoint() {
        assert!(
            refusal(
                Some("laptop.tail1.ts.net"),
                Some("http://laptop.tail1.ts.net")
            )
            .is_some()
        );
        assert!(refusal(Some("localhost:7300"), Some("https://localhost:7300")).is_some());
        assert_eq!(
            refusal(
                Some("laptop.tail1.ts.net:7300"),
                Some("http://laptop.tail1.ts.net:7300")
            ),
            None
        );
        assert_eq!(
            refusal(
                Some("laptop.tail1.ts.net:443"),
                Some("https://laptop.tail1.ts.net")
            ),
            None
        );
    }

    #[test]
    fn foreign_origins_are_refused() {
        for origin in [
            "http://evil.example",
            "http://127.0.0.1:7301",
            "http://localhost:7300",
            "https://laptop.tail1.ts.net:8443",
            "null",
            "file://",
        ] {
            assert!(
                refusal(Some("127.0.0.1:7300"), Some(origin)).is_some(),
                "{origin}"
            );
        }
        assert!(refusal(Some("laptop"), Some("http://laptop:7300")).is_some());
    }
}

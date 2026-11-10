//! The host and port a `Host` or `Origin` header names.

use std::net::IpAddr;

/// A parsed host and optional port, the host lower-cased and without a
/// trailing dot, so equal names compare equal.
#[derive(Debug, PartialEq, Eq)]
pub struct Authority {
    pub host: Host,
    pub port: Option<u16>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Host {
    Ip(IpAddr),
    Name(String),
}

impl Authority {
    /// Parses a `Host` header value: `name`, `name:port`, `1.2.3.4:port` or
    /// `[::1]:port`.
    pub fn parse(text: &str) -> Option<Authority> {
        let (host, port) = match text.strip_prefix('[') {
            Some(rest) => {
                let (ip, after) = rest.split_once(']')?;
                let port = match after {
                    "" => None,
                    _ => Some(after.strip_prefix(':')?),
                };
                (Host::Ip(IpAddr::V6(ip.parse().ok()?)), port)
            }
            None => {
                let (host, port) = match text.split_once(':') {
                    Some((host, port)) => (host, Some(port)),
                    None => (text, None),
                };
                (host_named(host)?, port)
            }
        };
        let port = match port {
            Some(port) if port.bytes().all(|b| b.is_ascii_digit()) => Some(port.parse().ok()?),
            Some(_) => return None,
            None => None,
        };
        Some(Authority { host, port })
    }

    /// Parses an `Origin` header value, `http://` or `https://` followed by an
    /// authority, and gives it with its port made explicit.
    pub fn parse_origin(text: &str) -> Option<Authority> {
        let (scheme, rest) = text.split_once("://")?;
        let default_port = match scheme {
            "http" => 80,
            "https" => 443,
            _ => return None,
        };
        let mut origin = Authority::parse(rest)?;
        origin.port.get_or_insert(default_port);
        Some(origin)
    }
}

fn host_named(text: &str) -> Option<Host> {
    if let Ok(ip) = text.parse() {
        return Some(Host::Ip(IpAddr::V4(ip)));
    }
    let name = text.strip_suffix('.').unwrap_or(text);
    let valid = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_');
    valid.then(|| Host::Name(name.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(host: &str, port: Option<u16>) -> Authority {
        Authority {
            host: Host::Name(host.to_owned()),
            port,
        }
    }

    #[test]
    fn hosts() {
        assert_eq!(Authority::parse("Laptop."), Some(name("laptop", None)));
        assert_eq!(
            Authority::parse("laptop.tail1.ts.net:7300"),
            Some(name("laptop.tail1.ts.net", Some(7300)))
        );
        assert_eq!(
            Authority::parse("127.0.0.1:7300").map(|a| a.host),
            Some(Host::Ip("127.0.0.1".parse().unwrap()))
        );
        assert_eq!(
            Authority::parse("[::1]:7300"),
            Some(Authority {
                host: Host::Ip("::1".parse().unwrap()),
                port: Some(7300)
            })
        );
        for bad in [
            "", ":7300", "a:b", "a:7300:1", "::1", "[::1", "[::1]x", "a b", "a/b", "a:+1",
            "a:99999",
        ] {
            assert_eq!(Authority::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn origins() {
        assert_eq!(
            Authority::parse_origin("https://laptop.tail1.ts.net"),
            Some(name("laptop.tail1.ts.net", Some(443)))
        );
        assert_eq!(
            Authority::parse_origin("http://localhost:5173"),
            Some(name("localhost", Some(5173)))
        );
        for bad in ["null", "file://", "ws://localhost", "http://", "http://a/b"] {
            assert_eq!(Authority::parse_origin(bad), None, "{bad:?}");
        }
    }
}

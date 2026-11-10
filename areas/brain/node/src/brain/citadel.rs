//! Context Citadel's ranked search (`POST /v1/library/search`), which owns the
//! index, the ranking and which collections a search covers. Parvi keeps no
//! search of its own. Citadel trusts loopback callers that send no token, so
//! Parvi talks to a Citadel on its own machine over plain HTTP.

use parvi_brain_contract::Found;
use std::time::Duration;

use anyhow::{Result, bail};
use axum::http::{Request, StatusCode, header};
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use tokio::net::TcpStream;

use super::refusal::Refusal;

/// How many results a search asks for: Citadel's most.
const RESULTS: usize = 50;

/// How long one search may take, connecting included.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The most bytes read from one answer.
const ANSWER_LIMIT: usize = 2 << 20;

pub struct Citadel {
    /// `host:port`.
    authority: String,
    /// Any path the service is mounted under, without a trailing `/`.
    prefix: String,
}

#[derive(Deserialize)]
struct Answer {
    results: Vec<Found>,
}

impl Citadel {
    /// The Citadel at `url`, an `http://host[:port][/prefix]` URL.
    pub fn new(url: &str) -> Result<Citadel> {
        let Some(rest) = url.strip_prefix("http://") else {
            bail!("the Citadel URL {url:?} must start with http://");
        };
        let (authority, prefix) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        if authority.is_empty() || authority.contains('@') {
            bail!("the Citadel URL {url:?} names no host");
        }
        let authority = if authority
            .rsplit_once(':')
            .is_some_and(|(_, p)| !p.contains(']'))
        {
            authority.to_owned()
        } else {
            format!("{authority}:80")
        };
        Ok(Citadel {
            authority,
            prefix: prefix.trim_end_matches('/').to_owned(),
        })
    }

    /// Asks Citadel for the pages matching `query`, best first.
    pub async fn search(&self, query: &str) -> Result<Vec<Found>, Refusal> {
        let failed = |why: String| Refusal::new(StatusCode::BAD_GATEWAY, why);
        let (status, body) = tokio::time::timeout(TIMEOUT, self.post(query))
            .await
            .map_err(|_| failed("Citadel did not answer in time".to_owned()))?
            .map_err(|error| failed(format!("Citadel did not answer: {error:#}")))?;
        if status != StatusCode::OK {
            return Err(failed(format!("Citadel refused the search ({status})")));
        }
        serde_json::from_slice::<Answer>(&body)
            .map(|answer| answer.results)
            .map_err(|_| failed("Citadel answered with something other than results".to_owned()))
    }

    async fn post(&self, query: &str) -> Result<(StatusCode, Vec<u8>)> {
        let stream = TcpStream::connect(&self.authority).await?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        tokio::spawn(connection);
        let body = serde_json::json!({ "query": query, "limit": RESULTS }).to_string();
        let request = Request::post(format!("{}/v1/library/search", self.prefix))
            .header(header::HOST, &self.authority)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Full::new(hyper::body::Bytes::from(body)))?;
        let response = sender.send_request(request).await?;
        let status = response.status();
        let body = Limited::new(response.into_body(), ANSWER_LIMIT)
            .collect()
            .await
            .map_err(|error| anyhow::anyhow!("reading the answer: {error}"))?;
        Ok((status, body.to_bytes().to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_url() {
        let citadel = Citadel::new("http://127.0.0.1:3000/").unwrap();
        assert_eq!(
            (citadel.authority.as_str(), citadel.prefix.as_str()),
            ("127.0.0.1:3000", "")
        );
        let citadel = Citadel::new("http://brain.local/citadel").unwrap();
        assert_eq!(
            (citadel.authority.as_str(), citadel.prefix.as_str()),
            ("brain.local:80", "/citadel")
        );
        assert_eq!(
            Citadel::new("http://[::1]:3000").unwrap().authority,
            "[::1]:3000"
        );
        assert_eq!(Citadel::new("http://[::1]").unwrap().authority, "[::1]:80");
        assert!(Citadel::new("https://brain.example").is_err());
        assert!(Citadel::new("http://").is_err());
    }
}

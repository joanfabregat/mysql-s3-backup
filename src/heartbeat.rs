//! The success ping for heartbeat monitors (UptimeRobot, Healthchecks.io,
//! Better Stack): one GET to `HEARTBEAT_URL` once every database is uploaded.
//! The monitor alerts when a ping does not arrive, so a failed backup sends
//! nothing.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use http_body_util::Empty;
use hyper::Uri;
use hyper::body::Bytes;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;

const ATTEMPTS: u32 = 3;
const TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_DELAY: Duration = Duration::from_secs(2);

/// Parses and checks `HEARTBEAT_URL`, so a typo fails before the dump rather
/// than after it.
pub fn parse_url(value: &str) -> Result<Uri, String> {
    let invalid = || format!("HEARTBEAT_URL must be an http:// or https:// URL, got '{value}'");
    let uri: Uri = value.parse().map_err(|_| invalid())?;
    match (uri.scheme_str(), uri.host()) {
        (Some("http" | "https"), Some(host)) if !host.is_empty() => Ok(uri),
        _ => Err(invalid()),
    }
}

async fn attempt(uri: &Uri) -> Result<()> {
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .context("loading the system CA certificates")?
        .https_or_http()
        .enable_http1()
        .build();
    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(https);
    let response = tokio::time::timeout(TIMEOUT, client.get(uri.clone()))
        .await
        .with_context(|| format!("no response within {}s", TIMEOUT.as_secs()))??;
    if !response.status().is_success() {
        bail!("HTTP {}", response.status());
    }
    Ok(())
}

/// Sends the ping, retrying a failure. Never fails the backup: the backup is
/// already stored, and a non-zero exit would make a Kubernetes Job dump
/// everything again over a monitoring problem. The monitor alerts on the
/// missing ping anyway. The URL is not logged, since it usually carries the
/// monitor's token.
pub async fn ping(uri: &Uri) {
    for n in 1..=ATTEMPTS {
        match attempt(uri).await {
            Ok(()) => {
                println!("Heartbeat sent.");
                return;
            }
            Err(e) => eprintln!("Warning: heartbeat attempt {n} of {ATTEMPTS} failed: {e:#}"),
        }
        if n < ATTEMPTS {
            tokio::time::sleep(RETRY_DELAY).await;
        }
    }
    eprintln!("Warning: the heartbeat was not delivered; the backup itself succeeded");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https() {
        let uri = parse_url("https://heartbeat.uptimerobot.com/m1-abc").unwrap();
        assert_eq!(uri.host(), Some("heartbeat.uptimerobot.com"));
        assert!(parse_url("http://127.0.0.1:8080/ping?x=1").is_ok());
    }

    #[test]
    fn rejects_other_values() {
        for value in [
            "heartbeat.uptimerobot.com/m1",
            "ftp://host/x",
            "https://",
            "not a url",
        ] {
            assert_eq!(
                parse_url(value).unwrap_err(),
                format!("HEARTBEAT_URL must be an http:// or https:// URL, got '{value}'"),
                "{value}"
            );
        }
    }
}

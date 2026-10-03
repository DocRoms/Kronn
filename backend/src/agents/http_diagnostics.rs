//! Safe HTTP failure metadata for persisted audit warnings. Provider bodies,
//! URLs and request arguments belong neither in this record nor in its summary.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

const PREFIX: &str = "kronn_http_failure:";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Failure {
    version: u8,
    http_status: Option<u16>,
}

pub(crate) fn record_failure(
    stderr: &Arc<Mutex<Vec<String>>>,
    status: Option<reqwest::StatusCode>,
) {
    let failure = Failure {
        version: 1,
        http_status: status.map(|s| s.as_u16()),
    };
    if let Ok(mut lines) = stderr.lock() {
        lines.retain(|line| !line.starts_with(PREFIX));
        if let Ok(json) = serde_json::to_string(&failure) {
            lines.push(format!("{PREFIX}{json}"));
        }
    }
}

/// A successful negotiated fallback must not leave a stale failure diagnostic.
pub(crate) fn clear_failure(stderr: &Arc<Mutex<Vec<String>>>) {
    if let Ok(mut lines) = stderr.lock() {
        lines.retain(|line| !line.starts_with(PREFIX));
    }
}

pub(crate) fn failure_summary(stderr: &[String]) -> Option<String> {
    stderr.iter().rev().find_map(|line| {
        let failure: Failure = serde_json::from_str(line.strip_prefix(PREFIX)?).ok()?;
        if failure.version != 1 {
            return None;
        }
        match failure.http_status {
            Some(429) => Some("HTTP provider returned 429 (rate limit or quota exhausted)".into()),
            Some(status @ 400..=599) => Some(format!("HTTP provider returned {status}")),
            None => Some("HTTP provider request failed before a response was received (connection or timeout)".into()),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_status_is_safe_and_a_successful_fallback_clears_it() {
        let stderr = Arc::new(Mutex::new(vec!["private provider response".into()]));
        record_failure(&stderr, Some(reqwest::StatusCode::TOO_MANY_REQUESTS));
        let summary = failure_summary(&stderr.lock().unwrap()).unwrap();
        assert!(summary.contains("429") && summary.contains("rate limit"));
        assert!(!summary.contains("private"));
        record_failure(&stderr, Some(reqwest::StatusCode::SERVICE_UNAVAILABLE));
        assert_eq!(
            failure_summary(&stderr.lock().unwrap()).unwrap(),
            "HTTP provider returned 503"
        );
        clear_failure(&stderr);
        assert!(failure_summary(&stderr.lock().unwrap()).is_none());
        assert_eq!(*stderr.lock().unwrap(), vec!["private provider response"]);
    }

    #[test]
    fn transport_failure_has_no_endpoint_or_raw_error() {
        let stderr = Arc::new(Mutex::new(Vec::new()));
        record_failure(&stderr, None);
        assert!(failure_summary(&stderr.lock().unwrap())
            .unwrap()
            .contains("connection or timeout"));
    }

    #[test]
    fn arbitrary_output_and_malformed_metadata_are_not_a_diagnostic() {
        for line in [
            "provider said 429: secret",
            "kronn_http_failure:not json",
            "kronn_http_failure:{\"version\":2,\"http_status\":429}",
            "kronn_http_failure:{\"version\":1,\"http_status\":200}",
            "kronn_http_failure:{\"version\":1,\"http_status\":429,\"body\":\"secret\"}",
        ] {
            assert!(failure_summary(&[line.into()]).is_none(), "{line}");
        }
        assert!(failure_summary(&[]).is_none());
    }
}

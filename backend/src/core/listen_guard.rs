//! A backend never starts on a port another process already serves.
//!
//! On macOS a listener on `127.0.0.1:<port>` coexists with one on
//! `*:<port>` (both set `SO_REUSEADDR`), and the more specific one wins every
//! local connection. A second Kronn (a worker's e2e run, a stray binary) would
//! silently take the UI, the MCP bridges and the workers' calls off the real
//! backend, so the bind is refused while anything answers on the port.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// `Err` naming the port when something already accepts connections on it.
pub fn refuse_if_port_served(host: &str, port: u16) -> Result<(), String> {
    // A wildcard bind is reached through loopback, like the UI reaches it.
    let probe_host = match host {
        "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
        other => other,
    };
    let targets: Vec<SocketAddr> = (probe_host, port)
        .to_socket_addrs()
        .map(Iterator::collect)
        .unwrap_or_default();
    if targets
        .iter()
        .any(|target| TcpStream::connect_timeout(target, PROBE_TIMEOUT).is_ok())
    {
        return Err(format!(
            "port {port} is already served on {probe_host}: another Kronn (or another \
             program) is running there. Stop it, or start this backend on another port."
        ));
    }
    Ok(())
}

/// A valid `KRONN_PORT` value, or `None` to keep the configured port.
pub fn port_override(value: Option<&str>) -> Option<u16> {
    value
        .map(str::trim)
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port > 0)
}

#[cfg(test)]
mod tests {
    use super::{port_override, refuse_if_port_served};

    #[test]
    fn only_a_real_port_overrides_the_configured_one() {
        assert_eq!(port_override(Some("23140")), Some(23140));
        assert_eq!(port_override(Some(" 23140 ")), Some(23140));
        for invalid in [None, Some(""), Some("0"), Some("70000"), Some("abc")] {
            assert_eq!(port_override(invalid), None, "{invalid:?}");
        }
    }

    #[test]
    fn a_served_port_is_refused_and_a_free_one_is_not() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let served = listener.local_addr().unwrap().port();
        let refused = refuse_if_port_served("127.0.0.1", served).unwrap_err();
        assert!(refused.contains(&served.to_string()), "{refused}");
        // A wildcard bind probes loopback, where a specific listener would win.
        assert!(refuse_if_port_served("0.0.0.0", served).is_err());

        drop(listener);
        let free = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        assert!(refuse_if_port_served("127.0.0.1", free).is_ok());
    }
}

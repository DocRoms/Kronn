//! Loopback port of the embedded backend.
//!
//! The webview's localStorage is scoped to `http://127.0.0.1:<port>`, so the
//! desktop keeps one port across launches: the value in `desktop-port.json`
//! (Kronn data directory) is reused when it can be bound, and a busy port
//! falls back to a free one for this launch only.

use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

pub const PORT_FILE: &str = "desktop-port.json";
/// Ports below this need privileges on Unix; a saved value under it is ignored.
const MIN_PORT: u16 = 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Persisted {
    Missing,
    Invalid,
    Port(u16),
}

impl Persisted {
    pub fn port(&self) -> Option<u16> {
        match self {
            Persisted::Port(port) => Some(*port),
            _ => None,
        }
    }
}

pub fn port_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PORT_FILE)
}

/// Accepts `{"port": 47315}` or a bare number, so the value is easy to set by hand.
pub fn parse_port(raw: &str) -> Option<u16> {
    let raw = raw.trim();
    let value = match raw.parse::<u32>() {
        Ok(number) => number,
        Err(_) => serde_json::from_str::<serde_json::Value>(raw)
            .ok()?
            .get("port")?
            .as_u64()
            .and_then(|port| u32::try_from(port).ok())?,
    };
    u16::try_from(value).ok().filter(|port| *port >= MIN_PORT)
}

pub fn read_persisted(path: &Path) -> Persisted {
    match std::fs::read_to_string(path) {
        Ok(raw) => parse_port(&raw).map_or(Persisted::Invalid, Persisted::Port),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Persisted::Missing,
        Err(_) => Persisted::Invalid,
    }
}

/// Atomic so a crash mid-write never leaves a truncated file behind.
pub fn write_persisted(path: &Path, port: u16) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{{\"port\": {port}}}\n"))?;
    std::fs::rename(&tmp, path)
}

pub struct ChosenPort {
    pub listener: TcpListener,
    pub port: u16,
    /// The saved port was bound; false when this launch fell back.
    pub reused: bool,
}

/// Binds the saved port, or any free port when it is busy or unset. The
/// listener is kept so nothing can take the port before the backend serves.
pub fn choose_port(
    persisted: Option<u16>,
    bind: impl Fn(u16) -> io::Result<TcpListener>,
) -> io::Result<ChosenPort> {
    if let Some(port) = persisted {
        match bind(port) {
            Ok(listener) => {
                return Ok(ChosenPort {
                    listener,
                    port,
                    reused: true,
                })
            }
            Err(error) => tracing::warn!(
                "Saved desktop port {port} is unavailable ({error}); using a free port for this launch"
            ),
        }
    }
    let listener = bind(0)?;
    let port = listener.local_addr()?.port();
    Ok(ChosenPort {
        listener,
        port,
        reused: false,
    })
}

/// A valid saved port is the user's or a previous launch's choice: a fallback
/// never replaces it, so the next launch returns to the same origin.
pub fn should_persist(persisted: &Persisted) -> bool {
    !matches!(persisted, Persisted::Port(_))
}

pub fn bind_loopback(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kronn-desktop-port-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_json_and_bare_values_and_rejects_the_rest() {
        assert_eq!(parse_port("{\"port\": 47315}\n"), Some(47315));
        assert_eq!(parse_port("  50000 "), Some(50000));
        for bad in [
            "",
            "{}",
            "{\"port\": 0}",
            "80",
            "70000",
            "{\"port\": \"x\"}",
            "port=1",
            "é",
        ] {
            assert_eq!(parse_port(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn persisted_port_round_trips_and_reports_missing_or_corrupt_files() {
        let dir = temp_dir("roundtrip");
        let path = port_file_path(&dir);
        assert_eq!(read_persisted(&path), Persisted::Missing);
        write_persisted(&path, 48123).unwrap();
        assert_eq!(read_persisted(&path), Persisted::Port(48123));
        std::fs::write(&path, "{\"port\":").unwrap();
        assert_eq!(read_persisted(&path), Persisted::Invalid);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_free_saved_port_is_reused() {
        let probe = bind_loopback(0).unwrap();
        let saved = probe.local_addr().unwrap().port();
        drop(probe);
        let chosen = choose_port(Some(saved), bind_loopback).unwrap();
        assert_eq!(chosen.port, saved);
        assert!(chosen.reused);
        assert_eq!(chosen.listener.local_addr().unwrap().port(), saved);
    }

    #[test]
    fn a_busy_saved_port_falls_back_and_keeps_the_saved_value() {
        let occupant = bind_loopback(0).unwrap();
        let saved = occupant.local_addr().unwrap().port();
        let chosen = choose_port(Some(saved), bind_loopback).unwrap();
        assert_ne!(chosen.port, saved);
        assert!(!chosen.reused);
        assert!(!should_persist(&Persisted::Port(saved)));
        drop(occupant);
    }

    #[test]
    fn first_launch_and_corrupt_files_persist_the_bound_port() {
        let chosen = choose_port(None, bind_loopback).unwrap();
        assert!(chosen.port >= MIN_PORT);
        assert!(!chosen.reused);
        assert!(should_persist(&Persisted::Missing));
        assert!(should_persist(&Persisted::Invalid));
    }

    #[test]
    fn the_listener_holds_the_port_until_it_is_handed_over() {
        let chosen = choose_port(None, bind_loopback).unwrap();
        assert!(bind_loopback(chosen.port).is_err(), "port must stay bound");
    }
}

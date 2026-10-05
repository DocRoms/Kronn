//! The saved loopback port of the desktop app (`desktop-port.json` in the Kronn
//! data directory). Shared by the desktop shell, which reads it before any
//! config load, and by the settings route that lets the user change it.

use std::io;
use std::path::{Path, PathBuf};

pub const PORT_FILE: &str = "desktop-port.json";
/// Ports below this need privileges on Unix; a saved value under it is ignored.
pub const MIN_PORT: u16 = 1024;

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

/// Highest port a user can pick.
pub const MAX_PORT: u16 = u16::MAX;

/// Validates a user-typed port: 1024..=65535.
pub fn validate_port(port: i64) -> Result<u16, String> {
    u16::try_from(port)
        .ok()
        .filter(|port| *port >= MIN_PORT)
        .ok_or_else(|| format!("Port must be between {MIN_PORT} and {MAX_PORT}"))
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
    fn user_ports_are_bounded_to_unprivileged_values() {
        assert_eq!(validate_port(1024), Ok(1024));
        assert_eq!(validate_port(65535), Ok(65535));
        for bad in [-1, 0, 80, 1023, 65536, i64::MAX] {
            assert!(validate_port(bad).is_err(), "{bad}");
        }
    }
}

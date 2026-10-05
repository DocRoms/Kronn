//! Loopback port of the embedded backend.
//!
//! The webview's localStorage is scoped to `http://127.0.0.1:<port>`, so the
//! desktop keeps one port across launches: the value in `desktop-port.json`
//! (Kronn data directory) is reused when it can be bound, and a busy port
//! falls back to a free one for this launch only.

use std::io;
use std::net::TcpListener;

pub use kronn::core::desktop_port::{
    port_file_path, read_persisted, write_persisted, Persisted, MIN_PORT, PORT_FILE,
};

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

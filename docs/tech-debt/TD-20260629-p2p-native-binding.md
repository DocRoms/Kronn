# TD-20260629-p2p-native-binding

- **ID**: TD-20260629-p2p-native-binding
- **Area**: Backend / Networking
- **Problem (fact)**: The contacts / peer (P2P) feature **cannot work for a natively-run Kronn**, by construction:
  1. **Binding**: native binds to `config.server.host` (default `127.0.0.1`); only Docker forces `0.0.0.0` (`backend/src/main.rs` ~L70-77). A native instance is therefore **only reachable from localhost**, never from another machine.
  2. **Advertised address**: `network-info` returned `advertised_host: 127.0.0.1` and `detected_ips: []` on macOS native — the CLI-based interface scan (`tailscale::scan_all_interfaces`, `ifconfig`/`ip` parsing) is fragile (PATH/format) — so the invite code a peer would use points at loopback.
  3. **Reachability-only contact model**: `contacts::add` pings `{peer}/api/health` → `accepted` if reachable, else `pending`. There is **no notification / accept handshake on the peer side** — adding a contact does nothing on the other machine. (User expectation of a "validate the connection" prompt doesn't match the design.)
  Result confirmed live 2026-06-29 (Mac native): contact `Romu` stuck `pending`; Mac advertised `127.0.0.1`; peer `172.20.231.144` (a WSL-internal IP) unreachable (`curl` timeout). The feature "never worked" cross-machine for native/WSL.
- **Why we can't fix now (constraint)**: A full fix spans binding policy + a security model (exposing the API to the LAN needs auth for non-localhost peers — `is_local_ip` only bypasses loopback + Docker bridge, so LAN peers get 401 on everything except `/api/health`), peer-auth in the invite code/handshake, and WSL networking guidance. Bigger than one PR.
- **Impact**: feature broken (cross-machine collaboration / inter-debug unusable on native + WSL)
- **Where (pointers)**:
  - `backend/src/main.rs` — host binding selection.
  - `backend/src/api/contacts.rs` — `add` (reachability-only status), `network_info`, `advertised_host_async`.
  - `backend/src/core/tailscale.rs` — `detect_all_ips` / `scan_all_interfaces` (CLI-parse fragility).
  - `backend/src/lib.rs` — `is_local_ip` (LAN peers are not "local" → auth applies).
- **Suggested direction (non-binding)**:
  - **Shipped 2026-06-29**: `KRONN_HOST` env override (headless) + a `UdpSocket`-based primary-LAN-IP detector feeding `detect_all_ips`/`advertised_host` (PATH-independent, cross-platform) **AND** a proper **Settings → Identity toggle** "Allow connections from other devices" (`core::net_expose` + `GET/POST /api/config/network-exposure`): flips `config.server.host` 0.0.0.0↔127.0.0.1, forces auth-on + token when exposing (secure-by-default), shows reachable IPs + a restart notice (Tauri restart button / CLI hint). The desktop app now honors `config.server.host` instead of hard-binding loopback.
  - **Remaining**: **peer auth** so LAN/Tailscale peer operations *beyond* `/api/health` aren't 401'd (the invite code should carry/establish a token — today only the reachability ping works unauthenticated); WSL guidance (the WSL-internal `172.x` isn't LAN-routable — needs `netsh portproxy` **or** Tailscale); optional live re-bind to avoid the restart.
  - **Recommended path for users**: **Tailscale** on both machines — stable routable IP across NAT/LAN/WSL; the whole `network-info`/`diagnose_unreachable` design already centers on it.
- **Next step**: create ticket.

## Notes

- Surfaced 2026-06-29 when the user tried to open a cross-machine (Mac ↔ WSL) discussion for inter-debug. The reachability quick-win lands the same day; full cross-machine P2P (auth + WSL) remains.

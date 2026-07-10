use axum::{extract::{Path, State}, Json};
use chrono::Utc;

use crate::models::{AddContactRequest, AddContactResult, ApiResponse, Contact, NetworkInfo};
use crate::AppState;

/// GET /api/contacts
pub async fn list(State(state): State<AppState>) -> Json<ApiResponse<Vec<Contact>>> {
    match state.db.with_conn(crate::db::contacts::list_contacts).await {
        Ok(contacts) => Json(ApiResponse::ok(contacts)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/contacts — add a contact from invite code
pub async fn add(
    State(state): State<AppState>,
    Json(req): Json<AddContactRequest>,
) -> Json<ApiResponse<AddContactResult>> {
    let (pseudo, kronn_url) = match crate::db::contacts::parse_invite_code(&req.invite_code) {
        Some(parsed) => parsed,
        None => return Json(ApiResponse::err("Invalid invite code. Format: kronn:pseudo@host:port")),
    };

    // Check if already exists
    let code = req.invite_code.clone();
    let exists = state.db.with_conn(move |conn| {
        crate::db::contacts::find_contact_by_invite_code(conn, &code)
    }).await;
    if let Ok(Some(_)) = exists {
        return Json(ApiResponse::err("Contact already exists"));
    }

    // Ping the peer to check reachability (non-blocking, 3s timeout)
    let health_url = format!("{}/api/health", kronn_url);
    let ping_error = reqwest::Client::new()
        .get(&health_url)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await;

    let (reachable, warning) = match &ping_error {
        Ok(r) if r.status().is_success() => (true, None),
        _ => {
            // Diagnose WHY the peer is unreachable
            let hint = diagnose_unreachable(&kronn_url).await;
            (false, Some(hint))
        }
    };

    let status = if reachable { "accepted" } else { "pending" };

    let now = Utc::now();
    let contact = Contact {
        id: uuid::Uuid::new_v4().to_string(),
        pseudo,
        avatar_email: None,
        kronn_url,
        invite_code: req.invite_code,
        status: status.into(),
        created_at: now,
        updated_at: now,
    };

    let c = contact.clone();
    match state.db.with_conn(move |conn| crate::db::contacts::insert_contact(conn, &c)).await {
        Ok(()) => Json(ApiResponse::ok(AddContactResult { contact, warning })),
        Err(e) => Json(ApiResponse::err(format!("Failed to add contact: {}", e))),
    }
}

/// Diagnose why a peer is unreachable and return a user-friendly hint.
async fn diagnose_unreachable(kronn_url: &str) -> String {
    let peer_host = kronn_url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split(':')
        .next()
        .unwrap_or("");

    let peer_is_tailscale = is_tailscale_ip(peer_host);
    let local_tailscale = crate::core::tailscale::detect_ip().await;

    if peer_is_tailscale && local_tailscale.is_none() {
        return "TAILSCALE_MISSING".into();
    }

    if peer_is_tailscale && local_tailscale.is_some() {
        return "TAILSCALE_UNREACHABLE".into();
    }

    let peer_is_lan = is_private_ip(peer_host);
    if peer_is_lan {
        return "LAN_UNREACHABLE".into();
    }

    "NETWORK_UNREACHABLE".into()
}

/// Check if an IP is in the Tailscale CGNAT range (100.64.0.0/10)
fn is_tailscale_ip(ip: &str) -> bool {
    let parts: Vec<u8> = ip.split('.').filter_map(|s| s.parse().ok()).collect();
    parts.len() == 4 && parts[0] == 100 && (64..=127).contains(&parts[1])
}

/// Check if an IP is in a private range (10.x, 172.16-31.x, 192.168.x)
fn is_private_ip(ip: &str) -> bool {
    let parts: Vec<u8> = ip.split('.').filter_map(|s| s.parse().ok()).collect();
    if parts.len() != 4 { return false; }
    parts[0] == 10
        || (parts[0] == 172 && (16..=31).contains(&parts[1]))
        || (parts[0] == 192 && parts[1] == 168)
}

/// DELETE /api/contacts/:id
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state.db.with_conn(move |conn| crate::db::contacts::delete_contact(conn, &id)).await {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err("Contact not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// This instance's pseudo for invite codes, with a safe fallback.
///
/// Falls back to `"anonymous"` when no pseudo is configured so the invite
/// code is always well-formed (`kronn:pseudo@host:port`). An empty pseudo
/// would yield `kronn:@host:port`, which `parse_invite_code` rejects — that
/// mismatch is what caused the cross-machine presence flap/ban.
pub fn invite_pseudo(server: &crate::models::ServerConfig) -> String {
    server.pseudo.clone().unwrap_or_else(|| "anonymous".into())
}

/// Build this instance's canonical invite code (`kronn:pseudo@host:port`).
///
/// Single source of truth shared by the `/api/contacts/invite-code` endpoint
/// and the outbound WS presence handshake (`ws_client`) so the code a peer
/// stores always matches the code we send on the wire.
pub async fn build_invite_code(server: &crate::models::ServerConfig) -> String {
    let pseudo = invite_pseudo(server);
    let host = advertised_host_async(server).await;
    format!("kronn:{}@{}:{}", pseudo, host, server.port)
}

/// GET /api/contacts/invite-code — returns this instance's invite code
pub async fn invite_code(State(state): State<AppState>) -> Json<ApiResponse<String>> {
    let config = state.config.read().await;
    let code = build_invite_code(&config.server).await;
    Json(ApiResponse::ok(code))
}

/// GET /api/contacts/network-info — returns network status (Tailscale, host, port)
pub async fn network_info(State(state): State<AppState>) -> Json<ApiResponse<NetworkInfo>> {
    let config = state.config.read().await;
    let tailscale_ip = crate::core::tailscale::detect_ip().await;
    let host = advertised_host_async(&config.server).await;
    let detected_ips = crate::core::tailscale::detect_all_ips().await;
    let info = NetworkInfo {
        tailscale_ip,
        advertised_host: host,
        port: config.server.port,
        domain: config.server.domain.clone(),
        detected_ips,
    };
    Json(ApiResponse::ok(info))
}

/// Returns the best publicly-reachable host for this instance.
/// Prefers `domain` (explicitly configured), falls back to `host`,
/// but replaces bind-all addresses (0.0.0.0) with localhost.
pub fn advertised_host(server: &crate::models::ServerConfig) -> String {
    if let Some(ref domain) = server.domain {
        if !domain.is_empty() {
            return domain.clone();
        }
    }
    let h = &server.host;
    if h == "0.0.0.0" || h == "::" {
        "localhost".into()
    } else {
        h.clone()
    }
}

/// Async version that also checks for Tailscale IP.
/// Priority: domain > tailscale_ip > host (with 0.0.0.0 fallback to localhost).
pub async fn advertised_host_async(server: &crate::models::ServerConfig) -> String {
    // 1. Explicit domain always wins
    if let Some(ref domain) = server.domain {
        if !domain.is_empty() {
            return domain.clone();
        }
    }

    // 2. Tailscale IP (stable across network changes)
    if let Some(ts_ip) = crate::core::tailscale::detect_ip().await {
        return ts_ip;
    }

    // 3. Configured host — but loopback / bind-all isn't reachable by a peer,
    //    so fall back to the primary LAN IP (std UdpSocket trick) when bound to
    //    127.0.0.1 / 0.0.0.0 / ::, so the invite code points somewhere a peer
    //    can actually reach. Keeps "localhost" only when no LAN IP is found.
    let h = &server.host;
    if h == "0.0.0.0" || h == "::" || h == "127.0.0.1" || h == "localhost" {
        return crate::core::tailscale::primary_lan_ipv4().unwrap_or_else(|| "localhost".into());
    }
    h.clone()
}

/// GET /api/contacts/:id/ping — check if a contact's Kronn is online
pub async fn ping(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<bool>> {
    let contact = state.db.with_conn(move |conn| {
        crate::db::contacts::get_contact(conn, &id)
    }).await;

    let contact = match contact {
        Ok(Some(c)) => c,
        Ok(None) => return Json(ApiResponse::err("Contact not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let url = format!("{}/api/health", contact.kronn_url);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => Json(ApiResponse::ok(true)),
        _ => Json(ApiResponse::ok(false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ServerConfig;

    fn cfg(host: &str, domain: Option<&str>) -> ServerConfig {
        ServerConfig {
            host: host.into(),
            port: 3140,
            domain: domain.map(String::from),
            auth_token: None,
            auth_enabled: false,
            auth_strict_localhost: false,
            failure_notify_url: None,
            run_retention_days: 0,
            max_concurrent_agents: 5,
            agent_stall_timeout_min: 5,
            pseudo: None,
            avatar_email: None,
            bio: None,
            global_context: None,
            global_context_mode: "always".into(),
            anti_hallucination_mode: "warn".into(),
            continual_learning_enabled: false,
            debug_mode: false,
            default_model_tier: crate::models::ModelTier::Default,
            default_summary_strategy: crate::models::SummaryStrategy::Off,
        }
    }

    #[test]
    fn advertised_host_prefers_domain() {
        assert_eq!(
            advertised_host(&cfg("127.0.0.1", Some("kronn.tailnet.ts.net"))),
            "kronn.tailnet.ts.net"
        );
    }

    #[test]
    fn advertised_host_falls_back_to_host() {
        assert_eq!(
            advertised_host(&cfg("192.168.1.50", None)),
            "192.168.1.50"
        );
    }

    #[test]
    fn advertised_host_replaces_bind_all() {
        assert_eq!(advertised_host(&cfg("0.0.0.0", None)), "localhost");
        assert_eq!(advertised_host(&cfg("::", None)), "localhost");
    }

    #[test]
    fn advertised_host_ignores_empty_domain() {
        assert_eq!(
            advertised_host(&cfg("10.0.0.5", Some(""))),
            "10.0.0.5"
        );
    }

    #[tokio::test]
    async fn advertised_host_async_never_advertises_bind_all_or_loopback() {
        // A peer can't reach 0.0.0.0/::/127.0.0.1 — the async resolver must
        // replace them with a usable address (primary LAN IP via the UdpSocket
        // trick, or "localhost" if none). It must NEVER hand back a bind-all.
        for h in ["0.0.0.0", "::", "127.0.0.1", "localhost"] {
            let got = advertised_host_async(&cfg(h, None)).await;
            assert_ne!(got, "0.0.0.0", "must not advertise bind-all (host={h})");
            assert_ne!(got, "::", "must not advertise bind-all (host={h})");
            assert!(!got.is_empty());
        }
        // An explicit domain still wins over everything.
        assert_eq!(
            advertised_host_async(&cfg("0.0.0.0", Some("kronn.example.com"))).await,
            "kronn.example.com"
        );
    }

    #[test]
    fn invite_pseudo_falls_back_to_anonymous() {
        // No pseudo configured → must NOT yield an empty pseudo (empty pseudo
        // produces `kronn:@host:port`, which peers reject → presence flap/ban).
        assert_eq!(invite_pseudo(&cfg("0.0.0.0", None)), "anonymous");
        let mut c = cfg("0.0.0.0", None);
        c.pseudo = Some("Romu".into());
        assert_eq!(invite_pseudo(&c), "Romu");
    }

    #[tokio::test]
    async fn build_invite_code_is_always_parseable() {
        use crate::db::contacts::parse_invite_code;
        // Even with no pseudo + bind-all host, the emitted code must round-trip
        // through the peer's parser (non-empty pseudo, reachable host).
        for h in ["0.0.0.0", "::", "127.0.0.1", "192.168.1.5"] {
            let code = build_invite_code(&cfg(h, None)).await;
            assert!(code.starts_with("kronn:"), "host={h} code={code}");
            assert!(!code.starts_with("kronn:@"), "empty pseudo leaked: {code}");
            let parsed = parse_invite_code(&code);
            assert!(parsed.is_some(), "peer must accept our code (host={h}): {code}");
        }
    }

    #[test]
    fn is_tailscale_ip_accepts_canonical_cgnat_range() {
        // 100.64.0.0/10 → first octet 100, second 64..=127 ; 4-octet form only.
        assert!(is_tailscale_ip("100.64.0.1"));
        assert!(is_tailscale_ip("100.65.10.20"));
        assert!(is_tailscale_ip("100.100.0.1"));
        assert!(is_tailscale_ip("100.127.255.254"));
    }

    #[test]
    fn is_tailscale_ip_rejects_out_of_range() {
        // Just outside the /10 (101.x, 100.63, 100.128).
        assert!(!is_tailscale_ip("101.64.0.1"));
        assert!(!is_tailscale_ip("100.63.0.1"));
        assert!(!is_tailscale_ip("100.128.0.1"));
        assert!(!is_tailscale_ip("10.0.0.1"));
        assert!(!is_tailscale_ip("192.168.1.1"));
    }

    #[test]
    fn is_tailscale_ip_rejects_malformed() {
        // Less than 4 octets, IPv6, garbage.
        assert!(!is_tailscale_ip("100.64.0"));
        assert!(!is_tailscale_ip(""));
        assert!(!is_tailscale_ip("100"));
        assert!(!is_tailscale_ip("::1"));
        assert!(!is_tailscale_ip("not-an-ip"));
    }

    #[test]
    fn is_private_ip_accepts_rfc1918_class_a() {
        assert!(is_private_ip("10.0.0.1"));
        assert!(is_private_ip("10.255.255.255"));
    }

    #[test]
    fn is_private_ip_accepts_rfc1918_class_b() {
        // 172.16.0.0/12 → second octet 16..=31.
        assert!(is_private_ip("172.16.0.1"));
        assert!(is_private_ip("172.20.5.5"));
        assert!(is_private_ip("172.31.255.255"));
    }

    #[test]
    fn is_private_ip_accepts_rfc1918_class_c() {
        assert!(is_private_ip("192.168.0.1"));
        assert!(is_private_ip("192.168.1.100"));
    }

    #[test]
    fn is_private_ip_rejects_near_misses() {
        // 172.15 and 172.32 are public.
        assert!(!is_private_ip("172.15.0.1"));
        assert!(!is_private_ip("172.32.0.1"));
        // 192.167 / 192.169 are public.
        assert!(!is_private_ip("192.167.1.1"));
        assert!(!is_private_ip("192.169.1.1"));
        // 11.x and 9.x are public.
        assert!(!is_private_ip("11.0.0.1"));
        assert!(!is_private_ip("9.255.255.255"));
    }

    #[test]
    fn is_private_ip_rejects_malformed() {
        assert!(!is_private_ip(""));
        assert!(!is_private_ip("garbage"));
        assert!(!is_private_ip("192.168.1"));
        assert!(!is_private_ip("::1"));
    }

    #[test]
    fn is_tailscale_and_private_dont_overlap() {
        // Sanity : a tailscale IP must NOT be classified as RFC1918 and vice-versa.
        assert!(is_tailscale_ip("100.100.0.1"));
        assert!(!is_private_ip("100.100.0.1"));
        assert!(is_private_ip("10.0.0.1"));
        assert!(!is_tailscale_ip("10.0.0.1"));
    }
}

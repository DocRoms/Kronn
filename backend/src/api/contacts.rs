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
    let health_url = format!("{}/api/health", &kronn_url);
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

/// GET /api/contacts/invite-code — returns this instance's invite code
pub async fn invite_code(State(state): State<AppState>) -> Json<ApiResponse<String>> {
    let config = state.config.read().await;
    let pseudo = config.server.pseudo.clone().unwrap_or_else(|| "anonymous".into());
    let host = advertised_host_async(&config.server).await;
    let port = config.server.port;
    let code = format!("kronn:{}@{}:{}", pseudo, host, port);
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

    // 3. Configured host (replace bind-all)
    let h = &server.host;
    if h == "0.0.0.0" || h == "::" {
        "localhost".into()
    } else {
        h.clone()
    }
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
            max_concurrent_agents: 5,
            agent_stall_timeout_min: 5,
            pseudo: None,
            avatar_email: None,
            bio: None,
            global_context: None,
            global_context_mode: "always".into(),
            debug_mode: false,
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
}

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::models::{AddContactRequest, AddContactResult, ApiResponse, Contact, NetworkInfo};
use crate::AppState;

/// GET /api/contacts
pub async fn list(State(state): State<AppState>) -> Json<ApiResponse<Vec<Contact>>> {
    match state.db.with_conn(crate::db::contacts::list_contacts).await {
        Ok(contacts) => Json(ApiResponse::ok(contacts)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// What a bare invite code (an address) gets: it pairs nobody.
const BARE_CODE_HINT: &str = "A bare invite code is only an address and cannot pair a contact. \
     Ask the other side to open Settings > Identity, generate a pairing code \
     (kronn:pseudo@host:port#secret) and paste that here.";

/// What the accepting side answers to any pairing it cannot complete: the same
/// words whatever the reason, so they tell nothing about which contacts exist.
const PAIR_REFUSED: &str = "Pairing refused: the pairing code is unknown, expired or already \
     used, or it does not fit this contact. A code pairs a new contact only; a contact that \
     already exists on this instance needs a code issued for it (Re-pair on its entry), or \
     must be deleted on both sides first. Generate a new pairing code on this instance \
     (Settings > Identity) and paste it on the other side.";

/// Body of `POST /api/contacts/pairing-offer`.
#[derive(Debug, Default, Deserialize)]
pub struct PairingOfferRequest {
    /// Issue the code for this existing, unpaired contact (a re-pair from its
    /// entry): the code then pairs that contact and nobody else.
    #[serde(default)]
    pub contact_id: Option<String>,
}

/// POST /api/contacts/pairing-offer — hand out a pairing code: this instance's
/// public invite code, `#`, and a fresh random secret (KT-1650). The secret is
/// shown once; only its hash is kept, until a peer spends it (7 days at most).
/// Without a `contact_id` the code pairs a NEW contact only.
pub async fn pairing_offer(
    State(state): State<AppState>,
    body: Option<Json<PairingOfferRequest>>,
) -> Json<ApiResponse<String>> {
    let contact_id = body.and_then(|Json(b)| b.contact_id);
    let code = {
        let config = state.config.read().await;
        match try_build_invite_code(&config.server).await {
            Ok(code) => code,
            Err(reason) => return Json(ApiResponse::err(reason)),
        }
    };
    match state
        .db
        .with_conn(move |conn| {
            crate::db::contacts::create_pairing_offer(conn, contact_id.as_deref())
        })
        .await
    {
        Ok(Some(secret)) => Json(ApiResponse::ok(format!("{code}#{secret}"))),
        Ok(None) => Json(ApiResponse::err(
            "This contact cannot be re-paired: it is unknown, already paired or refused",
        )),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/contacts — pair with a contact from the pairing code it gave.
///
/// The code carries the peer's address and the secret we will present to it. We
/// make our own secret for the other direction and hand it over in the pairing
/// request, which the peer accepts only against a live pairing code of its own.
/// Both secrets are random and exchanged here once; from then on every P2P route
/// asks for them, never just for the public invite code.
pub async fn add(
    State(state): State<AppState>,
    Json(req): Json<AddContactRequest>,
) -> Json<ApiResponse<AddContactResult>> {
    let Some((invite_code, offer_secret)) =
        crate::db::contacts::split_pairing_code(&req.invite_code)
    else {
        return Json(ApiResponse::err(
            if crate::db::contacts::parse_invite_code(&req.invite_code).is_some() {
                BARE_CODE_HINT
            } else {
                "Invalid pairing code. Format: kronn:pseudo@host:port#secret"
            },
        ));
    };
    let Some((pseudo, kronn_url)) = crate::db::contacts::parse_invite_code(&invite_code) else {
        return Json(ApiResponse::err(
            "Invalid pairing code. Format: kronn:pseudo@host:port#secret",
        ));
    };

    // A contact that came in as a request, or predates pairing secrets, is paired
    // in place (its row and shared discussions stay); a paired one is not touched.
    let lookup = invite_code.clone();
    let existing = state
        .db
        .with_conn(move |conn| {
            let found = crate::db::contacts::find_contact_by_invite_code(conn, &lookup)?;
            Ok(match found {
                Some(c) => {
                    let paired = crate::db::contacts::is_paired(conn, &c.id)?;
                    Some((c, paired))
                }
                None => None,
            })
        })
        .await;
    let existing_id = match existing {
        Ok(Some((c, false))) if c.status != "refused" => Some(c.id),
        Ok(Some(_)) => {
            return Json(ApiResponse::err(
                "Contact already exists: delete it on both sides before pairing again",
            ))
        }
        Ok(None) => None,
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    if !state.p2p.enabled() {
        return Json(ApiResponse::err(
            "P2P connections are off: turn them on to pair a contact",
        ));
    }
    let our_code = {
        let config = state.config.read().await;
        match try_build_invite_code(&config.server).await {
            Ok(code) => code,
            Err(reason) => return Json(ApiResponse::err(reason)),
        }
    };
    let Some(base) = crate::db::contacts::contact_base_url(&kronn_url) else {
        return Json(ApiResponse::err("Contact address is invalid"));
    };
    let Ok(url) = reqwest::Url::parse(&format!("{base}/api/contacts/pair")) else {
        return Json(ApiResponse::err("Contact address is invalid"));
    };
    let client = match crate::core::safe_http::peer_client(std::time::Duration::from_secs(5)) {
        Ok(client) => client,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    let reverse_secret = crate::db::contacts::generate_secret();
    let body = serde_json::json!({
        "from_invite_code": our_code,
        "offer_secret": offer_secret,
        "reverse_secret": reverse_secret,
    });
    let pair = async {
        let resp = client.post(url).json(&body).send().await.ok()?;
        resp.json::<serde_json::Value>().await.ok()
    };
    match crate::api::federation::unless_p2p_off(&state, pair).await {
        None => return Json(ApiResponse::err("P2P connections were turned off")),
        Some(None) => {
            let hint = diagnose_unreachable(&kronn_url).await;
            return Json(ApiResponse::err(format!(
                "Pairing not completed: the peer is unreachable ({hint}). The pairing code \
                 stays valid for {} days: retry once the network works.",
                crate::db::contacts::PAIRING_OFFER_TTL_DAYS
            )));
        }
        Some(Some(answer)) if answer.get("success").and_then(|v| v.as_bool()) == Some(true) => {}
        Some(Some(answer)) => {
            let reason = answer
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("no reason given");
            return Json(ApiResponse::err(format!(
                "The peer refused the pairing: {reason}"
            )));
        }
    }

    // The peer accepted: keep both secrets (ours hashed, theirs as given), in
    // one transaction that re-checks the contact. It may have been paired,
    // refused or deleted while the peer was being asked: then nothing is
    // written (the peer still holds our secret and must delete its side too).
    let now = Utc::now();
    let inbound_hash = crate::db::contacts::hash_secret(&reverse_secret);
    let new_contact = Contact {
        id: uuid::Uuid::new_v4().to_string(),
        pseudo,
        avatar_email: None,
        kronn_url,
        invite_code,
        status: "pending".into(),
        created_at: now,
        updated_at: now,
    };
    let stored = state
        .db
        .with_conn(move |conn| {
            crate::db::contacts::complete_outbound_pairing(
                conn,
                existing_id.as_deref(),
                &new_contact,
                &inbound_hash,
                &offer_secret,
            )
        })
        .await;
    match stored {
        Ok(Some(contact)) => Json(ApiResponse::ok(AddContactResult {
            contact,
            warning: None,
        })),
        Ok(None) => Json(ApiResponse::err(
            "Pairing not stored: the contact changed while the peer was being asked (paired, \
             refused or deleted). Delete it on both sides and pair again.",
        )),
        Err(e) => Json(ApiResponse::err(format!("Failed to add contact: {}", e))),
    }
}

/// Body of `POST /api/contacts/pair`: a peer completes a pairing with a pairing
/// code this instance handed out.
#[derive(Debug, Deserialize)]
pub struct PairRequest {
    /// The caller's public invite code: who it is, not a credential.
    pub from_invite_code: String,
    /// The secret of the pairing code we handed out. It becomes what the caller
    /// presents to us from now on.
    pub offer_secret: String,
    /// The secret we will present to the caller from now on.
    pub reverse_secret: String,
}

#[derive(Debug, Serialize)]
pub struct PairResponse {
    pub pseudo: String,
}

/// POST /api/contacts/pair — auth-exempt (a remote peer has no bearer token),
/// gated on a live pairing code. The code is spent by the first successful
/// pairing; an already paired contact is never overwritten, so a leaked code
/// cannot take over an existing contact.
pub async fn pair(
    State(state): State<AppState>,
    Json(req): Json<PairRequest>,
) -> Json<ApiResponse<PairResponse>> {
    let refused = || Json(ApiResponse::err(PAIR_REFUSED));
    if !state.p2p.enabled() {
        return refused();
    }
    let invite_code = req.from_invite_code.trim().to_string();
    if invite_code.len() > crate::db::contacts::MAX_INVITE_CODE_LEN
        || !crate::db::contacts::is_well_formed_secret(&req.offer_secret)
        || !crate::db::contacts::is_well_formed_secret(&req.reverse_secret)
    {
        return refused();
    }
    let Some((pseudo, kronn_url)) = crate::db::contacts::parse_invite_code(&invite_code) else {
        return refused();
    };
    let our_pseudo = {
        let config = state.config.read().await;
        invite_pseudo(&config.server)
    };

    let attempt = crate::db::contacts::PairingAttempt {
        offer_secret: req.offer_secret,
        invite_code,
        pseudo,
        kronn_url,
        reverse_secret: req.reverse_secret,
    };
    let gate = state.p2p.clone();
    let paired = state
        .db
        .with_conn(move |conn| {
            // Held through the write: P2P cannot turn off in between.
            let on = gate.read();
            if !*on {
                return Ok(false);
            }
            crate::db::contacts::accept_pairing(conn, &attempt)
        })
        .await;
    match paired {
        Ok(true) => Json(ApiResponse::ok(PairResponse { pseudo: our_pseudo })),
        Ok(false) => refused(),
        Err(e) => {
            tracing::warn!("pairing failed: {e}");
            refused()
        }
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
    if parts.len() != 4 {
        return false;
    }
    parts[0] == 10
        || (parts[0] == 172 && (16..=31).contains(&parts[1]))
        || (parts[0] == 192 && parts[1] == 168)
}

/// DELETE /api/contacts/:id
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state
        .db
        .with_conn(move |conn| crate::db::contacts::delete_contact(conn, &id))
        .await
    {
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

/// The invite code of an instance behind a declared TLS proxy (KT-1596):
/// `kronns:pseudo@host:port` on the proxy's public origin, which peers dial
/// over https / wss. The plain backend port is never advertised: it is bound to
/// loopback, and a code carrying it would send P2P across the network in clear.
pub fn proxy_invite_code(pseudo: &str, public_origin: Option<&str>) -> Result<String, String> {
    let origin = public_origin.ok_or_else(|| {
        "TLS is declared as terminated by a proxy but its public https:// address is not set: \
         invites advertise it. Set it in Settings (or KRONN_PUBLIC_ORIGIN)."
            .to_string()
    })?;
    let authority = crate::db::contacts::public_origin_authority(origin).ok_or_else(|| {
        format!("The proxy's public address {origin:?} is not a bare https:// origin.")
    })?;
    Ok(format!(
        "{}{}@{}",
        crate::db::contacts::TLS_INVITE_PREFIX,
        pseudo,
        authority
    ))
}

/// Build this instance's canonical invite code (`kronn:pseudo@host:port`, or
/// `kronns:…` on the proxy's public origin when TLS is terminated by a proxy).
///
/// Single source of truth shared by the `/api/contacts/invite-code` endpoint
/// and the outbound WS presence handshake (`ws_client`) so the code a peer
/// stores always matches the code we send on the wire. An error means there is
/// no safe code to give out (a declared proxy with no public origin).
pub async fn try_build_invite_code(server: &crate::models::ServerConfig) -> Result<String, String> {
    let pseudo = invite_pseudo(server);
    if crate::core::net_expose::proxy_declared_for(server) {
        return proxy_invite_code(
            &pseudo,
            crate::core::net_expose::public_origin_for(server).as_deref(),
        );
    }
    // Exposure was asked for and nothing carries TLS: this boot serves nobody on
    // the network, and a plain LAN / Tailscale address in an invite would send the
    // peer (and its P2P traffic) to a port that is either closed or in clear.
    if crate::core::net_expose::plain_http_blocked_for(server) {
        return Err(
            "No invite code while network exposure is blocked: nothing carries TLS. Declare \
             a TLS proxy and its public https:// address in Settings, or turn the exposure \
             off."
                .to_string(),
        );
    }
    let host = advertised_host_async(server).await;
    Ok(format!(
        "kronn:{}@{}:{}",
        pseudo,
        host,
        server.listening_port()
    ))
}

/// [`try_build_invite_code`] for the broadcasts that carry an invite code in
/// every message: when there is no safe code (a declared proxy with no public
/// origin) it is EMPTY, which peers refuse from anything but the local frontend,
/// so the failure is closed and never a plain-HTTP address on the wire.
pub async fn build_invite_code(server: &crate::models::ServerConfig) -> String {
    try_build_invite_code(server)
        .await
        .unwrap_or_else(|reason| {
            tracing::warn!("No invite code to announce: {reason}");
            String::new()
        })
}

/// GET /api/contacts/invite-code — returns this instance's invite code
pub async fn invite_code(State(state): State<AppState>) -> Json<ApiResponse<String>> {
    let config = state.config.read().await;
    match try_build_invite_code(&config.server).await {
        Ok(code) => Json(ApiResponse::ok(code)),
        Err(reason) => Json(ApiResponse::err(reason)),
    }
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
        port: config.server.listening_port(),
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
    if crate::core::net_expose::is_exposed_host(h) {
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
    if crate::core::net_expose::is_exposed_host(h) || h == "127.0.0.1" || h == "localhost" {
        return crate::core::tailscale::primary_lan_ipv4().unwrap_or_else(|| "localhost".into());
    }
    h.clone()
}

/// HTTP client for requests to a contact: short timeout, no redirects (a
/// peer must not bounce Kronn to another address).
pub(crate) fn guarded_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
}

/// GET /api/contacts/:id/ping — check if a contact's Kronn is online
pub async fn ping(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<bool>> {
    let contact = state
        .db
        .with_conn(move |conn| crate::db::contacts::get_contact(conn, &id))
        .await;

    let contact = match contact {
        Ok(Some(c)) => c,
        Ok(None) => return Json(ApiResponse::err("Contact not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };
    if !state.p2p.enabled() {
        return Json(ApiResponse::err("P2P connections are off"));
    }
    // A request or a refused contact came from someone else's code: Kronn
    // never sends it a request.
    if !crate::db::contacts::dials_outbound(&contact.status) {
        return Json(ApiResponse::err("Contact not accepted"));
    }
    let Some(base) = crate::db::contacts::contact_base_url(&contact.kronn_url) else {
        return Json(ApiResponse::err("Contact address is invalid"));
    };

    // Turning P2P off cancels the ping in flight.
    let ping = guarded_client().get(format!("{base}/api/health")).send();
    match crate::api::federation::unless_p2p_off(&state, ping)
        .await
        .and_then(Result::ok)
    {
        Some(resp) if resp.status().is_success() => Json(ApiResponse::ok(true)),
        _ => Json(ApiResponse::ok(false)),
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::models::ServerConfig;

    fn cfg(host: &str, domain: Option<&str>) -> ServerConfig {
        ServerConfig {
            host: host.into(),
            port: 3140,
            runtime_port: None,
            domain: domain.map(String::from),
            auth_token: None,
            auth_enabled: false,
            auth_locked: false,
            auth_token_session_only: false,
            auth_strict_localhost: false,
            tls_terminated_by_proxy: false,
            public_origin: None,
            protect_vault_from_agents: true,
            lane_seeds_copy_enabled: true,
            lane_seeds_adopt_suggested: false,
            lane_seeds_strong_confirm: true,
            p2p_enabled: false,
            frontend_origins: vec![],
            failure_notify_url: None,
            run_retention_days: 0,
            run_payload_retention_days: 30,
            execution_variable_retention_days: 30,
            interrupted_worktree_ttl_days:
                crate::models::setup::DEFAULT_INTERRUPTED_WORKTREE_TTL_DAYS,
            disk_critical_gib: crate::models::setup::DEFAULT_DISK_CRITICAL_GIB,
            disk_warning_gib: crate::models::setup::DEFAULT_DISK_WARNING_GIB,
            disk_launch_min_free_gib: crate::models::setup::DEFAULT_DISK_LAUNCH_MIN_FREE_GIB,
            disk_sweep_idle_hours: crate::models::setup::DEFAULT_DISK_SWEEP_IDLE_HOURS,
            max_concurrent_agents: 5,
            agent_stall_timeout_min: 5,
            agent_global_timeout_min: crate::models::DEFAULT_AGENT_GLOBAL_TIMEOUT_MIN,
            local_agent_global_timeout_min: crate::models::DEFAULT_LOCAL_AGENT_GLOBAL_TIMEOUT_MIN,
            ollama_context_overrides: std::collections::HashMap::new(),
            max_prompt_bytes: crate::models::setup::DEFAULT_MAX_PROMPT_BYTES,
            pseudo: None,
            avatar_email: None,
            bio: None,
            global_context: None,
            global_context_mode: "always".into(),
            anti_hallucination_mode: "warn".into(),
            continual_learning_enabled: false,
            debug_mode: false,
            discussion_notes_enabled: true,
            default_model_tier: crate::models::ModelTier::Default,
            default_summary_strategy: crate::models::SummaryStrategy::Off,
            agent_handoffs_enabled: false,
            agent_handoff_paid_limit: 1,
            agent_handoff_paid_unlimited: false,
            agent_handoff_blocked_agents: vec![],
            discussion_weight: crate::models::DiscussionWeightConfig::default(),
            timezone: None,
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
        assert_eq!(advertised_host(&cfg("192.168.1.50", None)), "192.168.1.50");
    }

    #[test]
    fn advertised_host_replaces_bind_all() {
        assert_eq!(advertised_host(&cfg("0.0.0.0", None)), "localhost");
        assert_eq!(advertised_host(&cfg("::", None)), "localhost");
    }

    #[test]
    fn advertised_host_ignores_empty_domain() {
        assert_eq!(advertised_host(&cfg("10.0.0.5", Some(""))), "10.0.0.5");
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
        // Even with no pseudo, the emitted code must round-trip through the
        // peer's parser (non-empty pseudo, reachable host). An exposed host only
        // gets a code once a proxy carries TLS (next test).
        for h in ["127.0.0.1", "localhost", "[::1]"] {
            let code = try_build_invite_code(&cfg(h, None)).await.unwrap();
            assert!(code.starts_with("kronn:"), "host={h} code={code}");
            assert!(!code.starts_with("kronn:@"), "empty pseudo leaked: {code}");
            let parsed = parse_invite_code(&code);
            assert!(
                parsed.is_some(),
                "peer must accept our code (host={h}): {code}"
            );
        }
    }

    #[tokio::test]
    async fn no_plain_invite_while_exposure_is_blocked() {
        // KT-1596: exposure requested, nothing carries TLS → this boot serves
        // nobody and the invite would name a port that is closed or in clear.
        for h in ["0.0.0.0", "::", "192.168.1.5"] {
            let mut server = cfg(h, Some("kronn.example"));
            let reason = try_build_invite_code(&server)
                .await
                .expect_err("a plain LAN invite must not be handed out");
            assert!(reason.contains("TLS"), "{reason}");
            // The infallible variant (every broadcast) is EMPTY, never plain.
            assert_eq!(build_invite_code(&server).await, "");
            // A declared proxy lifts the block: the https/wss form is advertised.
            server.tls_terminated_by_proxy = true;
            server.public_origin = Some("https://kronn.example.com".into());
            assert_eq!(
                try_build_invite_code(&server).await.unwrap(),
                "kronns:anonymous@kronn.example.com:443"
            );
        }
    }

    #[test]
    fn a_proxied_instance_advertises_its_public_https_origin_never_the_plain_port() {
        use crate::db::contacts::parse_invite_code;
        // KT-1596: behind a TLS proxy the invite names the proxy, over https,
        // so peers dial wss and P2P never crosses the network in clear.
        let code = proxy_invite_code("Romu", Some("https://kronn.example.com")).unwrap();
        assert_eq!(code, "kronns:Romu@kronn.example.com:443");
        assert_eq!(
            parse_invite_code(&code),
            Some(("Romu".into(), "https://kronn.example.com:443".into()))
        );
        let ws = crate::core::ws_client::peer_ws_url("https://kronn.example.com:443").unwrap();
        assert_eq!(ws, "wss://kronn.example.com:443/api/ws");
        // A custom proxy port survives.
        let code = proxy_invite_code("Romu", Some("https://kronn.example.com:8443/")).unwrap();
        assert_eq!(code, "kronns:Romu@kronn.example.com:8443");
        // No public origin → no code at all (not a plain-HTTP fallback).
        assert!(proxy_invite_code("Romu", None).is_err());
        // Not an https origin → refused too.
        for bad in [
            "http://kronn.example.com",
            "kronn.example.com",
            "https://h/x",
        ] {
            assert!(proxy_invite_code("Romu", Some(bad)).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn a_declared_proxy_makes_build_invite_code_use_the_https_form() {
        let mut server = cfg("0.0.0.0", Some("ignored.example"));
        server.tls_terminated_by_proxy = true;
        // Declared, no origin: nothing safe to advertise.
        assert!(try_build_invite_code(&server).await.is_err());
        server.public_origin = Some("https://kronn.example.com".into());
        let code = try_build_invite_code(&server).await.unwrap();
        assert_eq!(code, "kronns:anonymous@kronn.example.com:443");
        assert!(
            !code.contains(&server.port.to_string()),
            "plain port leaked: {code}"
        );
    }

    #[tokio::test]
    async fn desktop_invite_uses_the_runtime_port() {
        let mut server = cfg("127.0.0.1", Some("kronn.example"));
        server.runtime_port = Some(53591);
        let code = try_build_invite_code(&server).await.unwrap();
        assert_eq!(code, "kronn:anonymous@kronn.example:53591");
        assert_eq!(server.port, 3140);
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

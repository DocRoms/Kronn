# Auth & TLS — operator runbook

## Summary

Kronn ships with a Bearer-token auth scheme, with a localhost
auto-bypass enabled by default for self-hosted comfort. This page
covers:

1. How auth is gated today
2. The transport guard: Kronn is never exposed over plain HTTP — TLS
   terminated by a declared proxy, or nothing (KT-1596)
3. The early-opt-out for users who want a token even on loopback
   (`server.auth_strict_localhost`)
4. The migration path when in-process TLS lands

## Today's defaults

`~/.config/kronn/config.toml`:

```toml
[server]
auth_enabled = true            # toggleable from Settings UI
auth_strict_localhost = false  # localhost gets free pass (default)
```

The token itself (a random UUID generated on first boot) is stored
encrypted in the database since 0.14.3, not in `config.toml`; see
[key-management.md](key-management.md). `KRONN_AUTH_TOKEN` still
overrides it.

Behaviour:

- **Localhost requests** (`127.0.0.1`, `::1`, Docker bridge
  `172.16.0.0/12`): bypass auth — the user is at the same host.
- **Remote requests** (Tailscale, LAN, public): require
  `Authorization: Bearer <auth_token>`.
- **Health endpoint** `/api/health`: never requires auth (Docker
  healthcheck path).
- **WebSocket** `/api/ws`: handled by the WS handshake's invite-code
  check, not the middleware.

The token can be rotated at any time from `Settings → Auth`. The
endpoint is `POST /api/config/auth-token/regenerate` — localhost
access only, so a leaked token holder can't rotate it without first
landing on the box.

## Plain HTTP on the LAN: refused unless TLS is carried elsewhere

Kronn itself (the backend, and the nginx gateway on `:80`) speaks plain
HTTP. When it is reachable beyond loopback, the Bearer token and P2P invite
codes would cross the network in clear, so Kronn only accepts to be exposed
through one of two paths. Loopback-only use is never affected.

| Path | Status | How |
|------|--------|-----|
| **TLS terminated by a proxy** (nginx, Caddy, `tailscale serve`) | Shipped | Run a TLS-terminating reverse proxy **on the same machine**, then declare it: Settings › Identity › "TLS proxy in front of Kronn" (the declaration and the proxy's public `https://` address are stored in `config.toml` as `server.tls_terminated_by_proxy` / `server.public_origin`), or the environment: `KRONN_TLS_TERMINATED_BY_PROXY=1` + `KRONN_PUBLIC_ORIGIN=https://kronn.example.com`. It is a declaration by the operator: Kronn cannot verify the proxy is there. |
| **In-process TLS** (rustls, generated certificate, fingerprint carried in the P2P invite) | Not shipped | `boot_bind` / the toggle already take a `tls_enabled` input, currently always `false`. Generating and serving the certificate needs `rcgen` and `axum-server`, which are not in the workspace yet. Tracked by the reduced `TD-20260314-no-tls`. |

Tailscale: `tailscale serve --bg 3140` is a TLS-terminating proxy on loopback
(real certificates, `https://<machine>.<tailnet>.ts.net`), so it is the first
path with the machine's tailnet address as the public origin. Binding the
Tailscale interface directly is not offered: it would still leave plain HTTP
open on the physical LAN and the machine's own loopback address would stop
answering.

### Reaching a Kronn that runs inside WSL (contacts / P2P)

A native Kronn inside WSL2 in the default NAT mode advertises the VM's
internal address (`172.x`), which no other machine can route to. A contact
added from another machine stays `pending`: the check is reachability only
(`GET {peer}/api/health`), so an unreachable address is never accepted. Two
ways out:

1. **Tailscale (recommended).** Install it on both machines (inside WSL or on
   the Windows host), let the peer use the tailnet address, and put
   `tailscale serve --bg 3140` in front as described above. The address is
   stable across NAT, LAN and WSL, and it is the one `network-info` is built to
   advertise.
2. **Windows `netsh interface portproxy`.** Forward a port of the Windows host
   to the VM (`netsh interface portproxy add v4tov4 listenport=3140
   listenaddress=0.0.0.0 connectport=3140 connectaddress=<WSL address>`), open
   that port in Windows Firewall, then give the peer the Windows host's LAN
   address. The WSL address changes at each `wsl --shutdown`, so the rule has to
   be refreshed. The exposure rules above still apply: turn on "Allow
   connections from other devices" (Settings › Identity), keep authentication
   on, and declare the TLS proxy.

This paragraph assumes the default NAT mode; the mirrored mode that
[windows-wsl-agents.md](windows-wsl-agents.md) describes was not tried for
contacts.

### What each runtime does

| Situation | Native / desktop | Docker |
|-----------|------------------|--------|
| Loopback only | Binds as asked. | `KRONN_BIND` loopback: starts. |
| Exposure asked, **no** TLS and **no** declared proxy | The Settings toggle **refuses** (HTTP 409, with what is needed; nothing is written). A config that already asks for it (written by an older Kronn, by hand, or `KRONN_HOST`) **starts on `127.0.0.1`** and Settings shows a banner saying why: the owner is never locked out of the UI that can fix it. | The publish is outside the process: refuses to start, naming `KRONN_TLS_TERMINATED_BY_PROXY` and `KRONN_PUBLIC_ORIGIN`. |
| Exposure asked + **declared proxy** | Binds `127.0.0.1` (the proxy on this machine is the only way in, the P2P socket included). | Starts **only with `KRONN_BIND=127.0.0.1`**: the proxy on the Docker host reaches Kronn through loopback. A declared proxy with the plain port still published beyond loopback refuses to start (the port would keep serving the token in clear): a proxy on another machine, which needs that publish, is not supported until in-process TLS ships. |

Behind a declared proxy:

- **Invites advertise the proxy.** The code is `kronns:pseudo@host:port` on
  the public origin (`kronn:` stays plain http/ws; `kronns:` is https/wss). A
  declared proxy with no public origin has **no** invite code: the invite
  endpoint answers an error and the peer broadcasts carry an empty code, which
  peers refuse. A peer that predates `kronns:` rejects the code (fail-closed).
- **Loopback is no longer the owner.** Every proxied request arrives from
  `127.0.0.1`, so the loopback auth bypass would let the whole network in:
  natively, a declared proxy requires the token from loopback too (the same
  rule as `server.auth_strict_localhost`) and the WebSocket "local frontend"
  shortcut is withdrawn. Declaring from Settings creates the token if needed
  and hands it to the UI in the same reply; a browser or desktop webview that
  loses it must paste it again (Settings › Server & network access). **Docker is
  no exception**: a proxy on the host reaches nginx from the bridge gateway
  (`172.x.0.1`), which counts as local, and nginx's `X-Real-IP` then names that
  gateway, not the remote user. With a declared proxy no peer is ever the owner
  by address, Docker included: the token (or, for recovery, the local proof
  below) is required.
- The proxy must forward WebSocket upgrades (`/api/ws`) for P2P.
- **The browser origin is allowed exactly.** CORS and the WebSocket `Origin`
  check accept the declared public origin (and only it, read from the live
  config), so a remote user loading the UI through the proxy is not refused as
  a foreign page.
- **The declaration is owner-only.** `POST /api/config/network-exposure` is
  gated like the other config changes that widen access (it is in
  `PERMISSION_POSTS`): even with auth disabled
  (`KRONN_ALLOW_INSECURE_LAN=1`) a non-loopback peer needs the token, so a LAN
  client cannot declare a proxy or change the exposure.
- **A reset refuses while the proxy is declared**, in the config or by
  `KRONN_TLS_TERMINATED_BY_PROXY`. The reset rewrites `config.toml` key-only,
  which would forget a config declaration while the proxy keeps forwarding from
  `127.0.0.1` (the loopback bypass would return). Withdraw the declaration in
  Settings (and unset the variable) first.
- **Recovery behind a proxy needs a proof read at the machine.** While auth is
  locked (the stored token cannot be decrypted), the recovery routes
  (`/api/config/recovery/*`, `auth-token/regenerate`) are normally open to a
  loopback caller. Behind a proxy every caller is loopback, so they also need the
  `X-Kronn-Local-Proof` header, set to the content of the `local-proof` file in
  the Kronn data directory (owner-only, minted when a proxy is declared; a
  file shorter than 32 characters is never accepted and is re-minted). The
  locked screen links to this page; the web UI cannot send the header, so this
  is a `curl` recovery. From
  the machine: `curl -H "X-Kronn-Local-Proof: $(cat <data dir>/local-proof)" …`.
  The locked refusal (HTTP 423) says so.

While exposure is *blocked* (asked for, nothing carries TLS: a stale config or
`KRONN_HOST`), the boot is on loopback, **no invite code is handed out** (the
endpoint answers an error and peer broadcasts carry an empty code) and Settings
shows one notice, read from the effective bind (`KRONN_HOST` included).

`KRONN_ALLOW_INSECURE_LAN=1` (an *unauthenticated* API on a trusted LAN) does
**not** waive any of this: authentication and transport are checked separately.

The logic lives in `backend/src/core/net_expose.rs` (`boot_bind`,
`plain_http_exposure_refusal`, `proxy_declared_for`), called by the standalone
backend (`backend/src/main.rs`), the embedded desktop server
(`desktop/src-tauri/src/main.rs`) and the toggle
(`backend/src/api/setup.rs::set_network_exposure`). Tests:
`core::net_expose::tests`, `api_tests::network_exposure_*`, `api_tests::recovery_behind_a_declared_proxy_*`,
`api_tests::a_reset_is_refused_*`, `lib.rs::auth_tests` (owner-only gate),
`cors_policy` and `api::ws` origin tests (declared origin),
`api::contacts::tests` (proxy invites, none while blocked), the desktop
`embedded_*` tests and `IdentitySection.test.tsx`.

**Upgrading:** an install whose `config.toml` already says `host = "0.0.0.0"`
(the old toggle) now boots on loopback with a banner until a proxy is
declared; a Docker install published on the network (`KRONN_BIND` not
loopback) fails its next boot until it goes back to loopback
(`KRONN_BIND=127.0.0.1`) behind a proxy on the host that is declared with the two
variables. Existing `kronn:` contacts keep working.

## Strict-localhost opt-in

Set the flag in `config.toml`:

```toml
[server]
auth_strict_localhost = true
```

Reload the backend (`kronn restart` or kill the process). After this,
**every** request — including `127.0.0.1` — must present the Bearer
token. This is the recommended setting if:

- You run Kronn on a shared dev VM or container host where other
  unrelated processes might call `localhost:3140`.
- You're auditing the access trail (every call now appears with an
  Authorization header in your logs).
- You're getting ready to ship TLS and want to test the strict path
  before the cutover.

The trade-off: the desktop UI, CLI and Tauri apps must now embed
the token in their requests. The setup wizard already provisions one
on first boot, and the Tauri build reads it from the same
`config.toml` — so most users won't notice a difference.

## What changes when in-process TLS ships

The transport guard above *reduced* the old `TD-20260314-no-tls` debt entry
(it stays open): an exposed instance can no longer run in clear by accident,
but it needs a proxy you run yourself. In-process TLS itself (the second path
in the table) is the remaining step. Once
that's in place, the localhost auto-bypass becomes redundant — the
threat model it mitigated (HTTP-on-loopback) goes away. The plan:

1. **TLS lands first**, with the auto-bypass still in place. No
   regression for self-hosted users on day one.
2. **Bump the default of `auth_strict_localhost`** from `false` to
   `true` in a minor release. Users who explicitly set it to
   `false` keep the old behaviour; everyone else gets the strict
   path.
3. **Two minor versions later**, remove the bypass code entirely
   (`is_local_ip` becomes dead code; ditto the `auth_strict_localhost`
   flag). Auth becomes Bearer-only.

This is a slow, graceful deprecation — not a flag day. The migration
trigger is "TLS works for ≥ 80 % of self-hosted users" (we'll measure
via opt-in telemetry on the SetupWizard's `tls_enabled` flag).

## Security headers & Content-Security-Policy

The HTTP security headers (CSP, `X-Frame-Options`, `X-Content-Type-Options`,
`Referrer-Policy`) are set by the **nginx gateway**, NOT the Rust backend —
they live in **`.docker/nginx.conf`** (the `add_header … always;` block near
the top of the `server {}`). The desktop (Tauri) build instead serves the
frontend via `serve_app_documents`, whose only header is the frame-policy CSP
(`frame-src`/`child-src`/`worker-src`); it sends no COOP/COEP (KT-1123, see
`docs/architecture/live-pages.md`), so the directives below apply to the
Docker/self-hosted deployment.

The CSP is a single `add_header Content-Security-Policy "…" always;` line:

| Directive | Value | Why |
|-----------|-------|-----|
| `default-src` | `'self'` | Same-origin by default. |
| `script-src` | `'self' 'unsafe-inline' 'unsafe-eval' blob:` | Vite runtime + web workers (Whisper STT) need inline/eval/blob. |
| `style-src` | `'self' 'unsafe-inline'` | Inline component styles. |
| `img-src` | `'self' data: blob: https:` | App assets + data/blob previews + **any HTTPS image** — see note. |
| `connect-src` | `'self' ws: wss: https://huggingface.co https://cdn.jsdelivr.net https://cdnjs.cloudflare.com` | API + WS + model/lib downloads (Whisper, mermaid). |
| `worker-src` / `media-src` | `'self' blob:` | Web workers + TTS/STT audio blobs. |
| `font-src` | `'self' data:` | Bundled + data-URI fonts. |

**`img-src https:`** (widened in 0.8.6 from a `www.gravatar.com` allowlist):
the project-doc viewer renders README/markdown files that embed external
images — shields.io badges, screenshots on CDNs, etc. A host allowlist would
break them, and images can't execute code, so `https:` is an acceptable
relaxation. To harden a locked-down deployment, replace `https:` with an
explicit host list (expect external README images to stop rendering).

**Relative-path images** in a README (e.g. `docs/screenshots/foo.png`) are
served by the backend `doc-asset` route — image files only, confined to the
project root, size-capped — and the doc viewer rewrites their `src` to it.
Same-origin, so `img-src 'self'` covers them with no CSP change. See
`backend/src/api/ai_docs.rs::read_doc_asset` +
`frontend/src/lib/docImageRewrite.ts`.

## Test surface

Auth is regression-tested in `backend/src/lib.rs::auth_tests`:

- IPv4 + IPv6 loopback recognised as local
- `"localhost"` string explicitly rejected (defensive against
  forged `X-Real-IP`)
- Docker bridge `172.16-31` accepted
- Tailscale, LAN, public IPs rejected
- Malformed strings fail-closed (require auth)

When you flip `auth_strict_localhost`, all of these still pass — the
flag wraps the bypass in a single `if !strict_localhost` block, so
the predicate behaviour is unchanged.

## Related files

- `backend/src/lib.rs:auth_middleware` — gate
- `backend/src/lib.rs:is_local_ip` — IP classifier
- `backend/src/api/setup.rs:regenerate_auth_token` — rotation endpoint
- `frontend/src/pages/SettingsPage.tsx:1240-1275` — Settings UI
- `.docker/nginx.conf` — CSP + security headers (gateway, Docker mode)
- `desktop/src-tauri/src/main.rs:desktop_app` — desktop router (no COOP/COEP)
- `backend/src/core/net_expose.rs:boot_bind` — transport guard and bind plan (KT-1596)
- `backend/src/api/setup.rs:set_network_exposure` — the toggle that refuses plain HTTP

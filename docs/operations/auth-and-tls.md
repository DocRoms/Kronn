# Auth & TLS — operator runbook

## Summary

Kronn ships with a Bearer-token auth scheme, with a localhost
auto-bypass enabled by default for self-hosted comfort. This page
covers:

1. How auth is gated today
2. The early-opt-out for users who can't wait for TLS
   (`server.auth_strict_localhost`)
3. The migration path when TLS lands (TD-20260314-no-tls)

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

## What changes when TLS ships

`TD-20260314-no-tls` tracks the move to nginx-fronted TLS. Once
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
- `docs/tech-debt/TD-20260314-no-tls.md` (when filed) — TLS plan

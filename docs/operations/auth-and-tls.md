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
auth_token = "<random-uuid>"   # generated on first boot
auth_enabled = true            # toggleable from Settings UI
auth_strict_localhost = false  # localhost gets free pass (default)
```

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
- `docs/tech-debt/TD-20260314-no-tls.md` (when filed) — TLS plan

# Debug & operations (AI reference)

## Common commands

| Action | Command |
|--------|---------|
| Start full stack | `make start` |
| Stop all services | `make stop` |
| View logs | `make logs` |
| Rebuild all | `make build` |
| Dev backend (hot reload) | `make dev-backend` |
| Dev frontend (Vite) | `make dev-frontend` |
| Regenerate TS types | `make typegen` |
| Clean everything | `make clean` |

## Docker services

| Service | Internal port | External port | Image |
|---------|--------------|---------------|-------|
| backend | 3140 | — | Built from `backend/` |
| frontend | 80 | — | Built from `frontend/` |
| gateway | 80 | 3456 | nginx |

## Troubleshooting

### Gateway serves stale frontend assets
- **Symptom**: UI changes not visible after `docker compose build`.
- **Cause**: nginx gateway container caches old static files.
- **Fix**: `docker compose down && docker compose up -d` or `docker compose restart gateway` + `Ctrl+Shift+R` in browser.

### Axum panics on startup with route conflict
- **Symptom**: `thread 'main' panicked at 'overlapping routes'`
- **Cause**: Two `.route()` calls with the same path.
- **Fix**: Chain methods — `.route("/path", get(h1).post(h2))` instead of two separate calls.

### TypeScript type mismatch after model change
- **Symptom**: Frontend build fails with type errors on API responses.
- **Cause**: Rust models changed but `generated.ts` not regenerated.
- **Fix**: `make typegen` then rebuild frontend.

### SSE stream never completes (Stop button stuck)
- **Symptom**: Stop button stays visible after agent finishes.
- **Cause**: Missing `finished` guard in SSE handler, or `onDone` not called.
- **Fix**: Ensure `finished` boolean guard in `_streamSSE` and `orchestrate` in `api.ts`.

### Agent CLI not found
- **Symptom**: "command not found" errors in agent runner.
- **Cause**: Agent CLI not installed or not in PATH inside Docker container.
- **Fix**: Check Docker volume mounts for `~/.claude` and `~/.codex` in `docker-compose.yml`.

### HTTP 502 Bad Gateway in Docker
- **Symptom**: Frontend loads but all API calls fail with 502.
- **Cause**: Backend bound to `127.0.0.1` instead of `0.0.0.0`. In Docker, nginx is in a separate container and cannot reach `127.0.0.1` (loopback).
- **Fix**: Backend detects Docker via `KRONN_DATA_DIR` env var and forces `0.0.0.0` binding. If still failing, check that `KRONN_DATA_DIR` is set in `docker-compose.yml`.

### HTTP 401 Unauthorized after upgrade
- **Symptom**: All API calls return 401 after upgrading from a version that auto-generated auth tokens.
- **Cause**: Legacy `auth_token` in config.toml without the new `auth_enabled=true` flag.
- **Fix**: On startup, backend clears tokens where `auth_enabled=false` (migration). If persists, manually remove `auth_token` line from config.toml: `docker exec kronn-backend sed -i '/^auth_token/d' /data/config.toml && docker restart kronn-backend`.

### SQLite WAL issues on network/synced drives
- **Symptom**: Database lock errors, slow writes, or corruption.
- **Cause**: WAL mode doesn't work well on NFS, SMB, iCloud, or OneDrive drives.
- **Fix**: Set `KRONN_DB_WAL=0` in docker-compose.yml to use DELETE journal mode.

### Cross-platform: macOS firewall popup
- **Symptom**: macOS asks to allow incoming connections when running natively.
- **Cause**: Binding to `0.0.0.0` triggers macOS firewall.
- **Fix**: Keep default `127.0.0.1` for native execution (only bind `0.0.0.0` in Docker).

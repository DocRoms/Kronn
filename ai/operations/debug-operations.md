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
| backend | 3001 | — | Built from `backend/` |
| frontend | 5173 | — | Built from `frontend/` |
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

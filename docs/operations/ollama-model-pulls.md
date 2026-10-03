# Pulling Ollama models from Settings

`POST /api/ollama/pull` accepts only a model name and proxies Ollama's native
pull stream as server-sent events. Progress events expose the current status,
digest, completed bytes and optional total bytes; terminal success and errors
use separate event names. Connection/header, record-size, idle and event-count
limits terminate with an actionable error payload. [src: file: backend/src/api/ollama.rs:382-486]

The Settings card holds one `AbortController` per active model. Cancelling a
download aborts the browser request and removes that model's transient progress
state, so the same download can be started again. [src: file: frontend/src/components/settings/OllamaCard.tsx:225-288]

Updating an installed model uses the same endpoint and the same card state: the
Update button pulls the model's exact tag again (KT-930), so its progress,
cancellation and errors are the download's. Whether a newer version exists is
read from the official library without downloading (`GET /api/ollama/registry`).
The suggestions and their sizes, the update check, the Mac MLX builds and the
fold of the block are described in
[ollama-local-models.md](ollama-local-models.md#downloading-updating-and-mac-mlx-builds-from-settings-kt-930).

The streaming client accepts both the Ollama endpoint's `message` field and the
shared SSE limiter's backwards-compatible `error` field, and treats a clean EOF
without a terminal event as failure. [src: file: frontend/src/lib/api.ts:2480-2518]

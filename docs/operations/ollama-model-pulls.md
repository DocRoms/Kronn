# Pulling Ollama models from Settings

`POST /api/ollama/pull` accepts only a model name and proxies Ollama's native
pull stream as server-sent events. Progress events expose the current status,
digest, completed bytes and optional total bytes; terminal success and errors
use separate event names. Connection/header, record-size, idle and event-count
limits terminate with an actionable error payload. [src: file: backend/src/api/ollama.rs:382-486]

The Settings card holds one `AbortController` per active model. Cancelling a
download aborts the browser request and removes that model's transient progress
state, so the same download can be started again. [src: file: frontend/src/components/settings/OllamaCard.tsx:225-288]

The streaming client accepts both the Ollama endpoint's `message` field and the
shared SSE limiter's backwards-compatible `error` field, and treats a clean EOF
without a terminal event as failure. [src: file: frontend/src/lib/api.ts:2480-2518]

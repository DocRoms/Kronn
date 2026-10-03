# Startup request caching

Agent discovery uses stale-while-revalidate semantics. A fresh cached value is returned directly; an expired value is also returned directly while one shared refresh runs in the background. Concurrent cold callers wait for that same refresh instead of starting duplicate detection work. [src: file: backend/src/agents/mod.rs:310]

The frontend persists the last server-confirmed setup status and uses it as the initial application state. This lets the dashboard render while `/api/setup/status` refreshes in the background. [src: file: frontend/src/lib/appBoot.ts:23] [src: file: frontend/src/App.tsx:27]

Unsignalled GET requests share only in-flight work by default, so a real-time refresh after settlement always reaches the backend. A two-second post-resolution window is restricted to the explicit startup allowlist: server config, skills, agents, health, agent access and per-discussion native-agent mode. Mutations, API base changes and authorization changes invalidate all shared entries. [src: file: frontend/src/lib/api.ts:266-315] [src: file: frontend/src/lib/api.ts:697-705]

Same-origin Live Page links targeting `#discussion-…` or `#page/…` update the current tab's hash. The dashboard follows discussion hashes, while the application router follows Page hashes and swaps to the standalone Page without reloading the browser tab. [src: file: frontend/src/lib/live-page-sandbox.ts:559-568] [src: file: frontend/src/pages/Dashboard.tsx:222-232] [src: file: frontend/src/App.tsx:104-112] [src: file: frontend/src/App.tsx:196-205]

The Vite development `/api` proxy uses a protocol-matched keep-alive agent. The production nginx gateway already proxies API requests upstream over HTTP/1.1. [src: file: frontend/vite.config.ts:4-10] [src: file: frontend/vite.config.ts:52-56] [src: file: .docker/nginx.conf:56-60]

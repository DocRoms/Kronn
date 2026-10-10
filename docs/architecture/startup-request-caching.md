# Startup request caching

Agent discovery uses stale-while-revalidate semantics. A fresh cached value is returned directly; an expired value is also returned directly while one shared refresh runs in the background. Concurrent cold callers wait for that same refresh instead of starting duplicate detection work. [src: file: backend/src/agents/mod.rs:310]

The frontend persists the last server-confirmed setup status and uses it as the initial application state. This lets the dashboard render while `/api/setup/status` refreshes in the background. [src: file: frontend/src/lib/appBoot.ts:23] [src: file: frontend/src/App.tsx:27]

Unsignalled GET requests share only in-flight work by default, so a real-time refresh after settlement always reaches the backend. A two-second post-resolution window is restricted to the explicit startup allowlist: server config, skills, agents, health, agent access and per-discussion native-agent mode. Mutations, API base changes and authorization changes invalidate all shared entries. [src: file: frontend/src/lib/api.ts:266-315] [src: file: frontend/src/lib/api.ts:697-705]

Same-origin Live Page links to an address of the app (a legacy `#discussion-…` or `#page/…` hash included) are resolved to their canonical path and followed in the current tab, without reloading it; the router swaps to the standalone Page or lands on the discussion. See `docs/architecture/ui-structure.md` § Routing. [src: file: frontend/src/lib/live-page-sandbox.ts:560-584]

The Vite development `/api` proxy uses a protocol-matched keep-alive agent. The production nginx gateway already proxies API requests upstream over HTTP/1.1. [src: file: frontend/vite.config.ts:4-10] [src: file: frontend/vite.config.ts:52-56] [src: file: .docker/nginx.conf:56-60]

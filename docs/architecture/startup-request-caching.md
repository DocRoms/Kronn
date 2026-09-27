# Startup request caching

Agent discovery uses stale-while-revalidate semantics. A fresh cached value is returned directly; an expired value is also returned directly while one shared refresh runs in the background. Concurrent cold callers wait for that same refresh instead of starting duplicate detection work. [src: file: backend/src/agents/mod.rs:310]

The frontend persists the last server-confirmed setup status and uses it as the initial application state. This lets the dashboard render while `/api/setup/status` refreshes in the background. [src: file: frontend/src/lib/appBoot.ts:23] [src: file: frontend/src/App.tsx:27]

Unsignalled GET requests share both in-flight work and the result for a two-second startup window. Mutations, API base changes, and authorization changes invalidate that shared cache. [src: file: frontend/src/lib/api.ts:266] [src: file: frontend/src/lib/api.ts:645]

Same-origin Live Page links targeting `#discussion-…` or `#page/…` update the current tab's hash. The dashboard follows discussion hash changes without remounting the application. [src: file: frontend/src/lib/live-page-sandbox.ts:563] [src: file: frontend/src/pages/Dashboard.tsx:224]

# TD-20260713-graph-delegated-oauth

- **ID**: TD-20260713-graph-delegated-oauth
- **Area**: Backend / API broker / Workflows (scheduled ApiCall)
- **Problem (fact)**: The Daily Briefing workflow (`0935b1b2…`) and 3 Quick APIs
  (`Get Unread Emails`, `Get Today's Calendar`, `Send Teams Chat Message`)
  reference `api_plugin_slug: "@bundle:ms-graph"` — an atomic-bundle-creation
  SENTINEL (`backend/src/models/bundle.rs`), not a real plugin. It resolves only
  against an artifact declared in the same bundle payload; here it never was
  (external import, origin not attributable from this repo's history). Every
  cron tick has failed since at least 2026-06-29. The failure webhook of that
  workflow goes through the same broken Quick API, so the failures were silent.
- **Why we can't fix now (constraint)**: MS Graph `/me/...` and `/chats/...`
  endpoints require a **delegated** OAuth token. The registry's
  `mcp-microsoft-365` entry is an MCP transport (device-code auth, no
  `api_spec`) — not usable by ApiCall steps. Kronn's OAuth2 broker
  (`backend/src/core/oauth2_cache.rs`) only implements client-credentials /
  static-exchange caching; it neither acquires nor persists/rotates a delegated
  refresh token. Rebinding the Quick APIs to the M365 MCP is NOT equivalent and
  was explicitly rejected (Codex review, room 3f603a34 msg 153).
- **Impact**: the Daily Briefing (and any future scheduled Graph ApiCall) cannot
  run unattended; personal-scope Microsoft data is unreachable from
  désagentified steps.
- **Mitigation in place (2026-07-13)**: workflow cron DISABLED (was failing
  silently every weekday 09:15). Manual alternative: an Agent step using the
  M365 MCP after a device-code consent — no cron reliability promise.
- **Suggested direction (non-binding)**:
  1. Add an OAuth2 **authorization-code + refresh-token** flow to the broker:
     encrypted per-config refresh token storage, renewal/rotation on expiry,
     same envelope encryption as other secrets.
  2. Ship a bundled `api-ms-graph` ApiSpec (mail, calendarView, chat messages)
     usable by ApiCall steps once the delegated flow exists.
  3. Re-point the 3 Quick APIs to the new plugin; re-enable the cron; make the
     failure webhook use a channel that does not depend on the plugin under
     test.
- **Related watch item**: run `e3eb5c20…` (2026-07-12) hit `__guard_timeout__`
  at 1591 s against a 600 s limit with ZERO step results — the guard counts
  wall-clock across a daemon blockage/reboot and fired late. Not the same bug;
  keep an eye on `kronn::invariant` for recurrences (runner.rs guard loop).
- **Next step**: dedicated ticket/PR — explicitly OUT of the stab-1 scope.

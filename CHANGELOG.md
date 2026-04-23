# Changelog

All notable changes to Kronn will be documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.5.1] - 2026-04-23

Polish + playful additions on top of 0.5.0: the light theme got a proper
expert-led rework, a hidden-unlock system was added for early-access
testing, three secret themes ship with a small interactive layer, agents
can now generate PDF / DOCX / XLSX / CSV / PPTX files directly from the
conversation without the user installing anything, and the release
closes with three user-facing hygiene wins: an in-nav "active runs"
popover that one-clicks a running workflow to Stop, a first-class RTK
(Rust Token Killer) integration that cuts agent shell outputs by ~90%
before they hit the model, and a toast refactor that turns errors into
copyable, sticky notifications the user can actually read.

### Added
- **Active-runs popover in the nav** — When one or more workflow runs
  are in flight, clicking the Automatisation tab (which now spins a
  Loader2 icon + counter badge) no longer navigates to the list but
  opens a popover listing every live run with its project name, live
  elapsed timer (front-computed so it ticks every second without
  hitting the network), a one-click `⏹ Arrêter` button, and a
  "Voir tous les workflows" footer for the classical route. Stop
  feedback is immediate (button swaps to `⏳ Arrêt…` and disables),
  no modal confirm — the UX designer + a lambda-user persona both
  validated that friction-free kill is the whole point. `Esc` +
  click-outside close the popover; a tiny `stopPropagation` on the
  nav button's `mousedown` prevents the opening-click-closes-itself
  race. Runs-in-flight polling stays at 3 s (existing behavior),
  the popover reads from the same `workflowsApi.list()` cache.
  Already-on-the-page fallback: a matching `⏹ Stop` button inline on
  every `.wf-card` whose `last_run.status === Running | Pending`
- **RTK (Rust Token Killer) integration** — Kronn now detects, wires,
  and reports on RTK, a Rust shell-output compressor that cuts ~89%
  of tokens on commands like `git`, `cargo`, `ls`, test runners. The
  integration ships in three layers:
  - **Detection** (`backend/src/core/rtk_detect.rs`): extends
    `AgentDetection` with `rtk_available` (binary on PATH) and
    `rtk_hook_configured` (per-agent hook file scan). Paths come
    from RTK's own docs — Claude Code = `~/.claude/settings.json`,
    Codex = `~/.codex/AGENTS.md` (not `config.toml`, caught on the
    first iteration), Gemini = shell-rc scan (bash/zsh/fish/profile),
    rest = `None` (Kiro, Copilot CLI, Vibe, Ollama are not in RTK's
    supported list — the badge explicitly reads "Non pris en charge
    par RTK")
  - **Activation** (`POST /api/rtk/activate`): takes an `agents[]`
    body, filters to RTK-compatible types, and spawns **one**
    `rtk init -g <flag> --auto-patch [--hook-only]` per agent. The
    matrix was extracted from the RTK README and defensive-fixed
    after two iterations caught: (a) `rtk init -g` alone only covers
    Claude Code, not Codex / Gemini; (b) `--hook-only` can't be
    combined with `--codex` or `--gemini` (they *are* the hook).
    `--auto-patch` is mandatory for non-interactive — without it the
    command waits on a TTY prompt the backend can't answer and
    exits 0 having done nothing ("RTK activated" lying toast)
  - **Savings counter** (`GET /api/rtk/savings`): parses
    `rtk gain --all --format json`, digs into `summary.total_saved`
    / `summary.avg_savings_pct` / `summary.total_commands`. Real-
    payload test embedded as a regression guard — the first parser
    looked at non-existent top-level keys and systematically
    returned zero. Tolerant to JSON reshape: falls back on
    generic-name keys (`tokens_saved`, `ratio`, `sample_count`),
    `available: false` when anything goes wrong so the UI hides
    cleanly rather than showing a misleading "0 tokens saved"
- **CompressionSection component** — a single card at the top of
  Settings → Agents rendering "Mode économique" (rebranded from
  "RTK" for non-technical users, attribution kept in the
  footer: "Propulsé par RTK (open source)" → GitHub). Three states
  drive the CTA copy:
  - **0/N configured** → amber card, "Activer sur les N agents
    compatibles"
  - **partial** → neutral, "Activer sur les X restants"
  - **all configured** → green, no CTA, savings counter visible
  Plus an "Install RTK" modal when the binary isn't on PATH (copy-
  paste curl command + link to GitHub to reassure the tech
  colleague). A **(?) info button** next to the title reveals a
  sobriety-numérique note in italics — *"L'usage le plus sobre
  reste de ne pas utiliser d'IA. Si vous en utilisez, RTK
  compresse..."* — because claiming "eco mode" without caveats
  doesn't match the product's values
- **Per-agent RTK badge** in the agent list row: 🟢 `RTK actif`
  / 🟡 `RTK — hook non configuré` / ⚪ `RTK non installé` / italic
  `Non pris en charge par RTK` for Kiro / Copilot CLI / Vibe /
  unsupported agents. Each badge is a link to the RTK repo so the
  user can read what the thing actually does
- **"Détails" expand on the savings counter** — 3 stat cards
  (Tokens économisés, Ratio moyen, Commandes compressées) pulled
  from the same `GET /api/rtk/savings` call, hidden behind a chevron
  toggle so the minimal UI stays a one-liner until a user wants the
  breakdown
- **Dockerfile — RTK bundled in the backend image** — pinned 0.37.1,
  same `dpkg --print-architecture` pattern used for `glab` / `bun`
  / `uv`. Adds both `x86_64-unknown-linux-musl` and
  `aarch64-unknown-linux-musl` targets — the image is still
  single-arch at publish time but the `case` switch is arm64-ready
  for the first user on Apple Silicon self-host. New pre-created
  directory `/home/kronn/.config` chowned to the app user so RTK's
  own config writes don't trip a cross-uid permission wall
- **docker-compose RTK bind-mounts** — `~/.config/rtk`
  and `~/.local/share/rtk` mount into the container rw so
  `rtk gain` inside reads the same SQLite the user's shell wrote
  to on the host. Without these, the savings counter reports zero
  even with thousands of host-side compressions

- **Document generation — Kronn Docs** — Agents can now
  produce five file formats from a discussion without any external
  tooling on the user's side. Ships as a Python sidecar
  (`backend/sidecars/docs/`) spawned at backend startup: WeasyPrint for
  PDF, python-docx + BeautifulSoup for DOCX (HTML → Word mapping of
  headings / paragraphs / lists / tables / inline formatting), XlsxWriter
  for XLSX, stdlib `csv` for CSV, python-pptx for PPTX (Title+Content
  layout, bullets preferred over content-split). Sidecar binds to a
  random loopback port and prints `KRONN_DOCS_READY <port>` to stdout
  for deterministic startup. Rust side exposes
  `POST /api/docs/{pdf,docx,xlsx,csv,pptx}` and
  `GET /api/docs/file/:disc/:filename`; all five handlers go through a
  single `proxy_to_sidecar()` helper so adding a format = one arm.
  Filename sanitization (alphanumeric + `-_ ` only, UUID suffix,
  extension forced) + canonicalize check defend against path traversal.
  Graceful "Document sidecar unavailable — run `make docs-setup`" error
  when the venv isn't installed (the sidecar is opt-in, not a hard
  dependency). New skill `kronn-docs.md` tells the agent about the two
  fence conventions and the direct-API fallback. Auto-activation: the
  skill ships with `auto_triggers.common/fr/en/es` regex buckets that
  detect "génère un rapport PDF" / "create a presentation" / "exporta
  hoja xlsx" etc.; matched skills auto-inject into the system prompt
  (user can opt out per-skill in Settings → `auto_triggers.disabled`)
- **DocPreview component (HTML-based formats)** — when the agent wraps
  a full HTML document in a ```` ```kronn-doc-preview ```` fenced code
  block, the frontend intercepts it in `MessageBubble`'s
  `MarkdownContent` and renders a sandboxed iframe (empty `sandbox=""`
  — no scripts, no same-origin, no forms) with two export buttons
  below: 📄 PDF and 📝 DOCX. The same HTML is the payload for both
  endpoints — one preview, two formats. Per-format state with
  independent loading / ready / error rows so the user can export
  both and get two distinct download links
- **DocDataExport component (structured formats)** — a second fence
  ```` ```kronn-doc-data ```` carries a JSON payload with a `format`
  discriminator (`csv | xlsx | pptx`). No iframe — a spreadsheet or
  slide deck in an iframe looks worse than the real app — just a
  compact header (format + summary: row count / sheet count / slide
  count) and a single "Export" button. Malformed JSON or unknown
  format discriminator falls back cleanly to a regular `<pre>` so a
  broken message doesn't blow up the chat
- **Auto-trigger opt-out** — Settings → Skills gains a per-skill toggle
  backed by `POST/DELETE /api/auto-triggers`. Disabled skills stay
  visible but stop contributing to prompt injection even when their
  regexes match, letting the user neutralize a noisy auto-trigger
  without removing the skill itself
- **Secret unlock system** — A hidden area in Settings (only revealed
  after the Konami code `↑ ↑ ↓ ↓ ← → ← → B A` is entered on the page)
  exposes an input that accepts short codes. Codes are hashed
  server-side (`SHA-256` committed in `BUILT_IN_UNLOCK_HASHES`) and
  resolve to one or several unlocks — a code can unlock a theme, a
  profile, or **bundle both in one shot**. Shared with testers
  out-of-band. Operators can also add local plaintext overrides in
  `~/.config/kronn/config.toml` `[secret_themes]` for quick testing
  without a release. Generic `invalid code` error on miss so probers
  can't enumerate configured unlocks. Theme unlocks persist in
  `localStorage`; profile unlocks persist in
  `AppConfig.unlocked_profiles` (written to `config.toml` on success)
  so the profile survives restarts and shows up in `GET /api/profiles`
- **Three secret themes (hidden until unlocked)** — each adds a full
  `:root[data-theme="<name>"]` palette override:
  - **Matrix** — CRT phosphor aesthetic (near-black bg, phosphor green
    accent, green-biased text hierarchy, glow shadows). Ships with a
    JS decoding effect: titles scramble (katakana + digits + ASCII)
    then settle one-shot on mount and on occasional global pulses
    (8-14 s jitter, 15 % chance per title per pulse). The user's last
    message scrambles briefly when a discussion is opened
  - **Sakura** — pastel pink/purple blossom palette (warm white bg,
    hot-pink accent, dark-plum text). Ships with 6 falling `🌸`
    petals, each randomized on init (duration / drift / size /
    spin / opacity). Mouse proximity (≤ 90 px) pushes petals away
    with a proportional force — feels like breath on the sakura
  - **Gotham** — deep navy-noir bg with bat-signal yellow accent.
    Ships with an ambient bat-signal radial gradient drifting across
    the viewport (30 s round-trip) and 3 `🦇` that fly right-to-left
    with staggered delays so one appears every ~5-10 s
- **Batman profile — first secret built-in profile** — hidden until
  unlocked via the same input as themes (bundle code also unlocks the
  Gotham theme, so one unlock = profile + palette).
  `backend/src/profiles/batman.md` defines the persona in French: "le
  plus grand détective du monde", methodological investigator that
  collects physical evidence first, consults all configured MCPs +
  APIs, cross-checks against other repos via GitHub MCP / Context7,
  delegates to sub-agents as expert witnesses, and signs every report
  *"Je suis Batman."*. Surfaces `SECRET_PROFILE_IDS` in
  `core/profiles.rs` + visibility gating in `api/profiles.rs` so
  locked secret profiles 404 identically to missing ones —
  brute-forcing IDs reveals nothing
- **Light theme — properly accessible rebuild** — a full expert-led
  rework after the initial hasty refactor had the accent lime leak
  onto pale lime tints ("jaune sur vert clair" illegibility). New
  palette: teal-700 `#0f766e` accent (5.5:1 AA, chromatic cousin of
  the dark-theme lime on the cyan-yellow axis — same "électrique"
  tension without the contrast fail), cool-gray ramp
  (`#f6f7f9` / `#eceef2` / `#ffffff` with 11 % luminance delta so
  cards visibly lift), hovers bumped to `rgba(16,24,40,0.08+)` for
  WCAG 1.4.11 conformance on raised surfaces, shadow ramp doubled in
  thickness (two-layer with tight + wide), `--kr-text-on-accent`
  flipped to white in light (so black-on-dark-teal illegibility
  disappears). Every semantic hex flipped to Tailwind 600/700 range.
  `text-faint` merged with `text-muted` and `text-ghost` raised to
  4.5:1 to fix a fail flagged by the A11y audit where 10-11 px labels
  fell below AA normal text. Focus ring gets a
  `box-shadow: 0 0 0 4px rgba(accent, 0.22)` halo (WCAG 2.4.11 Focus
  Not Obscured Minimum)
- **`config.appearance` picker — unlocked themes appear inline** —
  once a secret theme is unlocked it slots in alongside system / light
  / dark with its own icon (Terminal for Matrix, Heart for Sakura,
  `🦇` for Gotham). The picker transparently re-uses the existing
  `set-choice-btn` styling
- **Theme-effects overlay infrastructure** — `<ThemeEffects />` mounts
  once at the app root and renders the right decorative layer based
  on the current theme. `pointer-events: none` + `aria-hidden="true"`
  on every sprite so clicks pass through and screen readers ignore
  them. `@media (prefers-reduced-motion: reduce)` disables all
  sprites + sidecar effects so the user keeps the palette without the
  motion
- **`useMatrixDecode` / `<MatrixText>`** — hook + wrapper component
  that replace a target string with a one-shot scramble animation
  (~470 ms) when the matrix theme is active, falling back to plain
  text otherwise. Applied to page `<h1>` headers, discussion titles
  in the sidebar, and the chat header title. Listens to the global
  `matrix:pulse` event for occasional re-scrambles so long-lived
  pages stay alive
- **Global `matrix:pulse` scheduler** — emitted from `<ThemeEffects />`
  every 8-14 s (jitter) while the matrix theme is active. Every
  `<MatrixText />` instance rolls a 15 % dice per pulse to decide
  whether to re-scramble — with ~20 visible titles, ~3 scrambles per
  pulse → sensation of aliveness without synchronized robotic mass
  updates
- **`useKonamiCode(onUnlock)` hook** — sequence-matching keyboard
  listener (accepts both lowercase and uppercase B/A, resets on wrong
  key but a fresh `↑` re-starts as step 1). Skips when the event
  target is `INPUT` / `TEXTAREA` / `contenteditable` so typing with
  arrow keys never advances the sequence accidentally. 6 unit tests
- **`kronn:profiles-changed` window event** — fired by the Secret Code
  submit handler whenever a profile unlock lands. The 4 consumers of
  `GET /api/profiles` (ProfilesSection, NewDiscussionForm,
  DiscussionsPage, WorkflowWizard) listen and refetch — Batman
  appears everywhere in real time without a page reload

### Changed
- **Toast system — errors are now persistent and copyable by
  default** — `useToast` gains a third argument
  `options?: { persistent?: boolean; copyable?: string }` and the
  defaults now differ per type: `success` = 3 s auto-dismiss, `info`
  = 5 s auto-dismiss, **`error` = sticky with a mandatory X close
  button**. When a `copyable` payload is passed, it renders below
  the title in a monospace `<pre>` (selectable, scrollable, max
  240 px) with a Copy button that swaps its icon to a Check for
  1 s on click. Matches a validated UX-expert + lambda-user pair:
  asymmetric treatment because a success confirms an action the
  user already took, whereas an error interrupts flow and needs
  diagnostic time. Hook file moved from `useToast.ts` →
  `useToast.tsx` (JSX + new dependencies `Copy` / `Check` / `X`
  from `lucide-react`), consumer API retro-compat
- **`POST /api/themes/unlock` response shape — `{ unlocks: [{kind, name}, ...] }`**
  (was single `{ theme }`). A single code may now match multiple rows
  in `BUILT_IN_UNLOCK_HASHES`, enabling bundles like
  kronnBatman → profile + theme together. Frontend `themes.unlock()`
  returns the array unchanged and the `ThemeContext` dispatches per
  kind. Operator-local plaintext theme overrides still resolve via
  `config.secret_themes`; profiles always go through built-in hashes
- **`AppConfig` gained `unlocked_profiles: Vec<String>`** + `secret_themes:
  HashMap<String, String>`, both `#[ts(skip)]` so codes never leak to
  the TypeScript bundle. `default_config()` initializes them empty.
  Zero migration — absence → default
- **Dark theme unchanged** — every light-theme fix is gated under
  `:root[data-theme="light"]`, the dark variables are untouched (the
  lime `#c8ff00` + black-on-lime contract stays 14:1 AAA)

### Fixed
- **RTK activation in Docker** — three successive bugs caught
  while iterating with a real user:
  1. First spawn passed no `--auto-patch` → `rtk init` waited on a
     TTY, exited 0, nothing happened. Frontend reported "RTK
     activated" falsely
  2. Second spawn overrode `HOME=$KRONN_HOST_HOME` (the *host*
     path e.g. `/home/priol`) which doesn't exist inside the
     container. RTK tried `mkdir /home/priol/.claude` and errored
     with "failed to create directory". The correct move is to
     leave HOME alone — the container already bind-mounts the
     right `.claude` / `.codex` / `.gemini` dirs from the host
  3. Third spawn added `--hook-only` to every agent. RTK rejects
     `--codex --hook-only` / `--gemini --hook-only` with
     "cannot be combined" because those flows *are* the hook. The
     flag now only applies to the Claude default command
- **Codex RTK detection path** — the first-pass detector looked
  in `.codex/config.toml`, RTK actually writes to
  `.codex/AGENTS.md`. Fixed with a regression test
  (`codex_reads_agents_md_not_config_toml`) that asserts
  AGENTS.md is the source of truth even when config.toml happens
  to mention RTK
- **`/api/rtk/savings` returned zero even with thousands of
  compressions** — the parser looked for `tokens_saved` at the
  JSON root but RTK 0.37 nests everything under
  `summary.{total_saved, avg_savings_pct, total_commands}`. The
  zero-returning path triggered the counter-hiding branch in the
  UI, so the section looked empty. Fix: navigate to `summary.*`
  with fallbacks on legacy keys + a regression test embedding the
  real user-provided JSON payload
- **Light theme — black text on dark accent illegibility** —
  `var(--kr-accent)` is now dark teal in light, so
  `color: var(--kr-text-on-accent) = #111` on it gave 3.5:1 (FAIL AA).
  User caught the regression on the pending-files badge and the
  "Créer avec IA" workflow CTA. Flipped `--kr-text-on-accent` to
  `#ffffff` in light, preserving 5.5:1 on the teal background while
  keeping 14:1 on the lime dark-theme fill
- **Light theme — sidebar blue cast** — the second-pass ramp
  (`#e3e4eb` for raised) read as "bleu" because `B=235 > R=223`.
  Neutralized to equal-channel gray `#e3e3e3` first, then the
  third-pass expert rework landed on `#eceef2` (3-pt cool tint,
  perceptually neutral)
- **Light theme — hovers indistinguishable from idle** —
  `rgba(0,0,0,0.06)` on raised gave 1.13:1 (FAIL WCAG 1.4.11 which
  requires 3:1 for state indicators). Bumped to
  `rgba(16,24,40,0.08)` on idle / `0.14` on strong / `0.18` on active
  — all pass 3:1 now
- **Light theme — bubble agent white-on-white** — white bubbles on
  near-white base (`#f5f6f8`) had no visual separation. Base darkened
  to `#f6f7f9` with 11 % delta to surface; agent bubble border
  tightened to `--kr-border-strong` + explicit `box-shadow` so the
  bubble now crisply lifts off the page
- **Light theme — active discussion almost identical to idle** —
  `.disc-item[data-active="true"]` used `rgba(accent, 0.06)` which
  gave near-imperceptible tint in light. Bumped to `0.18` + added
  `font-weight: 500`. Dark theme kept at `0.18` too (18 % lime tint
  on `#0e1117` reads distinctly without being harsh)
- **Light theme — flat neutrals `#ededed`/`#e3e3e3` read "dated"** —
  the R=G=B neutrals that were picked to kill the blue cast felt
  clinical per the UI audit. Final palette retains 3-pt cool tint
  (matches Linear / Vercel / Stripe dashboards without any "blue
  cast") — imperceptible at first glance but gives backgrounds a
  subtle alive feel

### Security
- **RUSTSEC-2026-0104 — `rustls-webpki` 0.103.12 → 0.103.13** in
  both `backend/Cargo.lock` and `desktop/src-tauri/Cargo.lock`.
  Reachable panic in CRL parsing; pulled transitively via `rustls`
  (→ `reqwest`, `hyper-rustls`, `tokio-tungstenite`, `quinn`). No API
  surface change
- **Direct `rand` dependency removed** — crypto.rs was the only
  consumer (`OsRng.fill_bytes()` for nonce + key generation), and
  `aes-gcm` already re-exports the required `RngCore` trait via its
  own `rand_core`. One less path to `rand 0.8` (RUSTSEC-2026-0097
  "unsound with a custom logger using `rand::rng()`"). A transitive
  path via `axum 0.7 → tungstenite 0.24` remains; it's a tolerated
  warning pending an axum 0.8 migration

### Developer experience
- **`make bump` keeps `Cargo.lock` in sync** — after editing the
  workspace-member `version` fields, the target now runs
  `cargo update --workspace --offline` on both backend and
  `desktop/src-tauri` (with an online fallback on cold-cache runs).
  Fixes a case where a fresh bump left `cargo check --locked`
  broken in CI
- **AI docs — secret-themes guide** — `ai/operations/secret-themes.md`
  explains the unlock system, how to add a new theme, how to add a
  new secret profile, and enumerates the security caveats (hash is
  SHA-256 unsalted on purpose so contributors can regenerate with
  `sha256sum`; codes ≥ 12 chars for dictionary resistance). Linked
  from `ai/index.md` Tier 1 table
- **Tests** — net **+35 backend** tests (secret-unlock: built-in hash
  path, bundle unlock persistence, secret-profile filter, locked-id
  404, hash determinism canary + RTK: binary detection, hook paths per
  agent incl. shell-rc scan for Gemini, regression for `.codex/
  AGENTS.md` not `config.toml`, unsupported agents always false,
  real-payload parse of `rtk gain --all --format json`, per-agent
  activation args) + **+79 frontend** (secret-unlock: Konami sequence,
  matrix decode edge cases, theme unlock bundle, tampered localStorage
  defense + workflow: ActiveRunsPopover 9 tests, inline Stop button on
  wf-card + stopPropagation + CompressionSection 16 tests covering
  the 3 states, install modal, sobriety tooltip, toast error stderr
  forwarding). Totals: **1178 backend lib / 756 frontend** — all
  green

---

## [0.5.0] — 2026-04-20

Major release: worktree test-mode UX, API plugins (Chartbeat as first), crash-recovery fixes, QP "Analyse de ticket Jira" hardening.

### Added
- **Plugins: kind = MCP | API | hybrid** — `mcp_servers` gains an `api_spec_json` column (migration 035) alongside the existing MCP transport. A plugin can declare a REST API capability via `ApiSpec { base_url, auth, endpoints, docs_url, config_keys }`, optionally alongside its MCP transport. Pure-API plugins use the new sentinel `McpTransport::ApiOnly`. Sync logic was taught to skip `ApiOnly` transports when writing `.mcp.json` / Vibe / Kiro / Gemini configs — their capability surfaces via prompt injection instead of disk files. Plugins UI gains per-card badges (`🔌 MCP` / `🌐 API` / `MCP + API` gradient) and a kind-filter pill row (`All | MCP | API`) next to the category pills. The add-plugin form reads `api_spec.config_keys` to (a) render non-secret keys as plain text with their own placeholders + descriptions, and (b) keep secret fields masked behind the eye toggle. Unlocks a "désagentification" roadmap: future workflow steps will call APIs directly without an agent. 5 new unit tests on the prompt-injection path + a regression guard on Chartbeat's endpoint set. Net ~750 LoC Rust + ~200 LoC React
- **Chartbeat — first API plugin** — full catalog entry `api-chartbeat` with 21 endpoints: Live dashboard API (`/live/dashapi/v4`, `/live/toppages/v4`, `/live/quickstats/v4`, referrers, geo, social, devices, video…) as synchronous GETs, and Historical API (`/historical/traffic/stats/{submit,status,fetch}/`, `/historical/traffic/series/…`, `/historical/dashapi/…`, topreferrers, authors, top_paths, sections, rankings) as 3-step async queries. Dedicated `default_context` explains the submit → status → fetch flow, includes a ready-to-paste polling loop, and warns about host vs sub-domain pitfalls (404 on `/historical/...` is usually a missing async flow, NOT an access/scope issue). Context is written to `ai/operations/mcp-servers/<slug>.md` on install — editable per-project. Auth is `apikey` query param; `CHARTBEAT_HOST` (non-secret) appears as a plain "Host (default)" field with a `domain.tld` placeholder — agents can override per-call when the user asks about a regional edition (e.g. `host=de.example.com`)
- **Adobe Analytics — second API plugin (OAuth2 S2S)** — `api-adobe-analytics` ships as the first plugin using the new `ApiAuthKind::OAuth2ClientCredentials` auth kind. Kronn mints + caches the bearer token automatically (exchange against Adobe IMS `/ims/token/v3`, 24h TTL with a 30s safety margin before refresh) so the agent never sees or handles the OAuth2 flow. 7 endpoints: `POST /reports` (the workhorse for pageviews × dimension × date range), `POST /reports/realtime`, `GET /dimensions`, `GET /metrics`, `GET /segments`, `GET /calculatedmetrics`, `GET /users/me` smoke test. Base URL templates `{ADOBE_COMPANY_ID}` into the path so the agent sees the tenant-scoped URL directly. Two mandatory extra headers (`x-api-key`, `x-proxy-global-company-id`) are surfaced via `OAuth2ExtraHeader.value_template` and interpolated at injection time. Full `default_context` with body-shape examples (pageviews-by-page, trended-by-minute, segment filters), rate-limit hints, and the top 5 pitfalls. Config keys exposed in the add-plugin form: `ADOBE_COMPANY_ID`, `ADOBE_ORG_ID`, `ADOBE_RSID` (non-secret); `ADOBE_CLIENT_ID` + `ADOBE_CLIENT_SECRET` masked. Unlocks Chartbeat × Adobe × Code × Fastly cross-analysis in a single discussion
- **Google Programmable Search — third API plugin** — `api-google-search` wraps the Custom Search JSON API. Simple `apikey=` query auth on the single `/customsearch/v1` endpoint (`https://www.googleapis.com/customsearch/v1`). `GOOGLE_SEARCH_CX` exposed as a non-secret `config_key` so users can duplicate the plugin for multiple Programmable Search Engines (site-scoped vs whole-web). `default_context` covers the full parameter matrix (`q`, `num`, `start`, `dateRestrict`, `siteSearch`, `searchType=image`, `lr`, `gl`…), the response shape (`items[].pagemap.metatags` for OpenGraph enrichment), three pre-composed curl snippets for common SEO use-cases (rank check, 7-day news window, site-scoped search), and warns loudly about the **100 queries/day free tier** + $5/1000 beyond + 10 000/day hard cap per project
- **`ApiAuthKind::OAuth2ClientCredentials` + in-memory token cache** — new auth variant carries `token_url`, `client_id_env`, `client_secret_env`, `scope`, and a `Vec<OAuth2ExtraHeader>` of provider-specific headers with `{ENV_KEY}` interpolation. Cache lives on `AppState.oauth2_cache` as `HashMap<config_id, CachedToken>` under `tokio::sync::Mutex` so concurrent discussion starts on the same plugin share one exchange. On restart the cache is lost (tokens are disposable); one HTTPS round-trip per active plugin on first use. Async resolver in `make_agent_stream` calls `core::oauth2_cache::resolve_token` for every OAuth2 plugin and injects the result under virtual env keys (`__access_token__` / `__token_error__`) — the sync `build_api_context_block` consumes those without knowing about the auth flow. Per-plugin isolation: one bad OAuth2 config doesn't hide other API plugins. 4 unit tests on cache behavior + 3 on the context render path (Adobe regression guard, template interpolation round-trip, token error surfacing). Generalizable to any future OAuth2 API (Google Analytics, Salesforce, etc.)
- **Base URL + header templating** — `ApiSpec.base_url` and `OAuth2ExtraHeader.value_template` now support `{ENV_KEY}` placeholders. Chartbeat's static URL is unchanged; Adobe's `https://analytics.adobe.io/api/{ADOBE_COMPANY_ID}` gets the live company ID substituted at render time. Missing keys render as `<NOT_CONFIGURED:KEY>` so the agent stops rather than firing a half-composed URL. Auth-guidance text adapts: *"Kronn refreshes this token automatically before it expires"* on success vs *"**TOKEN UNAVAILABLE — \<reason>**. Do not attempt API calls; tell the user and stop."* on failure — prevents unauthenticated 401 bursts when credentials are wrong
- **Worktree test-mode flow** — one-click UX wrapper around the existing `worktree-unlock/lock` endpoints. A `🧪 Tester cette version` CTA in the ChatHeader swaps the main repo to the discussion's branch + pauses the agent while the user tries the code in their IDE. A global banner (`TestModeBanner.tsx`) stays pinned at the top of the discussions page whenever any discussion is in test mode, with a single-click `Arrêter le test` button that restores the previous branch, pops the auto-stash, and re-creates the worktree. Triple preflight: (1) worktree clean, (2) main repo clean (opt-in `stash_dirty=true` or commit-first via the preflight modal), (3) HEAD not detached (force=true to override). Rollback on any checkout/stash failure — the user is never left in a half-switched state. New endpoints `POST /api/discussions/:id/test-mode/{enter,exit}` return a tagged envelope (`status: "ok" | "blocked"`) with per-kind details (`WorktreeDirty | MainDirty | Detached | …`). `TestModeModal.tsx` renders an action matrix per kind. Persistent across reboots via migration 034 (`test_mode_restore_branch`, `test_mode_stash_ref`). Dev-friendly subtext keeps git vocabulary visible alongside the user-friendly headline. 11 unit tests on the new worktree helpers + 5 integration + 13 component
- **Isolated-mode git-commit preamble** — `build_agent_prompt` now injects a worktree notice when `workspace_mode == "Isolated"` (localized fr/en/es). Agents running in a worktree get explicit instructions to `git add` + `git commit` at the end of their changes, with the branch name spelled out. Prevents the "agent modified files but the branch is empty" class of bug
- **Git-panel pending-files badge** — small accent-lime counter on the `GitBranch` icon in the ChatHeader shows N uncommitted files in the worktree (Isolated mode). Pulses on first render after an agent reply lands. Caps at `9+`; tooltip shows `"3 fichier(s) en attente de commit"`
- **Analyse de ticket Jira QP — hardened** — after auditing 3 real runs (EW-7223, EW-7141, EW-6071 — 71 messages total), rewrote the prompt template to eliminate ~25% of avoidable friction: mandatory pre-reads (`ai/templates/jira-ticket.md`, `ai/operations/confluence-doc.md`, skills), hard rules (framing-not-implementation, no-write-without-confirmation, `curl` REST v2 not MCP for description updates, valid transitions `To Frame → To Do`, code-reads budget 4-5 files), business-first lens ("quel problème BUSINESS est à résoudre ?" with rétrocompat example), 3-phase method (short tour d'horizon → deep dive → Jira wiki markup refacto)

### Changed
- **Profile injection always fires** — `start_agent_with_config` used to skip the profile prompt when a native `.claude/agents/*.md` file existed, on the assumption Claude Code would auto-load it. That assumption is false in `--print` mode: agent files there are only consulted after an explicit `@agent-name` mention. Result: `translator` profile activated but ignored. Now the compact persona injection always fires for API-capable agents, whether or not a native file exists. Added a regression guard on `build_profiles_prompt_compact` to ensure it always carries the persona name + role
- **Cancel workflow run — force DB status** — `POST /api/workflows/:id/runs/:run_id/cancel` now force-updates `workflow_runs.status = 'Cancelled'` in the DB when the in-memory cancel token is missing (runner crashed mid-await, backend restart, second-click after token already consumed). Returns `run_cancelled: true` as long as the row was actually rescued OR the token fired. Fixes the "nothing happens when I click stop" scenario on orphaned runs; 3 new integration tests
- **Agent stream: stdin for Claude Code prompt** — prompts now travel via `stdin` instead of argv on Claude Code, bypassing the Linux `ARG_MAX` per-argv cap (~128 KiB). Root cause of the "Spawn failed for npx: Argument list too long (os error 7)" on long conversations. `--append-system-prompt` is still argv-based but now truncates gracefully at 100 KiB with a `[... truncated to fit ARG_MAX ...]` marker. Doesn't affect other agents
- **Decoder-loop detection on agent stream** — Claude Opus with extended thinking can leak `</thinking>` tokens and get stuck repeating them (EW-7189 shipped 6349× closing tags into a single partial response). Two-layer defense: (1) parser-level strip of literal `<thinking>` / `</thinking>` tags before they reach `full_response`, (2) detection of ≥ 50 consecutive identical deltas of ≥ 3 non-whitespace chars → kill the child + footer "🔁 Decoder loop detected". Both the main stream loop and orchestration use the same mechanic. Size-cap safety net (2 MB) still in place as last resort
- **`cancel_registry` cleanup via forced row update** — workflow-run cancellation is no longer fragile to runner crashes / backend restarts; see "Cancel workflow run — force DB status" above
- **Plugins form — per-field metadata** — the env-key field in the add-plugin form now consults `api_spec.config_keys` FIRST for placeholders, then falls back to the static `ENV_PLACEHOLDERS` map. Any future API plugin gets meaningful form affordances (label, placeholder, inline description, no mask on non-secret keys) with zero frontend code change. Example: Chartbeat `Host (default)` field shows `domain.tld` placeholder + italic explanation underneath
- **`discussions.rs` — profile-ids regression fix** — 16 call sites that literally construct `Discussion {}` structs were updated for the new `test_mode_restore_branch` + `test_mode_stash_ref` fields via a scripted edit to avoid drift
- **Vendor-neutral tests + fixtures** — tests, placeholder values, and tech-debt notes no longer reference any real organization. All occurrences replaced with generic `example.com` / `acme.com` / `acme-frontend` / `your-company.atlassian.net`. CHANGELOG left as historical record

### Fixed
- **Fastly CLI not found inside Docker on WSL/Linux** — `npm i -g @fastly/cli` installs a JS wrapper script at `/usr/local/bin/fastly` that relative-symlinks to `../lib/node_modules/@fastly/cli/fastly.js`. Kronn mounted `/usr/local/bin` → `/host-bin/global` but NOT `/usr/local/lib`, so the symlink resolved to a non-existent path inside the container → "fastly CLI not found in PATH" when the Fastly MCP tried to shell out. Added `${KRONN_GLOBAL_LIB:-/usr/local/lib}:/host-bin/lib:ro` mount so the relative `../lib/node_modules/...` resolves correctly. Fastly CLI now runs transparently (`HOME=/host-home` picks up the user's `~/.config/fastly/` profile). Companion improvement: Fastly registry `default_context` now has a "FIRST IF fastly CLI not found" troubleshooting block, plus a traffic-correlation playbook for analytics-dip investigations (Chartbeat × Fastly hits vs cache_miss)
- **Chartbeat historical API auth was wrong in default_context** — the initial 0.5.0 Chartbeat context described `/historical/.../submit/` endpoints with `apikey=` query param. Actual Chartbeat API: historical/query endpoints require the `X-CB-AK` HEADER, the modern flow is `/query/v2/submit/page/` → `/status/` → `/fetch/`, and the legacy `/historical/traffic/series/` also accepts the header directly (often synchronously). Rewrote the context block + endpoint list based on the real Chartbeat API responses observed in production use. Also documents the 5-min live-granularity trick for short-window dip analysis (hourly historical misses sub-hour shape)
- **Test-mode modal readability** — initial CSS used undefined tokens (`--kr-bg-secondary`, `--kr-bg-primary`), which silently resolved to `transparent`, making the modal blend into the chat behind it. Replaced with real tokens (`--kr-bg-elevated`, `--kr-bg-overlay`). Primary-button hover lost the accent background because a generic `.test-mode-modal-btn:hover` rule (specificity 0,3,0) beat `.test-mode-modal-btn.primary` (0,2,0) — scoped the generic hover to `.ghost` and made the primary-hover explicit. Result: modal now has opaque `#161b22` background, hover stays on the accent lime
- **QP chain picker (⚡ button) — white-on-white hover** — the popover reused `.disc-mention-popover` / `.disc-mention-item` which rely on `data-highlighted` keyboard-nav state (never set on the QP picker) → no hover feedback at all, and the description text rendered almost invisible against the same bg color. Created dedicated `.disc-qp-picker-{item,icon,meta,name,desc}` classes with explicit `:hover` + `:focus-visible` state (accent tint) and a header row. Icon 20 px fixed column, name + description stacked with ellipsis / line-clamp
- **Orphaned workflow runs after crash / restart** — parent runs stuck at `status = 'Running'` with no in-memory token are now rescued by the second cancel click (see Changed above)
- **Decoder loop on Claude Opus extended thinking** — 76 KB of `</thinking>\n` accumulation no longer possible (see Changed above)
- **ARG_MAX / E2BIG on npx spawn** — stdin pipe fixes this (see Changed above)
- **Profile not applied when synced natively** — see Changed above
- **Test-mode CSS & UX polish** — modal background, hover readability, wording tweaks ("commit-les d'abord, ou demande à l'agent de le faire")

### Developer experience
- **Tests** — 988 backend lib + 157 integration + 648 frontend. Net +58 tests (+ 21 backend, + 13 frontend `TestModeBanner` / `TestModeModal`, + 5 context-injection, + 3 cancel-run, + 5 thinking-strip, + 4 OAuth2 cache, + 3 API-context render, + 1 Adobe regression guard, + 3 OAuth2-plugin isolation / sync-exclusion / multi-plugin token scoping)

---

## [0.4.2] — 2026-04-17

### Added
- **Discussion favorites / pin** — star icon in the ChatHeader (always visible, click to toggle). Pinned discussions appear in a dedicated "Favorites" section at the top of the sidebar, cross-project, sorted by last activity. Small `★` indicator on sidebar items for pinned discussions. Migration 033 adds `discussions.pinned` column. `PATCH /api/discussions/:id` accepts `{ pinned: bool }`
- **Unread badges on group headers** — the sidebar now shows an accent badge with the unread count on every group header (global, org, project), visible whether the group is collapsed or expanded. Previously, unread badges were only on individual discussion items (and clipped by overflow)
- **QP Chain — Phase 2 (workflow engine)** — `batch_chain_prompt_ids: string[]` on `WorkflowStep`. When a `BatchQuickPrompt` step fans out to N discussions, each child now runs the initial QP **then the full chain sequentially** inside the same conversation. The batch progress counter only bumps when the ENTIRE chain finishes for a given discussion. `spawn_agent_run_with_chain(state, disc_id, chain_ids, batch_item)` injects each chain QP's prompt as a User message between runs (author `⚡ <qp.name>`) and **renders the QP template with the same batch item value** (e.g. `"EW-1234"`) that the primary QP received — so `analyse → review → summary` on ticket `EW-1234` propagates the ticket ID through all three QPs. Chain QPs may have up to 1 variable (the first variable gets the batch item). Phase 1 (queue-a-QP-mid-stream in a single discussion) remains available for manual use
- **QP Chain — Workflow Wizard UI** — BatchQuickPrompt step form now has a "Chain more Quick Prompts (optional)" section. Chain QPs appear as ordered pills (`1. ⚡ Name`) with click-to-remove. Candidates = QPs with ≤ 1 variable (excluding the primary QP and already-chained). Hint explains the batch-item-value propagation. Also displayed as a labeled row in `WorkflowDetail` so configured chains are visible when inspecting an existing workflow
- **QP Chain UI — ChatInput picker** — while the agent is streaming, a ⚡ button next to ⏹ opens a popover of chainable QPs (those with no variables). The queued QP shows as a pulsing accent badge that click-to-cancels. Auto-fires on the `sending: true → false` edge. Extracted as the `useQpChain` hook (`frontend/src/hooks/useQpChain.ts`, 7 dedicated tests) — ref-based `onFire` pattern so callers don't need to memoize their send handler
- **rAF-batched stream writer hook** — `useRafBatchedStream` (`frontend/src/hooks/useRafBatchedStream.ts`, 5 tests). Collapses dozens of SSE token deltas per frame into one `setState` call. Extracted from `DiscussionsPage` (was inline there), now reusable for any future stream/chunk consumer
- **Custom skill editing** — Settings → Skills now shows a ✏️ edit button on every custom skill card (previously only delete was available, forcing delete+recreate for a typo fix). Reuses the create-form with prefilled values; submit dispatches to `skillsApi.update()` instead of `create()`. The markdown body is stripped of its frontmatter before populating the textarea so each edit round doesn't nest a new `---` block. New i18n keys `skills.editCustom` + `skills.saveChanges` (fr/en/es)
- **`.mcp.json` freshness guard** — `make_agent_stream` now re-syncs the project's `.mcp.json` to disk RIGHT BEFORE each agent run, plus logs an explicit warning if the file is missing when a project is set. Covers the case where MCPs were toggled/added after the last startup sync (notably hit batch discussions that spawned right after a new MCP config)
- **CLI: Ollama detection** — the CLI now detects Ollama as the 7th agent. Install via `curl -fsSL https://ollama.com/install.sh | sh`. Parallel arrays sized dynamically (no more "unbound variable" on new agents)
- **CLI: API-first hybrid mode** — when the backend is running, `kronn status`, `kronn agents`, `kronn projects` delegate to the REST API (instant, complete). Falls back to local shell detection when offline. New `lib/api-client.sh` wrapper with `kronn_api_available` probe + `kronn_api_show_agents` / `kronn_api_show_status` formatters
- **CLI: project action menu** — selecting a project now opens a sub-menu: Install template, Launch audit, Launch briefing, View MCPs, Open in dashboard. Actions adapt to audit state. Deep-link: "Open in dashboard" scrolls directly to the project card via `#project-<id>` hash
- **CLI: `--debug` auto-tails logs** — `./kronn start --debug` now streams logs automatically after boot. `./kronn logs` shows grep helpers. Help section explains where logs live
- **Dashboard deep-link** — `http://localhost:3140/#project-<id>` auto-expands and smooth-scrolls to the matching project card. Waits for project list to load before scrolling (double-rAF timing). Hash cleaned after consumption

### Changed
- **`discussions.rs` extracted** — the ~3400-line monolith was split: pure agent/text helpers moved to `api/disc_helpers.rs` (9 fns, 15 tests — `agent_prompt_budget`, `auth_mode_for`, `agent_display_name`, `smart_truncate`, `summary_msg_threshold/cooldown`, `is_compact_agent`, `language_instruction`, `estimate_extra_context_len`), and pure prompt builders moved to `api/disc_prompts.rs` (3 fns + `OrchestrationContext`, 9 tests — `build_agent_prompt`, `build_orchestration_prompt`, `build_synthesis_prompt`). `discussions.rs` is now ~2880 lines (**-15 %**, -518 lines). Behaviour unchanged — extraction is pure, tested in isolation, zero runtime diff
- **`DiscussionsPage.tsx` shrunk** — from 1783 → 1736 lines after extracting `useQpChain` and `useRafBatchedStream`. Same behaviour, cleaner separation of concerns
- **Settings → Skills card — pill overflow fix** — long skill names (e.g. `euronews-front-conventions`) used to push the `~XXX tok` badge out of the 220 px card. Header row now wraps gracefully: title stays on top, pill cluster (category + builtin/custom + token estimate) wraps below if space is tight. `overflow: hidden` on the card itself as a belt-and-suspenders
- **CLI: repo status display** — replaced verbose `ai/ 4 redirectors 6 MCPs .claude/` with compact dashboard-like format: `Validated · 9 MCPs` / `Audited · 6 MCPs` / `Template · 4 MCPs`. Color-coded by audit state (green/yellow/cyan/grey)
- **CLI boot reorder** — `./kronn start` now asks "web UI vs CLI" BEFORE running agent detection. The web UI has its own detection (instant via the backend API), so the ~5-10 s CLI sweep is skipped when the user picks web. Pure UX win on the most common path
- **CLI agent detection UX** — live progress line (`Scanning 3/7 — Vibe (Mistral)...`) replaced the frozen terminal. `show_detected_agents` prints every agent line immediately with a ⏳ placeholder, then updates each line in-place (ANSI `\033[NA` / `\033[NB` cursor moves) as the `npm view` update check returns. `check_agent_updates` (slow npm view × N) removed from the default flow — only runs when entering the manage-agents menu. Single-agent rescan after install/uninstall/update instead of full 7-agent sweep
- **CLI: `--version` timeout** — reduced from 5s to 3s with `</dev/null` to prevent agents that read stdin (Copilot, Kiro) from hanging indefinitely

### Fixed
- **Batch focus on sidebar** — clicking a Quick Prompt batch launch now passes the parent `batch_run_id` back to `Dashboard`, which threads it through `setFocusBatchId`. The sidebar auto-expands the project group + the batch group + scrolls to it after the refetch settles. Previously only the first discussion was selected with no batch-group visibility
- **Batch double-run** — `onBatchLaunched` used to set `setAutoRunDiscussionId`, triggering a second agent run for the first child (bug seen 2026-04-10: 7/6 ok on a 6-item batch). Now uses `setOpenDiscussionId` which opens without auto-running
- **Unread badge + pin star clipped by title overflow** — `.disc-item-title` had `overflow: hidden` which clipped flex children (badge, star) on long titles. Title text now truncates in its own `<span>` while badge/star remain outside the overflow zone
- **CLI: `make_args[@]: unbound variable` on macOS** — Bash 3.2 + `set -u` treats an empty array expansion as unbound. Fixed with `${make_args[@]+"${make_args[@]}"}` pattern (same as `remaining[@]` elsewhere)
- **CLI: Copilot hangs during detection** — `copilot --version` reads from stdin indefinitely. Fixed with `</dev/null` + 3s timeout. Agent still detected with version `?`
- **CLI: `_AGENT_LATESTS[$idx]: unbound variable`** — `detect_agents()` reset result arrays to 6 hardcoded elements after Ollama was added as 7th agent. Arrays now sized dynamically from `_AGENT_NAMES` length

### Infra
- **Tech-debt tracking** — three new detailed entries in `ai/tech-debt/`:
  - `TD-20260417-models-monolith.md` — `backend/src/models/mod.rs` (~2225 L, 147 types) — planned split into 15 sub-modules (15 helpers `default_*` scattered, needs dedicated session)
  - `TD-20260417-audit-monolith.md` — `backend/src/api/audit.rs` (~1966 L) — prerequisite: extract an `AuditEngine` abstraction before splitting handlers
  - `TD-20260417-projects-monolith.md` — `backend/src/api/projects.rs` (~1819 L) — sub-directory split (crud/bootstrap/clone/git/…), lowest-risk of the three
  - `TD-20260328-discussions-backend` status updated to partial-progress after the disc_helpers/disc_prompts extraction
- **`ai/` docs refresh** — `repo-map.md` LOC figures, `index.md` Last-updated date + version
- **CLI: project menu clears ANSI ghost lines** — `printf "\033[J"` after `menu_choice` + "Press Enter to continue" pause between action output and menu re-render. Fixes the "text printed on top of the old menu" visual glitch

### Tests
- Backend: **1090** (was 1026 in 0.4.1, **+64**) — +migration 033 coverage, +15 for `disc_helpers`, +9 for `disc_prompts`, +2 for `batch_chain_prompt_ids` (DB roundtrip + serde skip-if-empty)
- Frontend: **629** (was 610, **+19**) — 7 for `useQpChain`, 5 for `useRafBatchedStream`, 6 for `ChatInput` QP-chain picker, 1 for `SettingsPage` custom-skill edit button
- Shell: **195** (was 196 — 1 test removed during repos.sh refactor, 4 added for Ollama)
- Build: `pnpm build` ✅ · `cargo clippy -- -D warnings` ✅ · `tsc --noEmit` ✅

---

## [0.4.1] — 2026-04-15

### Added
- **Chat draft persistence** — the ChatInput textarea now survives tab/page navigation. Drafts are saved per-discussion in `localStorage['kronn:draft:<disc_id>']` (7-day TTL, schema-versioned, throttled 250 ms). On rehydration, a subtle "Brouillon restauré · écrit il y a X" badge shows relative time, auto-hides as soon as the user edits. New helper `lib/chat-drafts.ts` with `saveDraft` / `loadDraft` / `clearDraft` / `purgeExpiredDrafts`
- **Audit/briefing resume on navigation** — the AI audit no longer "disappears" when the user switches tabs. Backend `AuditTracker` gained a `progress` HashMap written by the 3 SSE streams (`run_audit`, `partial_audit`, `full_audit`) at each `start`/`step_start`/`done`/`cancelled`. New `GET /api/projects/:id/audit-status` endpoint. Frontend: `kronn:audit:<project_id>` checkpoint in localStorage, `ProjectCard` polls every 2 s on remount and repaints the progress bar without restarting the audit
- **MCP pulse hint on projects** — when a project has 0 plugins AND hasn't been audited yet (`NoTemplate` / `TemplateInstalled` / `Bootstrapped`), a pulsing `.dash-mcp-hint` callout invites the user to add plugins before launching briefing/audit. Respects `prefers-reduced-motion`
- **Emoji autocomplete in ChatInput** — typing `:ta` mid-sentence opens a ranked suggestion popover (`:tada:` 🎉, `:taco:` 🌮, …). Tab/Enter inserts the Unicode glyph directly (Discord/Slack UX). Mirrors the `@mention` keyboard model. Blocks false positives on timestamps (`12:30`) and URLs (`http://`). New `lib/emoji-autocomplete.ts` helper backed by `node-emoji`
- **Emoji shortcode rendering in messages** — `:shortcode:` in agent output is rendered as the Unicode glyph via `remark-emoji` inside `MarkdownContent`. Unknown shortcodes pass through verbatim (no silent data loss)
- **Syntax-highlighted diff viewer** — `GitPanel` diff view now highlights additions and context lines via `highlight.js` (core + 15 registered languages: TS/JS/Rust/Python/Go/Java/JSON/YAML/TOML/Markdown/CSS/HTML/Bash/SQL/XML). Deletions stay flat red — the point is what's being removed, not re-parsing stale code. Hunk headers, file meta (`diff --git`, `index …`, `+++`/`---`, renames, binary markers) rendered as dim italic chrome. Safe HTML injection via hljs-escaped output
- **In-memory log ringbuffer + live viewer** — every `tracing` event is captured into a 2000-line ringbuffer (`core::log_buffer`) via a custom `BufferLayer`. No file on disk, no Docker socket required. New endpoints `GET /api/debug/logs?lines=N` and `POST /api/debug/logs/clear`
- **Dedicated Debug settings card** — extracted from "Server & Security" into its own card between Server and Database in the Settings nav. Live log viewer (monospace, terminal vibe, 5-char level alignment) with Follow/Pause (auto-refresh 2 s + tail-f auto-scroll, respects user scroll), Refresh, Copy, Clear buttons. "N / 2000 lines" counter in header
- **"LIVE" visual indicator when debug mode is on** — pulsing red badge next to the Debug card title AND pulsing dot next to "Debug" in the Settings sidebar nav. Removes any ambiguity about whether verbose capture is active (the checkbox alone wasn't loud enough). Respects `prefers-reduced-motion`
- **Tracing init self-diagnostic** — first log line after boot now announces the filter in use (`tracing initialized — filter: kronn=debug,tower_http=debug`). Lets users confirm at a glance that debug_mode took effect
- **One-click "Report a bug on GitHub"** — button in the Debug card opens a new tab with a pre-filled GitHub issue (title `[Bug] Kronn v0.4.1 on macOS`, body with env info + agent summary + last 200 log lines in a collapsible `<details>`, `bug` label). Client-side redaction of common secret patterns (`sk-*`, `ghp_*`/`gho_*`/`ghs_*`/`ghu_*`, `AIza*`, `Bearer *`, JSON `password`/`token`/`api_key`/`secret`) before URL construction. Auto-trims log lines to stay under the 6000-char URL budget. Secondary "View existing issues" link to avoid duplicates
- **Debug mode** — `ServerConfig.debug_mode` persisted in `config.toml`. CLI `./kronn start --debug` writes `KRONN_RUST_LOG=kronn=debug,tower_http=debug` into `.env` for the current run (without touching config) AND auto-tails the logs after boot. `make start DEBUG=1` for direct use. `docker-compose.yml` now defaults `RUST_LOG` to `${KRONN_RUST_LOG:-}` (empty = backend picks based on `debug_mode`)
- **Diagnostic logs for cross-platform issues** — tagged `target: "kronn::agent_detect"` and `target: "kronn::scanner"`. `detect_all()` dumps env vars (HOST_OS / HOST_HOME / HOST_BIN / host_label) at sweep start + per-agent summary at end. `find_binary()` logs PATH + host_dirs, PATH hits, macOS skip reasons, final "NOT FOUND". `resolve_host_path()` logs each alias tried + success/failure + final decision. `scan_paths` logs ghost-path filter count
- **macOS APFS firmlink support** in the scanner — `resolve_host_path()` now tries 3 aliases for `/Users/X` paths: raw (`/Users/X`), APFS canonical (`/System/Volumes/Data/Users/X`), legacy (`/private/var/Users/X`). Prevents silent project-drop when a canonicalized path failed `strip_prefix`. New helper `host_home_aliases()`
- **`/api/health` enriched** with `version` (from `env!("CARGO_PKG_VERSION")`) and `host_os` (from `detect_host_label_public()`) for the bug-report flow. Docker healthcheck ignores the body — backwards-safe

### Changed
- **ChatInput remount on discussion switch** — `<ChatInput key={activeDiscussion.id}>` in `DiscussionsPage` forces a fresh mount per discussion. Guarantees the non-controlled textarea can never leak content across discussions (the root cause of the reported "same draft in all discussions" bug). Also resets voice mode / mention popover / emoji popover / draft hint cleanly at switch
- **macOS skip list extracted to `MACOS_HOST_BIN_SKIP` constant** in `agents/mod.rs`. The test `cross_agent_macos_skip_covers_npm_agents` now ENFORCES (no longer just documents) that every npm-installed agent is present — adding a new npm agent without updating the skip list is now a compile-time test failure
- **Emoji insertion format** — picking an emoji from the autocomplete inserts the Unicode glyph (🎉) into the textarea instead of the `:tada:` shortcode. Matches Discord/Slack UX where users see exactly what they picked. Agents still receive the glyph directly; `remark-emoji` handles the reverse direction for agent output using shortcodes

### Fixed
- **macOS — `gemini` never detected** — `gemini` was missing from the macOS skip list in `find_binary()`, so the host's Darwin `gemini` binary was mounted into the Linux container and failed to execute. Now covered by `MACOS_HOST_BIN_SKIP` + entrypoint installs
- **macOS — `gemini` + `copilot` never installed in the container** — `entrypoint.sh` only installed Linux versions of `kiro-cli`, `claude`, `codex`. Now also installs `@google/gemini-cli` and `@github/copilot` via npm when `KRONN_HOST_OS=macOS`
- **Chat draft lost on tab/page navigation** — ChatInput was re-rendered (not remounted) on discussion switch, and the non-controlled textarea kept its DOM value. Fixed by adding `key={activeDiscussion.id}` (see Changed above)
- **Chat draft bleed between discussions** — same root cause as above; the "same message in every discussion" bug is gone
- **`remark-emoji` + `node-emoji` install** — initial `npm install` created a parasitic `package-lock.json` alongside `pnpm-lock.yaml` and left `node-emoji` unhoisted (pnpm strict mode), breaking the Docker build. Both deps now declared explicitly via `pnpm add`
- **Debug viewer silently empty after restart** — `docker-compose.yml` resolves `RUST_LOG=${KRONN_RUST_LOG:-}` to an EMPTY STRING (not unset) when `KRONN_RUST_LOG` isn't defined, and `EnvFilter::try_from_default_env()` parses `""` into a filter that matches nothing — so flipping the debug toggle in Settings + restart produced zero captured events (stdout and BufferLayer both silenced). Fix in `main.rs`: treat whitespace-only `RUST_LOG` as "not set" so the `default_filter` derived from `config.server.debug_mode` kicks in

### Infra
- **`make bump V=x.y.z`** already existed — used to bump all 7 version files consistently (VERSION, Cargo.toml × 2, package.json × 2, tauri.conf.json, README)
- **`make start DEBUG=1`** new target helper (`_apply-debug-flag`) that writes `KRONN_RUST_LOG` into `.env`

### Tests
- Backend: **1054** (1047 lib + 147 integration at session start + ~10 new). New: `MACOS_HOST_BIN_SKIP` enforcement, `gemini`/`copilot` regression, `host_home_aliases` on 4 cases, `resolve_host_path` with aliases, `AuditTracker` progress (6 tests), `/api/projects/:id/audit-status` integration (3 tests), `log_buffer` (10 tests incl. tracing dispatcher end-to-end)
- Frontend: **610** (520 at session start + 90 new). New: `chat-drafts` (16), `ChatInput.draft` regression (8 incl. rerender-without-remount guard), `audit-resume` helper (10), `emoji-autocomplete` (18), `MessageBubble.emoji` (4 regression), `diff-syntax` (16), `bug-report` (18)
- Build: `pnpm build` (Dockerfile pipeline) ✅ · `cargo clippy --lib --tests -- -D warnings` ✅ · `tsc --noEmit` ✅

---

## [0.4.0] — 2026-04-14

### Added
- **Ollama local LLM integration** — new `AgentType::Ollama` for running local models (Llama, Gemma, Codestral, Qwen) at zero cost. HTTP API execution via `/api/chat` with separate system/user roles (model distinguishes MCP context from user question). Streaming output, token tracking (`prompt_eval_count` + `eval_count`). Health check (`GET /api/ollama/health`) with contextual hints per environment (native, Docker WSL, Docker macOS). Model listing (`GET /api/ollama/models`). `reqwest` stream feature added for HTTP response streaming
- **Ollama setup wizard in Settings** — 4-state inline card: not installed (OS-specific install commands + ollama.com link), offline/unreachable (contextual launch instructions for WSL/Linux/macOS), online with 0 models (4 suggested models with `ollama pull` commands + sizes), online with models (list of installed models with sizes). Refresh button for live status
- **Docker Ollama connectivity** — `OLLAMA_HOST` env var in docker-compose.yml. `extra_hosts: host.docker.internal:host-gateway` for Linux Docker. Contextual error message when Ollama listens on 127.0.0.1 only (WSL common issue)

### Changed
- **Ollama execution: CLI → HTTP API** — replaced `ollama run <model>` (single text blob, model confused MCP context with user question) with `POST /api/chat` (separate `role: system` for MCP/skills/profiles/directives context, `role: user` for the actual prompt). Fixes "response à côté de la plaque" issue with small models

### Tests
- Backend: **1187** (1040 lib + 147 integration). +2 Ollama endpoint tests, +5 cross-agent for 7 agents
- Frontend: **520** (41 suites). Cross-agent tests updated for 7 agents

---

## [0.3.7] — 2026-04-14

### Fixed (stability pass)
- **MCP whitelist migration** — `sync_claude_enabled_servers` now replaces the entire `enabledMcpjsonServers` whitelist instead of only adding entries. Fixes all MCPs broken by the `server.name` → `config.label` rename (Jira, GitHub, Slack, etc.). Stale entries cleaned up automatically
- **Project switch in discussions** — serde `Option<Option<String>>` can't distinguish JSON `null` from absent key. Frontend now sends `""` for "unset project", backend treats `""` as unset. Added try/catch on the PATCH call
- **Panic paths removed** — `lock().expect("poisoned")` → `match` + graceful break in agent stderr loop. 2× `unreachable!()` on `MessageRole::System` → returns `"System"`. 2× `disc.expect("is Some")` → match with SSE error response
- **Silent error swallowing** — 2 empty `catch {}` in AgentsSection (toggle, key sync) → error toast. 8 data-loading `.catch(() => {})` → `console.warn` with context. 6× `String(error)` → `userError()` in agent error handlers

### Changed
- **Package upgrades** — React 18→19, Vite 5→6, vitest 4.0→4.1, @vitejs/plugin-react 4→5, eslint 10.0→10.2, typescript-eslint 8.57→8.58, happy-dom 20.8→20.9. Only 2 lines of code changed (`useRef<T>()` → `useRef<T>(undefined)` for React 19 compat)
- **Settings accordion** — Agents, Skills, Profiles, Directives collapsed into a single card with 4 accordion sections (Agents open by default). Reduces vertical scroll by ~3 screens
- **Discussion form accordion** — Skills, Profiles, Directives in the new discussion form are collapsible (mutually exclusive). Selection count badge

### Added
- **Cross-agent regression tests** — 5 backend + 3 frontend parameterized tests that iterate ALL agent types. Auto-fail when a new agent is added without complete config (KNOWN_AGENTS, macOS skip, DB round-trip, color/label)
- **API smoke tests** — skills CRUD, directives list+CRUD, stats (tokens + agent-usage), quick_prompts CRUD, agents detect, disc_git (status + diff route), ai_files, discover_repos. +11 integration tests
- **Component smoke tests** — ChatInput, ProjectCard, WorkflowWizard render without crashing
- **Accessibility** — `aria-label` on ChatInput textarea, NewDiscForm selects (project + agent)

### Tests
- Backend: **1182** (1037 lib + 145 integration)
- Frontend: **520** (41 suites)
- Security: `cargo audit` clean (0 vuln), `pnpm audit` clean (0 vuln)

---

## [0.3.6] — 2026-04-14

### Added
- **Guided tour / onboarding overlay** — 15-step interactive walkthrough for new users, auto-launched on first visit. 3 interactive steps where the user clicks the real UI element (portal-rendered pulse animation, "Next" blocked until click). 5 acts with group labels (Projets → Plugins → Discussions → Automatisation → Config). Ends on Discussions page for action-oriented onboarding. Spotlight via box-shadow cutout, tooltip auto-positioned, mobile bottom-sheet. Keyboard: Escape/arrows. Replayable from "?" nav button or Settings. `kronn:tour-completed` localStorage persistence. Designed by consensus of 3 expert personas (PM Marie, UX Designer, Learning Scientist). 10 unit tests
- **Skill: structured-questions** — teaches agents the `{{var}}: question` format for structured Q&A. Bidirectional protocol: agent asks in `{{var}}: text` format → UI renders form → user replies as `var: value` lines → agent parses correctly. Category: domain
- **Profile: Translator / Teacher (Lin)** — contextual translation with vocabulary explanations. Translates with register awareness, explains idioms and jargon inline, treats each exchange as a micro-lesson. 17 builtin profiles total
- **macOS Docker agent bootstrap** — `entrypoint.sh` installs Linux `claude` + `codex` via npm on macOS hosts (Darwin binaries can't run in Linux container). Agent detection skips host-mounted Darwin binaries for `claude`, `codex`, `copilot`, `kiro-cli`. `~/.npm/bin` mounted via `KRONN_NPM_BIN` env var
- **Gemini CLI Docker mount** — `~/.gemini:/home/kronn/.gemini:rw` added to docker-compose.yml (was missing → Gemini crashed on agent switch with ENOENT on `projects.json`)
- **CI: desktop type-check** — `cargo check` of `desktop/src-tauri/` added to `ci-test.yml` to catch signature mismatches between backend lib and Tauri desktop app
- **Cross-agent regression tests** — 5 backend + 3 frontend parameterized tests that iterate over ALL agent types. Auto-fail when a new agent is added without complete configuration (KNOWN_AGENTS entry, macOS skip, DB round-trip, frontend color/label). Filet de sécurité pour ne plus casser un agent en en touchant un autre

### Changed
- **Settings: accordion for Agents & Skills** — Agents, Skills, Profiles, Directives collapsed into a single card with 4 accordion sections. Agents open by default, others collapsed. Reduces vertical scroll by ~3 screens
- **Discussions: accordion in advanced options** — Skills, Profiles, Directives in the new discussion form are now collapsible (mutually exclusive). Selection count badge. Same visual pattern as Settings
- **Tour step descriptions** — multiline text support (`white-space: pre-line`) for richer explanations. Step "3 façons de commencer" uses line breaks for clarity

### Fixed
- **Desktop Tauri build broken** — `AppState` and `WorkflowEngine::new()` signature updated in `desktop/src-tauri/src/main.rs` to match backend changes (removed `workflow_engine` field, added `cancel_registry`, `WorkflowEngine::new(state)` instead of `(db, config)`). Boot scans (orphan runs + partial recovery) added to desktop — were missing since 0.3.5
- **Project switch in discussions silently failing** — `Option<Option<String>>` serde bug: JSON `null` and absent key both deserialize as `None` (= no change). Frontend now sends `""` for "unset project", backend treats `""` as unset. Added try/catch + console.error on the PATCH call
- **Tour pulse animation invisible** — `box-shadow` on target element was hidden by parent stacking contexts (sticky nav). Pulse is now a separate portal div (`.tour-pulse-ring`) rendered above everything
- **Tour spotlight not cleaned up on step change** — `tour-target-elevated` class was not removed when transitioning to centered steps (welcome/finale). Fixed by calling `cleanupPrev()` before early returns in `useTourPositioning`
- **Tour backdrop blocking clicks on interactive steps** — `pointer-events: none` on backdrop during `waitForClick` steps so clicks reach the real UI element

### Tests
- Frontend: **517** (39 suites). +10 tour, +3 cross-agent consistency
- Backend: **1171** (1037 lib + 134 integration). +5 cross-agent regression (every_type_in_known_agents, definitions_complete, no_custom, macos_skip_covers_npm, db_round_trip_all_types)

---

## [0.3.5] — 2026-04-13

### Added
- **Batch Quick Prompts** — fan-out a Quick Prompt to N tickets/items in parallel. New step type `BatchQuickPrompt` with `batch_items_from` (resolves `{{steps.X.output}}` / `.data` / manual list), `batch_wait_for_completion`, `batch_run_worktrees`. Each child gets its own discussion, optional worktree isolation, aggregated in sidebar groups. Dry-run preview shows eligible items + warnings + per-item rendered prompt + one-click per-item test
- **Partial response recovery** — agent output is checkpointed every ~30s / ~100 chunks into `discussions.partial_response` (+ `partial_response_started_at`). On backend crash/restart, dangling partials are converted into Agent messages with an "⚠️ Réflexion interrompue" footer and `PartialResponseRecovered` WS broadcast. Migrations 031 (partial_response) + 032 (started_at) + 030 (workflow_run parent). `POST /api/discussions/:id/dismiss-partial` force-recovers on demand. `send_message` refuses a new run while a partial is pending (`partial_pending` SSE error) — prevents the 2026-04-13 double-response bug
- **Stop agent** — `POST /api/discussions/:id/stop` triggers a registered `CancellationToken` via `AppState.cancel_registry`. CancelGuard RAII pattern cleans the registry on agent completion. Frontend "⏹" button in chat header
- **Cancel workflow run (cascade)** — `POST /api/workflows/:id/runs/:run_id/cancel` cancels the linear run token AND cascades to every child batch discussion via `workflow_run.parent_run_id`. Child batch runs marked `Cancelled` in DB. Idempotent
- **Dry-run step test tracker** — module-level `activeStepTests` Map with subscribe/notify so in-flight step tests survive tab switches (React unmount). Each StepCard subscribes to its (workflowId, stepName, index) key
- **Workspace toggle always visible** — on new discussion form, the Direct/Isolated toggle is always shown when a project is selected; Isolated is disabled with tooltip when `repo_url` is null
- **UI locale persistence on Tauri WebView2** — backend stores `ui_language`, `stt_model`, `tts_voices` in config. `I18nContext` fetches from backend first with localStorage fallback, fixing the WebView2 localStorage wipe on Windows
- **SSE limits** — new `core/sse_limits.rs` module: global max concurrent SSE streams + per-client limit, configurable via `ServerConfig`
- **Cross-platform cmd helpers** — `core/cmd.rs` (`async_cmd`, `sync_cmd`) applies `CREATE_NO_WINDOW` on Windows. ALL `Command::new` calls routed through these to suppress flash-console windows on Tauri desktop
- **Structured agent questions** — `{{var}}: question` syntax parsed from the last agent message (`lib/agent-question-parse.ts`). When detected, a mini-form (`AgentQuestionForm.tsx`) renders above ChatInput with labeled fields for each variable. Submitting fills values and sends a formatted response. 15 parser tests + 5 component tests
- **Notify workflow step** — `StepType::Notify` with `NotifyConfig` (webhook URL, HTTP method POST/PUT/GET, optional body). Direct `reqwest` from Rust, zero tokens consumed. Template rendering in URL + body (`{{previous_step.output}}`, etc.). Frontend wizard form with method select + body textarea. 5 backend tests
- **5 new agent profiles** (total 16 builtins): Data Analyst (Ren), Data Engineer (Ash), SEO/Growth (Rio), SRE/DevOps (Ops), Staff Engineer (Dex)
- **Add project from local folder** — `POST /api/projects/add-folder` for non-git directories. Auto-detects `.git` if present. 3rd tab "Dossier local" in new project modal. 4 integration tests
- **Global context** — `ServerConfig.global_context` (markdown) + `global_context_mode` (always/no_project/never). Injected into agent prompts. `GET/POST /api/config/global-context` + `GET/POST /api/config/global-context-mode`. Settings UI with textarea + mode dropdown. 1 integration test

### Changed
- **Bootstrap-architect skill** — deeply rewritten for gated validation flow (architecture → plan → issues). +251 lines with clearer stage handoffs
- **Pagination** — `PaginationQuery.page` no longer has a `serde(default = 1)` — `Option<Query<_>>` now correctly falls through to unpaginated mode when no query params are sent. Regression fix for the 50-items silent cap
- **Settings UX** — section reorder (Usage before Server & Database), export warning redesigned (proper CSS class, "tokens d'authentification" consistent wording, clickable link scrolls to Server section)
- **Contrast & accessibility** — all inline `rgba(255,255,255,0.2-0.3)` replaced with CSS tokens (`--kr-text-dim`, `--kr-text-ghost`, `--kr-cancelled`). Token values raised from 0.2/0.3 to 0.35/0.45 for better readability. 8 icon-only buttons gained `aria-label`. Advanced toggle gained `aria-expanded`
- **Error messages humanized** — new `userError()` helper wraps raw `String(e)` in user-friendly messages (network, timeout, 413, generic fallback). 4 `alert()` calls replaced with `toast()`. Covers Dashboard, DiscussionsPage, WorkflowsPage
- **Hints rewritten for non-dev users** — batch worktree, agent question form, global context hints rewritten to explain WHY not HOW (FR/EN/ES)
- **Terminology consistency** — "clés API" vs "token API" confusion resolved in reset confirm dialog (FR/EN/ES). Distinction: "clés des fournisseurs IA" + "token d'authentification"

### Fixed
- **50-items silent pagination cap** — regression test added: creating 60 discussions and calling plain `GET /api/discussions` returns all 60 (not 50)
- **Double agent response after backend restart** — `partial_response_started_at` preserved across checkpoints so the recovered message sits chronologically before any later user message. `send_message` blocks while a partial is pending
- **Dry-run test state lost on tab switch** — module-level tracker owns the AbortController; components re-subscribe on mount
- **i18n placeholder mismatches** — new parity test caught 6 EN keys with dangling `{N}` placeholders (literal `{2}` rendered in UI)
- **Clippy** — `doc_lazy_continuation` in `models/mod.rs`, `manual_pattern_char_comparison` in `workflows/batch_step.rs`
- **macOS Docker: Claude Code not detected** — host-mounted macOS (Darwin) binaries can't execute in the Linux container. `entrypoint.sh` now bootstraps Linux `claude` + `codex` via npm on macOS hosts (same pattern as existing Kiro curl install). Agent detection skips Darwin binaries for all npm agents (`claude`, `codex`, `copilot`, `kiro-cli`). `~/.npm/bin` mounted + added to container PATH via `KRONN_NPM_BIN` env var (auto-detected by Makefile)
- **NewDiscussionForm: Escape + click-outside** — modal now closes on Escape key and overlay click (standard UX pattern)
- **NewDiscussionForm: double-submit prevention** — create button disabled after first click
- **AgentQuestionForm: Ctrl+Enter to submit** — keyboard shortcut + visual hint badge
- **Empty state projects** — text rewritten to guide user toward + button (add folder / clone / bootstrap)

### Tests (robustness pass)
- Backend: **1166** (1032 lib + 134 integration)
- Frontend: **504** (38 suites). New helpers + tests:
  - `src/test/apiMock.ts` — shared `buildApiMock()` factory (all 13 namespaces + 5 flat fns) + completeness test (ns coverage, flat-fn coverage, deep-merge preserves siblings)
  - `src/lib/__tests__/i18n-parity.test.ts` — 9 tests asserting fr/en/es key isomorphism + non-empty values + placeholder-subset invariant
  - `src/components/workflows/__tests__/BatchItemsList.test.tsx` — 6 tests (render, toggle prompt, dry-run forwarding, no-agent hides btn, running disables btn, defensive empty-prompt)
- `dictionaries` + `BatchItemsList` exported from their modules for testability

### DB migrations
- `030_workflow_run_parent.sql` — `workflow_runs.parent_run_id` for batch fan-out linkage
- `031_partial_response.sql` — `discussions.partial_response` (TEXT, nullable)
- `032_partial_response_started_at.sql` — preserves checkpoint start time across updates

---

## [0.3.4] — 2026-04-08

### Added
- **Quick Prompts** — reusable prompt templates with `{{variables}}` and conditional sections `{{#var}}text{{/var}}`. New tab "Quick Prompts" in the Automation page. Launch creates a discussion with rendered prompt and dynamic title. Full CRUD API + DB migration
- **MCP registry: 4 new MCPs** — MongoDB (official), Kubernetes (Red Hat), Qdrant (vector DB), Perplexity (AI search)
- **MCP Microsoft 365** — Outlook, Teams, OneDrive, OneNote via Softeria community server (device code flow auth)
- **MCP env var placeholders** — realistic hints for 30+ env vars + eye toggle on add form
- **Bootstrap++** — enhanced project creation with gated validation. New skill `bootstrap-architect` guides through 3 stages: architecture analysis → project plan → issue creation. Each stage requires user validation via CTA banner. Drag & drop document upload in the bootstrap modal (architecture docs, specs, PRDs). Uploaded files injected as context for the agent
- **WSL project discovery** — Windows Tauri app now auto-discovers WSL home directories for repo scanning

### Changed
- **Page title** — "Workflows" renamed to "Automatisation" (the page now contains Workflows + Quick Prompts tabs)
- **MCP registry** — Puppeteer removed (use Playwright), Google Analytics publisher corrected to "Community", Docker MCPs mention Docker requirement in help
- **MCP category pills** — fixed: filtering by category now works correctly (separated category selection from text search)
- **Setup wizard** — skeleton loader during agent detection, optimistic toggle (no rescan), animated completing state, parallel agent detection + repo scan (tokio::join)
- **Scan button** — loading state + toast feedback ("N new projects detected")
- **Reset config** — confirmation dialog with data loss warning

### Fixed
- **WSL scan paths** — `default_scan_path()` now returns WSL home on Windows native, scan always includes WSL homes
- **Setup wizard completion loop** — fast path for setup/status when already complete

---

## [0.3.3] — 2026-04-07

### Added
- **Export/Import ZIP cross-OS** — export as ZIP (data.json + config.toml sans secrets), import with config merge (pseudo, bio, language, scan_paths), path remapping for invalid project paths, contacts included in export (version 3). Retrocompatible with JSON v2 imports
- **Project path remapping** — `POST /api/projects/:id/remap-path` to fix project paths after cross-OS migration. Invalid paths flagged with warning toast after import
- **Workflow AI Architect** — new builtin skill `workflow-architect` + "Create with AI" button on Workflows page. Opens an interactive discussion where the AI designs, optimizes, and deploys a workflow. Agent emits `KRONN:WORKFLOW_READY` signal → one-click CTA creates the workflow
- **Test individual workflow steps** — `POST /api/workflows/test-step` with dry-run mode (agent reads but doesn't write). Live streaming output in the UI with elapsed timer. "Tester" button on each step card
- **Workflow starter templates** — 6 clickable examples in the simple wizard (Code Review, Changelog, Tech Debt, Test Coverage, Doc Update, Security Scan). Pre-fill name + prompt on click
- **MCP env var placeholders** — realistic hints for 30+ common env vars (Jira, GitHub, Slack, etc.) + eye toggle visibility on the add form
- **Setup wizard improvements** — WSL/Windows host label badge on agents, enable/disable toggle per agent
- **Stale-while-revalidate** — `useApi` hook keeps previous data visible during refetches, new `initialLoading` flag for first-load skeleton

### Changed
- **Export format** — now ZIP instead of raw JSON. Version bumped to 3. Includes `config.toml` (without auth_token/encryption_secret/API key values)
- **Workflow project_id** — can now be changed on existing workflows (was locked)
- **Workflow step prompts** — expandable with "Show more/less" toggle (was truncated to 200 chars)
- **Raw cron editor** — complex cron expressions (e.g. `0 7,10,13,16,19 * * 1-5`) preserved as raw strings instead of being mangled by the simple parser

### Fixed
- **Setup wizard completion loop** — clicking "Go to Dashboard" after skipping repos no longer loops back (setScanPaths with default path)
- **Setup status performance** — fast path skips filesystem scan when setup is already complete (10-30s → <1s on WSL paths)
- **Workflow project_id persistence** — SQL UPDATE was missing project_id column + serde double-Option deserialization fix
- **WSL agent detection fallback** — probe `~/.local/bin`, `~/.kiro/bin` when `bash -lc which` fails (non-interactive shell guard)
- **Flash of empty state** — projects/discussions no longer flash empty during refetches

---

## [0.3.2] — 2026-04-03

### Added
- **MCP default contexts** — new `default_context` field on `McpDefinition`. Registry MCPs can ship pre-filled context files (best practices, token-saving tips) written automatically to `ai/operations/mcp-servers/<slug>.md` on first install. Fastly is the first MCP with a default context (result pagination, JSON format, common commands)
- **MCP setup help i18n** — MCP setup instructions (`token_help`) can now be overridden per-locale via `mcp.help.<id>` i18n keys. Fastly and GitLab have dedicated help texts in fr/en/es
- **Claude Code settings sync** — `sync_claude_enabled_servers()` ensures Claude Code's `settings.local.json` whitelist (`enabledMcpjsonServers`) stays in sync with `.mcp.json`. MCPs added via Kronn are automatically added to the whitelist. Fixes a silent bug where Claude Code ignored MCPs not in its internal whitelist (bug #24657)
- **MCP publisher & origin badges** — new `publisher` (string) and `official` (bool) fields on `McpDefinition`. Registry cards and detail panels show "Officiel — Fastly" (green) or "Communautaire — Anthropic" (orange). All 49 registry entries classified
- **MCP load indicator** — per-project MCP count badge in scope toggles (green 1–5, orange 6–10, red 11+). Helps avoid agent slowdown from too many MCPs
- **MCP alt_packages matching** — new `alt_packages` field on `McpDefinition` allows the registry to recognize alternative package names for the same MCP server (e.g. npm `fastly-mcp-server` → registry `fastly-mcp`). Prevents duplicate `detected:*` entries when users have a different runtime than the registry default

### Changed
- **Fastly MCP → official Go server** — replaced the community npm package (`fastly-mcp-server`, required Bun) with the official Fastly MCP binary (`fastly-mcp`). Auth via Fastly CLI profiles (`fastly profile create`), no env var needed
- **GitLab MCP → official glab CLI** — replaced the archived Anthropic npm package (`@modelcontextprotocol/server-gitlab`, SDK 1.0.1 incompatible with modern Claude Code) with GitLab's official CLI (`glab mcp serve`). Auth via `GITLAB_TOKEN` + `GITLAB_HOST` env vars (stored encrypted in Kronn), supports self-hosted instances
- **MCP plugin detail panel** — setup instructions (`token_help`) and token link (`token_url`) are now displayed separately. URLs in help text are clickable. Setup section shown even for MCPs without env vars (e.g. Fastly, GitLab, Docker)
- **Codex MCP timeout** — npx/uvx-based MCP servers now get 60s startup timeout (was 30s). Fixes cold-start timeouts when packages are downloaded for the first time

### Fixed
- **GitLab MCP broken with Claude Code** — archived Anthropic package (`@modelcontextprotocol/server-gitlab`) uses SDK 1.0.1 which hangs on `notifications/initialized` sent by modern Claude Code. Replaced with `glab mcp serve` (official GitLab CLI)
- **Fastly MCP 401** — community npm package required Bun runtime for `execute` tool. Migrated to official Go binary that works standalone
- **MCP scan duplicate configs** — `match_registry_entry()` and `migrate_detected_to_registry()` now use `alt_packages` for cross-runtime matching (npx vs Go binary). `dedup_configs()` merges configs with same label+server_id (catches post-migration duplicates). Fixes 3x Fastly and 2x GitLab entries after sync
- **Stale project-level Codex config** — removed orphan `front_euronews/.codex/config.toml` that overrode the Kronn-managed global config with stale names and missing MCPs

---

## [0.3.1] — 2026-04-01

### Added
- **Usage dashboard** — new "Usage" section in Settings with summary cards (total tokens, estimated cost, discussions, workflows), provider breakdown bar, per-project horizontal bars, and daily history chart (30 days, stacked by provider). Toggle between token count and USD cost view. Filter by discussions, workflows, or all
- **Per-message cost tracking** — `cost_usd` column on `messages` table (migration 024). Real cost captured from Claude Code's `result` stream event; fallback to static pricing estimation for other providers
- **Static pricing engine** — `core/pricing.rs` with per-provider token pricing (Anthropic, OpenAI, Google, Mistral, Amazon). Used when real cost is unavailable
- **Daily usage history API** — `GET /api/stats/tokens` now returns `daily_history` with per-day token/cost breakdown by provider (last 30 days)
- **Discussion deep-link from Usage** — clicking a discussion name in the Usage top-5 list navigates directly to the discussion page and opens it
- **GitHub Copilot agent** — 7th supported agent (`copilot` CLI). Detected, installed, updated, and uninstalled via both web UI and Kronn CLI. Model tiers: economy (`gpt-4o-mini`), reasoning (`o4-mini`). Auth via `GH_TOKEN`, `COPILOT_GITHUB_TOKEN`, or `~/.copilot/config.json`. Full access flag: `--allow-all-tools`
- **Context files** — upload files (text, xlsx, docx, pptx, pdf, images) as context for discussions. Drag & drop, clipboard paste, or file picker. Extracted text injected into agent prompt. Images saved to project dir for agent vision tools. Max 20 files, 500KB text / 10MB images
- **User bio** — optional bio in Settings > Identity. Injected at the start of the first message in each new discussion so agents tailor responses to the user's profile

### Changed
- **Usage centralized in Settings** — the per-agent "Estimated token usage" section in Config > Agents has been removed. All usage data is now in the dedicated Usage section with richer visualizations
- **`StreamJsonEvent::Usage`** — `cost_usd: Option<f64>` integrated directly into the `Usage` variant; the separate `Cost` variant has been removed

### Fixed
- **Cross-platform audit** — 17 fixes for Windows/macOS/Linux/WSL/Docker compatibility: HOME/USERPROFILE resolution, `.cmd`/`.exe` binary detection, `WSL_DISTRO_NAME` detection, hostname fallback, Makefile BSD compatibility, UNC path normalization, conditional `SHELL` env var
- **First message identity** — Gravatar and pseudo were missing from the first message of a discussion (create handler didn't load identity from config)
- **AppImage removed** — Linux desktop builds now produce only `.deb` (19MB) instead of `.deb` + `.AppImage` (90MB)

---

## [0.3.0] — 2026-03-31

### Added
- **Workflow suggestions from MCP introspection** — `GET /api/projects/:id/workflow-suggestions` matches installed MCPs against a catalogue of 10 workflow templates (orphan PR detection, sprint digest, changelog, stale PRs, bug reports, PR quality, 5xx correlation, sprint brief, perf monitoring, doc sync). Each suggestion includes multi-step prompts, pre-filled trigger, and audience tag (dev/pm/ops)
- **Suggestion panel in workflow wizard** — sparkle button shows contextual workflow suggestions when a project with MCPs is selected. "Activate" (simple mode) or "Import as draft" (advanced mode). Multi-step or advanced suggestions auto-switch to advanced mode
- **Workflow wizard: simple mode** — new 3-step wizard (Infos, Task, Summary) alongside the existing 5-step advanced mode. Toggle at the top of the wizard. Simple mode: one agent, one prompt, manual or scheduled trigger
- **Scheduled trigger in simple mode** — "Manual" or "Schedule" toggle with visual frequency picker (every X minutes/hours/days). Converts to cron behind the scenes
- **System tray (desktop)** — closing the window hides to tray instead of quitting. Backend + workflow scheduler keep running. Double-click tray icon to reopen. "Quit" in tray menu for real exit
- **Wake lock (desktop)** — when cron workflows are active, prevents OS sleep. Windows: `SetThreadExecutionState`. macOS: `caffeinate -w`. Auto-releases when no cron workflows remain
- **MCP audit introspection (step 8)** — audit now calls read-only MCP tools to discover capabilities (tool inventory, project context: Jira projects, GitHub repos, Slack channels, etc.) and documents them in `ai/operations/mcp-servers/`. Generates workflow automation hints table
- **MCP drift auto-detection** — adding/removing/relinking a plugin on an audited project invalidates the `.mcp.json` checksum, flagging drift for step 8 re-run
- **Ad-hoc codesigning for macOS** — CI applies `codesign --force --deep -s -` when no Apple Developer certificate is configured. Release notes include `xattr -cr` instructions

### Changed
- **MCP renamed to "Plugins"** — all user-facing labels (FR/EN/ES), nav tab, page title ("Plugins (MCP / API)"), icons (Server -> Puzzle). Internal code keys unchanged
- **Plugin registry: card grid with category pills** — replaces the flat scrollable list. Cards with icon, name, description (2-line clamp), "Setup required" label. Category filter pills matching Config tab style (border-radius: 20px)
- **Installed plugins: inline expand** — click a plugin card to expand the detail panel in-place (grid-column: 1/-1), no CLS. Shows tokens, scope toggles, project links. Replaces the old accordion-by-server and the above/below detail panel
- **Plugin detail from project page** — clicking a plugin in ProjectCard navigates to Plugins tab and opens the detail panel for that specific config
- **Workflow wizard: advanced options hidden** — concurrency, workspace hooks moved behind "Advanced" toggle in the Config step. Per-step settings (model, retry, stall timeout) were already behind a toggle
- **Audit templates enriched** — `TEMPLATE.md` adds Capabilities table (tools, read-only flag, use-cases) and Project context section. `mcp-servers.md` adds Key capabilities column and Workflow automation hints table

### Breaking (internal)
- **Structured inter-step contract** — new `StepOutputFormat` enum (`FreeText` | `Structured`) on `WorkflowStep`. When `Structured`: engine auto-injects `---STEP_OUTPUT---` envelope instructions, extracts JSON envelope (`{data, status, summary}`) from output, exposes `{{previous_step.data}}`, `{{previous_step.summary}}`, `{{previous_step.status}}` in addition to raw `{{previous_step.output}}`. Includes repair prompt fallback when LLM doesn't comply. Existing workflows unaffected (default = `FreeText`)
- **Catalogue multi-step prompts** — all 10 workflow templates now have 2-4 specialized steps. Collection steps use `Structured` format with explicit data schema in the prompt. Synthesis steps use `FreeText`. Steps reference `{{previous_step.data}}` for structured data instead of raw output

### Fixed
- **Fastly MCP broken** — `fastly-mcp-server` v2.0.x switched to bun runtime. Pinned to v1.0.4 (Node.js) in registry + all 21 `.mcp.json` files across 7 repos. Backend test `pinned_packages_are_respected` prevents regression
- **`PINNED_PACKAGES` dead_code warning** — moved constant into `#[cfg(test)]` module
- **ProjectCard: Server icon → Puzzle** — consistent with Plugins rename

---

## [0.2.2] — 2026-03-29

### Added
- **Contact network diagnostics** — when adding a contact that's unreachable, the API now diagnoses the cause (Tailscale not active, LAN mismatch, peer offline) and returns a machine-readable code. Frontend shows a contextual toast instead of a generic error (i18n FR/EN/ES)

### Fixed
- **Windows: console windows flashing** — every background command (git, agent detection, npx probes, etc.) spawned a visible cmd.exe window on the Tauri desktop app. New `core::cmd` module applies `CREATE_NO_WINDOW` flag to all 50+ `Command::new` calls across the codebase
- **WSL agents not detected** — `wsl.exe -e which` doesn't load the user's login profile, so npm-installed agents (`~/.local/bin/claude`, etc.) were invisible. Now uses `bash -lc` for correct PATH resolution. Version detection also runs via `wsl.exe` for WSL binary paths
- **WSL repositories not scanned** — git commands failed on `\\wsl.localhost\...` UNC paths because Windows `git.exe` doesn't handle them. Git now runs inside WSL via `wsl.exe -e bash -lc "git -C ..."` for WSL filesystem paths. Scan timeout increased from 10s to 30s for WSL paths (9P filesystem is slow)
- **Desktop/self-hosted: "Cannot connect to backend"** — auth middleware relied on `X-Real-IP` header (set by nginx) to detect localhost. In Tauri desktop mode (no nginx proxy), all requests were treated as remote → 401 Unauthorized. Now also checks the direct peer IP via `ConnectInfo`. Startup timeout increased from 5s to 15s. Frontend auto-retries 5 times (2s interval) before showing the error screen
- **macOS CI codesign crash** — empty `APPLE_CERTIFICATE` secret was still exported as an env var, making Tauri attempt to import a null certificate. Signing env vars are now only exported when non-empty
- **Stale installers in CI artifacts** — cargo cache persisted old `.exe`/`.msi`/`.dmg` files across builds. Bundle directory is now cleaned before each build

### Changed
- **Setup wizard: all steps are now optional** — agents and repository detection steps can be skipped (button switches to "Passer cette étape"). Enables non-developer use cases: global discussions without projects, project creation without git repos
- **App icon** — new Lucide Zap lightning bolt icon (`#c8ff00` on `#0a0c10`) matching the web UI. Generated via `cargo tauri icon` from SVG source. Replaces the old generic icon across all platforms (ICO, ICNS, PNG, Windows Store logos)
- **`core::cmd` module** — centralized `async_cmd()` / `sync_cmd()` helpers replace raw `Command::new()` everywhere (agents, scanner, worktree, git ops, workflows, tailscale, checksums, audit). Single place to enforce cross-platform command behavior
- **WSL host label** — agents found via WSL now show "WSL" instead of "Windows" in the setup wizard (new `via_wsl` flag on `BinaryLocation`)

---

## [0.2.1] — 2026-03-28

### Fixed
- **WS security: first message must be Presence** — non-Presence first messages are now rejected, preventing invite code verification bypass (found by multi-agent audit)
- **Tauri desktop: blank page** — `extract_dir` doubled subdirectory paths (`assets/assets/index.js`). Fix: always use root target for path resolution
- **macOS CI build** — removed `|| ''` fallback on Apple signing secrets that caused empty certificate import to fail
- **Localhost exempt documented as tech debt** — `TD-20260328-localhost-exempt` with rotation plan

---

## [0.2.0] — 2026-03-28

### Added
- **Multi-user P2P chat** — share discussions between Kronn instances via WebSocket. Replicated model: each peer stores a full copy, messages sync in real-time
- **`POST /api/discussions/:id/share`** — share a discussion with contacts, broadcasts `DiscussionInvite` via WS
- **`WsMessage::ChatMessage`** — real-time message relay between peers with idempotent insertion (no duplicates)
- **`WsMessage::DiscussionInvite`** — auto-creates local discussion when a peer shares with you
- **Auto-add peers** — unknown but valid invite codes are auto-accepted as pending contacts (no mutual-add required)
- **Host IP detection for Docker** — `KRONN_HOST_IPS` env var, detected at `make start`, passed to container for accurate invite codes
- **Native skill files** — SKILL.md written to `.claude/skills/`, `.agents/skills/` (Codex), `.gemini/skills/` for progressive agent discovery (~95% token savings vs prompt injection)
- **Native agent profiles** — profiles synced as `.claude/agents/`, `.gemini/agents/`, `.codex/agents/` files
- **CSS design system** — `tokens.css` (83 CSS variables), `utilities.css`, `components.css` + per-page CSS files
- **Pagination API** — `?page=1&per_page=50` on discussions list and workflow runs (backward compatible)
- **Auth by default** — auto-generated Bearer token at first launch. Localhost exempt (no lock-out risk). Peers require token. WS auth via invite code
- **Share button** — in chat header, pick a contact to share the discussion with
- **Shared badge** — green Users icon on shared discussions in sidebar
- **Network feedback** — orange "pending" badge + tooltip on unreachable contacts, "offline" label for disconnected accepted contacts

### Changed
- **DiscussionsPage split** — 3254 → 1218 lines + 6 extracted components (ChatHeader, ChatInput, DiscussionSidebar, NewDiscussionForm, MessageBubble, SwipeableDiscItem)
- **SettingsPage split** — 1944 → 990 lines + 3 sections (AgentsSection, IdentitySection, ProfilesSection)
- **WorkflowsPage split** — 1780 → 373 lines + 3 components (WorkflowWizard, WorkflowDetail, RunDetail)
- **Dashboard split** — 1478 → 674 lines + 2 components (ProjectList, ProjectCard)
- **Backend split** — `projects.rs` 3823 → 1396 + `audit.rs` + `ai_docs.rs` + `discover.rs`. `discussions.rs` 3696 → 2322 + `disc_git.rs`
- **Inline styles extraction** — 1157 → 182 inline styles (dynamic only). All static styles moved to CSS
- **Prompt optimization** — native SKILL.md files use progressive disclosure instead of injecting full content. ~25 token reference prompt vs ~800 tokens full injection
- **WS endpoint** — skips auth middleware (invite code verification in ws.rs instead)
- **Tauri desktop app** — frontend files embedded in binary via `include_dir!` (fixes 404 on Windows/macOS installs)
- **Windows Tauri + WSL** — agents detected and executed via `wsl.exe -e` when running on Windows native. Windows paths auto-converted to WSL paths

### Fixed
- **TTS no sound** — added `media-src blob:` to nginx CSP (audio blobs were silently blocked)
- **Tailscale badge** — now conditional on `advertised_host === tailscale_ip` (badge stayed when switching to LAN IP)
- **French accents** — ~120 i18n strings corrected (détecté, sélectionné, créer, réseau, etc.)
- **Spanish accents** — ~90 i18n strings corrected (configuración, validación, código, etc.)
- **Discussion CTA from Projects** — clicking a discussion in ProjectCard now correctly opens it (was missing `onOpenDiscussion(disc.id)`)
- **Discussion visibility on navigate** — `ensureDiscussionVisible` now waits for `allDiscussions` to load before expanding sidebar groups
- **Test stability** — added `act()` flush in `wrap()` helper across 4 test files to reduce flaky failures

---

## [0.1.2] — 2026-03-25

### Added
- **Worktree unlock/lock** — manual button next to the branch badge to release/re-create the worktree. Lets you `git checkout` the branch in your main repo for testing without archiving the discussion
- **Auto re-lock** — when resuming a discussion whose worktree was unlocked, the worktree is automatically re-created (blocks if the branch is still checked out in the main repo)
- **API endpoints** — `POST /discussions/:id/worktree-unlock` and `POST /discussions/:id/worktree-lock`
- **Git signoff by default** — all commits now include `-s` (Signed-off-by), good practice at zero cost

### Changed
- **Worktrees in project directory** — worktrees are now created in `.kronn-worktrees/` inside the repo instead of `/data/workspaces/` in the Docker container. Visible from the host IDE (PHPStorm, VS Code, etc.)
- **Relative gitdir paths** — worktree cross-references use relative paths so they work both inside Docker and on the host
- **Startup migration** — existing worktrees at `/data/workspaces/` are automatically migrated to the new location on startup

### Fixed
- **GPG sign crash** — `--no-gpg-sign` is now passed when the user does not enable `-S`, preventing failures when `commit.gpgsign=true` is set in the git config but the signing key is missing
- **Worktree gitdir broken on host** — `.git` files in worktrees contained Docker-internal absolute paths (`/host-home/...`), now rewritten to relative paths
- **Branch checkout conflict** — clear error message when the branch is already checked out in the main repo instead of a cryptic git error

---

## [0.1.1] — 2026-03-25

### Added
- **MCP: draw.io** — official jgraph server added to registry (49 built-in servers)
- **MCP popover search** — filter + max-height scroll when > 6 MCPs (Discussions page)
- **MCP context file** — `ai/operations/mcp-servers/drawio.md`
- **Installation guide** — `docs/install.md` (Linux, macOS, Windows/WSL2)
- **ErrorBoundary per zone** — each Dashboard page (Projects, MCPs, Workflows, Discussions, Settings) has its own error boundary with inline retry
- **WorkflowStep metadata** — new `step_type` (Agent/ApiCall) and `description` fields on workflow steps, visible in wizard and summary. Prepares for future de-agentification of mechanical steps
- **Shell completions** — bash and zsh autocompletion for `kronn` CLI commands, auto-installed on first run
- **`make bump V=x.y.z`** — centralized version bump across all files (VERSION, Cargo.toml, package.json, tauri.conf.json, README)
- **CHANGELOG.md** — this file

### Changed
- **orchestrate() refactor** — extracted `run_agent_streaming()` and `run_agent_collect()` helpers, reducing orchestrate from ~625 to ~427 lines
- **Version centralized** — single `VERSION` file at repo root; shell, Rust (`env!`), and frontend (`package.json` import) read from it dynamically
- **Git push/PR: auto-token injection** — GitHub token resolved from MCP configs (encrypted in DB), injected into `gh` and `git push` automatically. SSH URLs rewritten to HTTPS with embedded token — no `gh auth login` or `export GITHUB_TOKEN` needed
- **PR creation: auto-push** — `Create PR` automatically pushes the branch if no upstream exists
- Installation docs simplified: agent install is handled by Kronn's setup wizard, not manual npm commands
- **Workflow runner** — replaced `run.clone()` with lightweight `RunProgressSnapshot`, avoids cloning full run state on every step
- **Error hints** — removed outdated French-only comment (messages were already in English)
- **Multi-arch Docker** — confirmed all Dockerfiles already support amd64 + arm64 natively (base images + arch-aware installs)
- **Zero `as any`** — eliminated all 12 `as any` casts across frontend (workers + tests), replaced with proper types (`VoiceId`, `AutomaticSpeechRecognitionPipeline`, `AgentType`, `AiAuditStatus`, `ToastFn`, `UILocale`)

### Fixed
- **Discussion badge desync** — unseen badge showed false positives when switching away from a discussion with an active agent stream
- **SSH on macOS** — git push now works on macOS Docker Desktop via `/run/host-services/ssh-auth.sock` forwarding
- **`.kronn-tmp/` polluting git status** — added to `.gitignore` + global git excludes in container; retroactive fix on startup for existing projects
- **`.kronn-worktrees/` not gitignored** — same treatment as `.kronn-tmp/`
- **Workflow run progress** — running workflows now show step-by-step progression with current step highlighted, instead of just "Running"
- Test fixtures — replaced project-specific names with generic placeholders
- Tech-debt list cleaned: removed 7 resolved entries

---

## [0.1.0] — 2026-03-24

### Added
- **Multi-agent discussions** — Claude Code, Codex, Vibe, Gemini CLI, Kiro with `@mentions`, debate mode, SSE streaming
- **MCP management** — 3-tier architecture (Server → Config → Project), 48 built-in servers, encrypted secrets (AES-256-GCM), disk sync for all agents
- **Workflow engine** — cron, multi-step multi-agent pipelines, tracker-driven (GitHub), manual triggers, 5-step creation wizard, live SSE progress
- **AI audit pipeline** — 4-state system (NoTemplate → TemplateInstalled → Audited → Validated), 10-step automated analysis, drift detection + partial re-audit
- **Pre-audit briefing** — optional 5-question conversational briefing injected into audit steps
- **Project bootstrap** — create new projects from scratch with AI-guided planning (Architect + Product Owner + Entrepreneur)
- **Tauri desktop app** — native installers for Windows, macOS, Linux (no Docker required)
- **Voice: TTS & STT** — 100% local, Piper WASM (9 voices FR/EN/ES) + Whisper WASM, voice conversation mode
- **5 supported agents** — Claude Code, Codex, Vibe (CLI + direct Mistral API), Gemini CLI, Kiro
- **Agent configuration (3-axis)** — 11 profiles (WHO), 22 skills (WHAT), directives (HOW)
- **ModelTier system** — abstract tier selection (fast/balanced/powerful) resolved per agent
- **Multi-key API management** — multiple named keys per provider with one-click activation
- **Token tracking** — per-message token counting (Claude Code stream-json, Codex stderr)
- **Worktree isolation** — each discussion/workflow in its own git worktree
- **GitHub/GitLab PR management** — create, review, merge from the dashboard
- **Responsive UI** — mobile-friendly layout
- **i18n** — French, English, Spanish (CLI + web)
- **CI pipeline** — GitHub Actions: clippy, cargo test, tsc, vitest, bats, security scan (label-triggered)
- **Security** — Bearer token auth (opt-in), CSP headers, AES-256-GCM for secrets

### Stack
- Backend: Rust (Axum 0.7, tokio, serde, SQLite WAL)
- Frontend: React 18 + TypeScript (Vite 5)
- Type bridge: ts-rs (Rust → TypeScript)
- Container: Docker Compose (backend + frontend + nginx gateway)

# Changelog

All notable changes to Kronn will be documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.8.7] - 2026-05-28

### Changed — Anti-hallucination: closed the web-project extension gap (.twig / .xlf) via a 4-conversation linguistic re-pass

A deep, multi-agent forensic re-pass (one linguistic expert per persona → reconciliation against the machine's real verdict → consolidated synthesis) read **every message** of the 4 persona conversations on the Symfony project DOCROMS_WEB and surfaced a single dominant root cause the earlier sampling missed.

- **`.twig` / `.html.twig` / `.xlf` (+ `.scss`, `.less`) were absent from the source-extension allowlist.** On a Symfony/web project these are THE primary files — so every real `templates/…html.twig` / `translations/messages.*.xlf` citation went unverified, and worse, the sentences citing them read as *unsourced* (false positives too). Fixed by extending the allowlist. No double-extension special-case is needed: matching is `ends_with`, and `foo.html.twig` ends with `.twig`.
- **The two extension lists had drifted** (`contains_code_anchor` lacked `.php`/`.go`/… that `looks_like_file_anchor` already had). Unified into a single shared `SOURCE_EXTS` const so they can't diverge again.
- **Proven on the real conversations:** verified sources **19 → 35** (+16 legitimate file citations recovered, incl. Clarisse's `templates/pages/projets.html.twig:82` — line 82 exact on disk — and Liza's `translations/messages.{fr,en}.xlf`), **0 false red**, and 8 non-resolving anchors (proposed/placeholder files) correctly surfaced as soft-amber "couldn't verify". The bounds check now also catches inline anchors whose line is out of range.
- **Honest limits documented** (module doc + this entry): the lint verifies *anchors* (path resolves, line in bounds), never file *content*. So 6 families stay structurally out of reach and are NOT bugs: semantic-content claims (the one real hallucination in the corpus — a non-existent `nth-of-type(even)` CSS rule — is invisible by construction), verbatim quotes, i18n *keys* inside an `.xlf` (the file resolves, the key isn't looked up — don't oversell `.xlf`), absence claims, bare basenames without `/` or `:line`, and approximate `~N` line numbers. 0 false positives across the whole corpus.

+6 anti_halluc tests (web extensions, unified-list guard, real-twig green, out-of-bounds-twig soft-amber). Full backend lib suite 2929 green, clippy clean.

### Changed — Anti-hallucination: ~300-test hardening campaign + closed the ES recall gap

A full autonomous validation campaign: a 9-dimension adversarial bug-hunt (~153 new integration tests in `backend/tests/anti_hallu_*.rs` — lang/citation-forms/verify-outcomes/security-jail/utf8/malformed-markers/tiers/multiroot/perf-caps), a forensic replay of `analyze()` against 7 real agent messages from 4 live persona discussions on a real project (17 verified sources, 0 fabricated, 0 real defects), and the closing of the last documented hole.

- **Closed the Spanish recall gap.** `CLAIM_CUES`/`HEDGES`/`OPINION_CUES`/`CONDITIONAL_OPENERS` were EN+FR only — a genuine Spanish code claim ("la función está definida en…") flagged via nothing. Added native ES cues (`está definido`, `se encuentra`, `la función`, `devuelve`, `es vulnerable`…) + ES precision guards (`debería`, `es preferible`, `cuando`, `creo que`…). Spanish claims now flag natively, ES opinions/hedges still suppressed. The heuristic is genuinely trilingual now.
- **Security/robustness invariants pinned** by the bug-hunt: a relative path escaping the project root (`../`, symlinked dir) is NEVER `Verified`; no panic on UTF-8 / emoji / CJK / malformed `[src:` / nested brackets / 100k-char inputs; the `MAX_SOURCES_VERIFIED` / `MAX_FLAGGED_SPANS` caps hold.

Backend ~174 unit + ~153 integration anti-hallu tests, all green; clippy clean. The bug-hunt found **0 new defects** — the core is solid after the session's four fixes (FR-question flagging, root-file recall, green-drawer colour, ES recall) + the two honesty tiers (unverified, unverifiable).

### Changed — Anti-hallucination: live-validated, hardened, + an honest "unverified" tier

A live PW validation campaign on a real project (DOCROMS_WEB, real Claude tokens) + a massive deterministic test expansion. Three real bugs found & fixed, one new honest signal added.

**Bugs found & fixed (the live + exhaustive testing earned its keep):**
- **French questions flagged as claims.** `is_question()` was dead code: `split_sentences` stripped the terminal `?` before it ran, so "Où est-ce configuré ?" was wrongly flagged unsourced. Fixed by re-appending the `?`/`!` terminator.
- **Root-level files never verified.** A live multi-citation test showed `composer.json:1` (cited, exists) not verified — `looks_like_file_anchor` required a `/`, dropping root files. Now accepts a slash **or** an explicit `:line` (so `composer.json:1` / `Cargo.toml:5` verify; bare prose `node.js` still excluded).
- **Green drawer on an orange background.** The lint detail drawer used a hardcoded amber background regardless of severity, so a verified (green) reply expanded onto an orange panel that read as "not good". Drawer now echoes `data-severity` (green/amber/red) + verified source paths render green, not red.

**New "unverified" tier (soft amber) — honesty over silence.** A natural inline anchor (`` `src/x.rs:9` ``) that doesn't resolve used to be silently dropped (a wrong inline citation escaped entirely). It's now surfaced honestly as `unverified_count` → a soft-amber "N citation(s) non vérifiée(s)" pill — distinct from the red "fabricated" (reserved for formal `[src:]` that fail, high-confidence) and from green "verified". The drawer gets a dedicated "Citées, non vérifiées" group, and the "Sources vérifiées" group now shows on ANY report with verified sources (say what's good *and* what's missing). i18n FR/EN/ES.

**New "unverifiable" tier (neutral grey) — Option B, warn about everything.** A reply citing ONLY uncheckable sources (URL / user-confirmed / inferred / commit / hypothesis) used to be dropped at finalize (`has_signal()` false → no pill), so "what couldn't be tested" was invisible. `has_signal()` is now simply `unsourced_count > 0 || !sources.is_empty()` — ANY citation is surfaced. These get a neutral grey "N source(s) non vérifiable(s)" pill + a "Non vérifiables (URL, déclaré…)" drawer group. The full honesty model now maps to the three things a user needs to know: ✅ vérifiée (green) · ✗ invalide (red) · ~ non vérifiée (soft amber) · — non vérifiable (neutral). The "« Vérifiée » ≠ vraie" caveat stays on every pill.

**Telemetry + testable finalize** (from the prior commit): `tracing::info!(target:"anti_halluc", …)` per reply + the extracted `finalize_lint_report` helper.

**Test coverage:** backend `anti_halluc` **88 → 174 tests** (classify/verify/clean_reference/multi-root/inline-anchor matrices, FR/EN/ES corpus precision guard at 0% FP, per-return-type scenario matrix 2-3 per pill type, finalize seam). Frontend lint pill **12 → 44 tests** (severity priority, every status→colour, drawer grouping per severity, the unverified tier). Full backend suite 2904 green. A live PW E2E scenario script lives at `docs/research/anti-hallu-e2e-scenarios.md`.

**Live-validated (DOCROMS_WEB):** green (verified inline anchor), no-false-positive (opinion/reasoning), multi-source green (surfaced the composer.json bug), red "source invalide" (formal `[src:]` out-of-bounds → "line 50 beyond file length 8"). The honest limit, documented: a live LLM can't be deterministically forced to emit every case (a well-behaved agent refuses to echo a knowingly-false citation = its own sourcing discipline working).

### Added — CLI-style message queue (type while the agent is streaming)

You can now type and send follow-up messages while an agent is still replying — instead of being blocked, they're **queued** and **merged into a single follow-up turn** that auto-fires when the response completes. The classic missing piece vs the raw CLIs. Implemented at Kronn's **orchestration layer** (the merged message becomes a normal `sendMessageStream` only once the prior run's `sending: true→false` edge fires), so it's **agent-agnostic** — works for ClaudeCode / Codex / Gemini / Vibe / Ollama alike, regardless of whether the underlying CLI supports queueing.

- The composer textarea stays editable while the agent streams; Enter (or a dashed queue-send button next to Stop) adds the message to the pending set.
- Parts added while pending are **accumulated and sent together as ONE message** (blank-line-joined) → the agent produces a single combined response, not N separate full responses. (True mid-stream injection into a running CLI turn isn't possible — the subprocess already has its prompt — so merging into one next-turn message is the efficient equivalent.)
- Pending parts render as ghost "outbox" bubbles above the composer (position number, optional `@agent`, ✕ to cancel each); the first explicit `@mention` among the parts sets the target agent.
- Stop clears the queue (stop means stop). The double-click send guard is preserved (`abortControllers` set synchronously → a same-tick second fire enqueues rather than launching a parallel run).
- New `useMessageQueue` hook (mirrors the proven `useQpChain` edge-trigger pattern; +9 unit tests). i18n FR/EN/ES.

### Fixed — Sidebar search now hides empty folders + non-matching favorites

Searching a discussion in the sidebar used to keep **every** project/org folder header and **every** favorite on screen — the actual matches were buried, forcing the user to expand folders and scroll. The folder/favorite visibility and the header counts were computed from the *unfiltered* disc lists. Now `matchesFilters` is applied to the favorites section, the "Général" group, the per-project/org folder visibility, and all the header counts. Since `matchesFilters` returns true for everything when no search/source filter is active, normal (unsearched) rendering is unchanged — but during a search only folders that contain a match render, non-matching favorites disappear, and the counts reflect the matches. (Folders already auto-expanded during search; the loose-disc cap + archives already bypassed the filter — those were fine.) +1 test in `DiscussionSidebar.grouping.test.tsx`.

### Changed — Anti-hallucination hardening (telemetry + testable finalize + dedup)

Follow-up to the precision fixes, prepping for data-driven tuning + live validation:
- **Telemetry hook** — the finalize path now emits one structured `tracing::info!(target: "anti_halluc", …)` line per agent reply with `unsourced` / `fabricated` / `verified` / `roots` counts. There was previously ZERO observability, so the real false-positive / verified-anchor rate couldn't be measured (the P4 heuristic is meant to be tuned from real data). `grep target=anti_halluc` now yields that dataset without touching the DB.
- **Testable finalize seam** — extracted the roots-assembly + `has_signal` gate out of the (un-unit-testable) `make_agent_stream` SSE closure into `anti_halluc::finalize_lint_report(text, workspace_path, project_path)`. This was the lowest-covered / highest-blast-radius part of the path; now covered by a sequential mode-aware test (off→None, verified→green, no-signal→None, worktree-root-first). `streaming.rs` just calls the helper.
- **Fixed `verified_count` double-count** — a file cited BOTH as a formal `[src:]` marker and an inline backtick anchor was counted twice (inflating the green chip + future telemetry). Dedup now keys on the bare path (type prefix + wrappers + `:line` stripped). +1 test.

+2 anti_halluc tests (88 total), clippy clean.

### Fixed — Anti-hallucination: false positives, false "not found", and the missing green badge

A multi-agent investigation (4 parallel deep-dives + a 60-message real-prose corpus) traced the user-reported anti-hallu problems to a handful of root causes. Key finding: agents barely emit formal `[src:]` markers (0 in 60 real messages) but **already self-source ~81 % of the time with natural backticked paths** — which the system accepted as "anchored" but never mechanically verified, so nothing ever earned a green badge and the prose heuristic carried the whole load (and over-flagged).

- **Green badge now actually shows (P2).** `LintReport::is_empty()` treated a fully-verified report as "nothing to show", so finalize stored `None` and the (already-built) green pill was dead code. Added `verified_count()` + `has_signal()`; the report is now stored whenever there's a signal — RED (`fabricated_count>0`), AMBER (`unsourced_count>0`), or **GREEN** (≥1 mechanically-verified source, 0 unsourced, 0 fabricated). Wording stays honest: "verified" = the source exists/resolves, not that the claim is true.
- **Natural anchors are auto-verified (P1, niveau-1.5).** `analyze()` now extracts backticked `` `path/file.ext[:line]` `` anchors (high-precision: slash + known extension required) and verifies them through the same path-jailed `verify_file_ref`. Purely positive — only resolving anchors are added (as verified File sources → green); a non-resolving inline anchor is dropped, never counted as fabricated. This converts the way agents *actually* write into a green signal with zero change to their behaviour.
- **Far fewer false "file not found" (P3).** `clean_reference()` strips the backticks / quotes / brackets / trailing punctuation agents wrap citations in (`` `src/foo.rs:42` `` used to fail outright). `verify_file_ref` now takes multiple roots and finalize passes the **Isolated-discussion git worktree first, then the main checkout** — so a file the agent saw/created in the worktree no longer reports NotFound. The lexical `../`-jail + symlink-escape re-check are applied per-root, so the SSRF/path-escape guarantees are unchanged.
- **Heuristic precision (P4).** The "N unsourced claim(s)" pill had a ~60 % false-positive rate on the corpus. Added accent/hyphen-insensitive matching (`peut être` == `peut-être`, the exact gap that flagged the reported DI sentence), opinion/recommendation suppressors (`devrait`, `je recommande`, `n'est pas toujours`, `anti-pattern`…), a conditional guard (a claim cue after `si`/`quand`/`if` is hypothetical, matched at word boundaries so `si` doesn't fire inside `version`), and skipping of questions / Markdown headings / imperative bullets.
- **Clearer directive (P5).** Rewrote the injected `PREAMBLE`: states the verify-cascade inline (instead of only pointing at a `docs/AGENTS.md` section that may not exist), explicitly blesses natural backticked-path / URL anchors as valid sources, reserves the formal `[src:]` grammar for curated docs, and gives agents an explicit way to mark opinions/guesses (`je recommande` / `[src: inferred: …]`).

`verify_source_marker` / `analyze` keep their `Option<&Path>` signatures as thin wrappers over new `_roots` variants (no churn on existing callers). +19 backend tests (86 in `anti_halluc` total), frontend pill unchanged (its green/amber/red derivation + FR/EN/ES strings were already built — 12 `lintPill` tests green). The fix is behind the existing non-blocking `warn` mode.

### Changed — Frontend workflow-builder component coverage (4th wave)

- **`WorkflowWizard`** 35 → 80 % Lines / 17 → 71 % Funcs (+77 tests, no prior test) — the 728-LOC multi-step builder: mode toggle, step navigation + name-gate, add/insert/move/remove steps, every step-type swap (Agent/Notify/Gate/Exec/JsonData/ApiCall/BatchQuickPrompt/BatchApiCall), `parseCronExpr` branches + Cron/Tracker trigger editors, per-step Advanced panel (`on_result` conditions, Goto + STATE blocks, retry/stall/backoff), QP-binding banner, skills/profiles/directives chips, undeclared-`{{var}}` declare flow, preset deep-link transform, QuickStart picker, full per-type Summary recap, Config tab (sandbox/allowlist/launch-vars/expert), and the create-vs-update save handler with payload assertion + double-click guard.
- **`WorkflowDetail`** 50 → 62 % Lines / 33 → 44 % Funcs (+26 tests) — run-history list, trigger/cancel/delete-run (+ confirm + catch branches), gate decision, synthesized live-run, `LiveFinishedBanner` variants, batch-chip navigation.
- **`ProjectLinkedRepos`** 39 → 97 % Lines / 35 → 97 % Funcs (+13 tests) — add (validation + payload), remove (confirm), URL-vs-path rendering, max-entries guard, save-error catch, picker prefill.

+116 tests, full suite green, tsc + eslint clean. Frontend total now **Functions 63.0 %, Lines 71.0 %, Statements 67.6 %, Branches 63.1 %** — crossed the 70 % Lines milestone, up from the 48/59/56/51 baseline. Floors ratcheted (statements 62→66, branches 55→61, functions 55→61, lines 65→69). Remaining gap is concentrated in the 5 page-shells + 3 huge chat/project components (integration-test territory, deferred).

### Changed — Frontend mid-tier component coverage (3rd wave)

Targeted the lowest-coverage mid-size components, prioritising the worst Functions % (error-handlers / rare branches — the gap the QA audit flagged):

- **`GitPanel`** 31 → 93 % Lines / 15 → 85 % Funcs (+34 tests) — the sensitive git panel: commit (payload + amend/sign), push, create-branch, create-PR (template prefill + auto-push-when-no-upstream + GitLab MR + existing-PR link), diff-on-click, exec terminal, and every rejected-api catch branch.
- **`CustomApiAiHelper`** 51 → 94 % / **`ApiCallAiHelper`** 54 → 94 % (+24 tests) — the AI-assisted form helpers: send→`sendMessageStream`, `KRONN:APPLY` block parse → SuggestionCard → `onApply` mapped payload, stream error branch, stop/minimize/restore/close teardown, agent-switch re-create.
- **`AgentsSection`** 52 → 70 % (+18 tests) — per-agent install/uninstall (with `confirm()` + post-uninstall verify), enable/disable toggle, full-access switch (click + keyboard), re-entry guard, all error toasts.
- **`AiDocViewer`** 38 → 67 % (+17 tests) — tree navigation → `readAiFile`, debounced `searchAiFiles`, tech-debt fix-CTA, and the three api-reject catch branches.

`tts-engine.ts` excluded from coverage (Web Worker + `new Audio()` playback, un-instrumentable in happy-dom — covered by the e2e voice flow, like the STT/TTS workers). +93 tests, full suite green, tsc + eslint clean. Frontend total now **Functions 57.1 %, Lines 66.9 %, Statements 63.7 %, Branches 57.0 %** (from the 48/59/56/51 baseline). Floors ratcheted (statements 58→62, branches 52→55, functions 52→55, lines 61→65).

### Changed — Frontend component coverage : leaf + stateful test pass

Following the `api.ts` sprint, brought the under-tested components up — targeting the lowest-coverage files first (biggest reliability win per test):

- **`QuickApiForm`** 0 → 96 % Lines / 91 % Funcs (+22 tests) — the QuickApi builder mirror of `QuickPromptForm`: field edits, dynamic variable rows auto-synced from `{{var}}` tokens, `ApiCallStepCard` wiring, save-payload mapping, inline error surfacing, race-free double-click guard, edit-mode round-trip.
- **`ProjectSkills`** 5 → 100 % (+9 tests) — chip rendering, external badge, active styling, unknown-category fallback, toggle add/remove with the `togglingRef` re-entry guard.
- **`SwipeableDiscItem`** 44 → 95 % Lines / 89 % Funcs (+11 tests) — the previously-uncovered pointer/swipe math: delta clamping (`sign(d)·min(|d|·0.7, 120)`), threshold-crossing reveals (archive-right / delete-left), sub-threshold snap-back, tap→`onSelect(id, unseenBasis)`, `pointerCancel` abort.
- **`DiscussionSidebar`** 69 → 97 % Lines / 94 % Funcs (+20 tests) — grouping (multi-org headers, "Local" last, pinned cross-project section), collapse/expand keys, search filter (title + id-prefix + clear + no-match), loose-disc cap "+N more", batch pastilles (status pill, parent-workflow nav, delete/retry confirm flows).
- **`MessageBubble`** 67 → 85 % Lines / 72 % Funcs (+49 tests) — role/variant selection, author gravatar-vs-initials-vs-fallback, copy/TTS button states, footer chips (tokens / auth_mode / duration / model_tier / full-access), last-message edit+retry affordances, edit-mode Ctrl+Enter, auth-error & partial-response CTAs, summary-cached expand toggle, `MarkdownContent` copyable-block + table `extractText` paths.

`ProjectList` (76 %) and `IdentitySection` (76 %) were already adequately covered and left as-is. Frontend total after both waves: **Functions 48.4 → 53.7 %, Lines 59.2 → 63.2 %, Statements 56.2 → 60.2 %, Branches 50.8 → 54.3 %**. Floors ratcheted again (statements 57→58, functions 50→52, branches 50→52, lines 60→61). +111 component tests, full suite green.

### Changed — Frontend `api.ts` becomes the best-covered file (the UI↔backend boundary)

`lib/api.ts` is the single seam between every UI surface and the backend — one wrong verb, typo'd path, or botched query-string is a production break that no page-level test localizes. It was the worst-covered critical file (**11 % Functions / 25 % Lines**) because the structural `expect(api.foo).toBeDefined()` tests never actually *called* the methods. This sprint makes it the **best**-covered:

- **Every non-streaming method exercised** (`api.methods.test.ts`, +115 tests) — verb + URL + path/query encoding + request-body shape, across the long tail that was uncovered: projects (audit-info / anti-hallu / briefing / linked-repos / ai-files / git ops), mcps (config CRUD / custom-spec / host-discovery / context files), discussions (share / participants / test-mode / context files / git ops), workflows (bundles / run lifecycle / test-worktree / dry-run helpers / batch), quickPrompts + quickApis (batch / compare / export-import / versions), plus profiles / stats / apiCallLogs / userContext. Special non-`api()` paths covered too: blob exports (`exportData`, `exportWorkflow`, `exportQp`, `exportQa`) with content-disposition filename + fallback + non-ok throw, FormData uploads (`importData`, `uploadContextFile`), and the pure `exportFileUrl` URL-builder.
- **SSE streamers pinned** (`api.streaming.test.ts`, +18 tests) via a `ReadableStream` mock harness — `projects.auditStream` / `partialAuditStream`, `discussions.orchestrate`, `discussions.sendMessageStream` / `runAgent` (delegation + `onLog`), `workflows.triggerStream` (run_start / step_progress + variables-body branch) and `testStepStream`. Each asserts the full event→handler dispatch table, the `HTTP <status>` error path, and the aborted-fetch-as-clean-done path — exactly where "spinner forever, event never fires" bugs hide. (`_streamSSE` + `fullAuditStream` were already covered.)

**Result** : `api.ts` **11 → 98.9 % Functions** (261/264), **25 → 92.4 % Lines**, **~25 → 72.8 % Branches**. Frontend total moved Functions 48.4 → 51.6 %, Lines 59.2 → 61.4 %, Statements 56.2 → 58.4 %. `vite.config.ts` coverage floors ratcheted up (statements 55→57, functions 47→50, lines 58→60) to lock the gain. Web Workers (`stt-worker.ts` / `tts-worker.ts`) excluded from coverage — they run off-thread loading ML models, un-instrumentable in happy-dom (covered by the e2e voice flow).

### Changed — small pure-logic extractions (frontend)

Three inline helpers lifted out of heavy page shells into `lib/` so the logic is unit-tested independently of the (un-mountable-cheaply) pages: `pluginKind` (MCP plugin-kind bucketing, 4 cases — `lib/pluginKind.ts`), `linkify` (URL→anchor splitting with paren/edge cases — `lib/linkify.tsx`), `findLastAgentMessage` (auto-TTS target selection — `lib/discussionHelpers.ts`). +21 tests. Behaviour identical; `McpPage` / `DiscussionsPage` now import them.

### Changed — `AgentIo` test-seam : the agent pipelines become unit-testable (no CLI, no tokens)

The agent-consumption loops (the bug-prone core the QA audit flagged: SSE streaming, tool-call parsing, decoder-loop detection, stall/cancel/error-exit) had **zero** unit coverage because every test would spawn a real CLI subprocess. Introduced a `runner::AgentIo` trait (`#[async_trait]`, mirroring the `TrackerSource` convention) abstracting the agent process surface — `next_line / output_mode / kill / wait / try_wait / child_id / captured_stderr_flushed / fix_ownership` + a portable `AgentExit { success, code }`. `AgentProcess` impls it for production; a `#[cfg(test)] ScriptedProcess` yields pre-canned lines with no subprocess. The pipeline loops are now generic over `impl AgentIo` and driven by scripted output under test.

- **`AgentStartConfig::new(agent_type, project_path, prompt, tokens)`** — the 11 spawn sites (streaming / orchestration ×5 / audit ×3 / steps / runner) previously repeated `mcp_context_override: None, model_tiers: None, context_files_prompt: "", discussion_id: None, …` verbatim ; now use struct-update over the constructor. ~90 lines of boilerplate removed, behaviour identical.
- **`run_agent_collect`** (silent summarization collect) — generic over `impl AgentIo` + 5 scripted tests (raw join+trim, empty stream, stream-json text-only accumulation, non-JSON→raw-text fallback contract, single line).
- **`run_agent_streaming`** (orchestration debate rounds) — generic + 7 tests : raw chunk emission, stream-json text accumulation, tool-call → exactly one Log event (no JSON leak into prose), terminal-signal truncation, **decoder-loop abort** (EW-7189), empty-response error-exit formatting, clean-exit `[No response]`.
- **`make_agent_stream`** (main chat SSE) — extracted the two pure pieces shared with `run_agent_streaming` into tested helpers : `is_decoder_loop` (the `</thinking>`×N detector — was byte-duplicated 2×) + `classify_tool_call` → `ToolRecord::{Kronn,Native}` (the kronn-internal-vs-native transcript bucketing). 8 helper tests (threshold fire, reset-on-change, short/whitespace ignored, both buckets, arg truncation). The async control flow was deliberately left in place — its cancel/timeout/stall paths are covered by-proxy via `run_agent_streaming` (same `tokio::select!` mechanic).
- **`run_agent_with_timeout`** (workflow Agent step — Ticket Autopilot / BatchQuickPrompt) — post-spawn loop extracted into `drive_agent_to_output(impl AgentIo, …)` + 6 scripted tests : text+token collection, raw join, progress_tx tool breadcrumbs, failed-exit-with-stderr-tail, failed-exit-no-stderr actionable message, clean-empty-exit.

**Result** : the extracted/migrated functions are ~100 % covered ; backend total holds at ~77.8 % Lines / 81.6 % Functions. The host files (`streaming.rs` 48 %, `runner.rs` 63 %) stay below 75 % at the FILE level because the bulk is the real-`Command` spawn machinery + the 500-line SSE handler body — inherently un-unit-testable without a mocked subprocess (a deliberately deferred fixtures sprint). The **bug-prone consumption logic is now pinned** — that's the Reliability lever, independent of the file-level %.

**Tech-debt acknowledged** : a future full refactor can (a) merge the duplicated `StreamJsonEvent` match-arms between `make_agent_stream` + `run_agent_streaming` into one handler (would finally move the jscpd rust dup down), and (b) fully extract `make_agent_stream`'s 180-line loop to test cancel/timeout/stall directly instead of by-proxy. Both scoped out here for safety on the core chat path.

### Changed — CI hardening : duplication ceiling + coverage floors + Node 24 + E2E container

- **`duplication-check` job (jscpd)** across Rust + TS/TSX + Python — `.jscpd.json` pins `threshold: 4` (baseline 3.75 % dup lines, 270 clones, rust-dominated). Exits 1 when exceeded, mirroring the coverage floors. Ratchet down as dup drops, never raise. Non-code formats (md/yaml/sql/json) excluded so they don't dilute the real signal.
- **Coverage regression floors** : backend `cargo llvm-cov --fail-under-lines 77 --fail-under-functions 81 --fail-under-regions 78` (via `taiki-e/install-action`) ; frontend `coverage.thresholds {statements 55 / branches 50 / functions 47 / lines 58}` in `vite.config.ts`, CI runs `pnpm test:coverage`.
- **E2E job moved into the official Playwright container** (`mcr.microsoft.com/playwright:v1.59.1-noble`, image tag == resolved `@playwright/test`) — browsers pre-baked, **zero `cdn.playwright.dev` download**. That CDN intermittently TCP-half-closed after the 170 MiB chromium .zip hit 100 %, hanging the step to the job timeout so the browser cache never saved (cold-loop, 3× in project history). Rust + C toolchain installed in-container via rustup + apt.
- **Node 24 runtime opt-in** (`FORCE_JAVASCRIPT_ACTIONS_TO_NODE24`) — silences the Node 20 deprecation annotations ahead of GitHub's 2026-06-02 cutover.
- **`.gitignore`** : `frontend/coverage/` + `backend/coverage/` (lcov-report was being committed).

### Fixed — "Add project → Discover repos" multi-provider regression

- **Discover-repos state is now reset when the "Add project" modal closes**, so a re-open starts from a clean slate. Pre-fix `selectedSourceIds` survived modal close → if the user had previously unchecked `github Euronews` or `GitLab` chips, the next open would send a filtered `source_ids` request and the backend would correctly return only the perso GitHub repos. The user perceived it as *"only my personal GitHub key is detected"* even though both pro tokens were configured + functional. The reset wipes 6 states (`selectedSourceIds`, `discoveredRepos`, `availableSources`, `discoverSources`, `discoverSourceErrors`, `repoSearch`, `discoverError`). Also fixed : toggling the last active source-chip OFF now clears the repo list (was leaving stale repos visible while no chip was active).
- **Per-source discovery failures are surfaced in the UI** instead of being lost in `tracing::warn!`. The discover endpoint accumulates a new `errors: Vec<DiscoverSourceError {source_id, source_label, provider, message}>` field on its response, populated whenever `fetch_github_repos` / `fetch_gitlab_repos` returns Err. The frontend renders an amber chip per failing source — e.g. "🦊 GitLab — 401 Unauthorized — token revoked or scopes missing" — so the user knows WHY a configured source returned zero repos instead of guessing the integration is broken. (This was the GitLab silent-fail trigger.) Frontend defaults `errors ?? []` to tolerate older backends. **Tests** : 4 in `Dashboard.discover.test.tsx` (modal-close state reset / errors chip rendered when present / no chip when empty / legacy backend tolerance).

### Added — QA P0 quick-wins sweep (test coverage)

Multi-agent QA audit (4 specialists + ISO 25010 lead) ran on 2026-05-28 and produced a P0/P1/P2 matrix. The 6 P0 quick-wins (≤ 60 min each) shipped in this batch :

- **P0-1 `core/crypto.rs` AES-256-GCM roundtrip + AEAD failure** : 16 new `#[cfg(test)] mod tests` cases. The MCP env-secret pipeline rests on this 91-LOC module ; pre-fix it had **zero** tests. Pinned : encrypt/decrypt roundtrip (ASCII / empty / UTF-8 emoji / 8 KB payload), unique-nonce-per-call, AEAD tag-tampering rejected, wrong-key rejected, truncated input rejected, invalid base64 rejected, `generate_secret` returns 64 hex chars + unique-per-call, `parse_secret` accepts/rejects (length / non-hex / odd-length), `mask_value` short-vs-long display contract.
- **P0-2 `auth_middleware` ConnectInfo fallback** : 13 new integration tests in `backend/tests/api_tests.rs::auth_middleware_tests` covering the full decision matrix : health/ws bypass, no-token-configured passthrough, valid/invalid/missing-prefix Bearer, X-Real-IP localhost / public / forged-"localhost" string, ConnectInfo localhost / public / missing extension (the audit's actual concern — confirmed fail-closed), `auth_strict_localhost = true` disables both X-Real-IP and ConnectInfo bypasses but still accepts Bearer. The pure `is_local_ip` was already covered by `auth_tests` ; this layer pins the middleware's wiring under axum.
- **P0-3 SSRF guards `assert_public_ip` + `assert_host_matches_base`** : **SKIPPED** — audit was a false positive. `backend/src/workflows/api_call_security.rs` already ships 22 tests covering loopback v4/v6, RFC1918, AWS metadata link-local, fc00::/7 ULA, host-match exact/port/subdomain/scheme/case, public-IP allowlist. Surface verified pre-write per the new "verify before code" feedback ([[feedback_verify_subagent_audits]]).
- **P0-4 `create_batch_run` transaction rollback** : 2 new tests in `db/tests.rs`. Confirms that an injected FK violation mid-loop (`parent_run_id` pointing at a non-existent run) returns `Err` AND fully rolls back — no orphaned placeholder workflow, no orphaned `workflow_run`, no orphaned `discussion`, no orphaned `message`. Companion test pins that the connection is in a clean transactional state after rollback (a subsequent call with a valid input still succeeds — guards against the "stuck BEGIN" SQLite footgun).
- **P0-5 Symlink-escape on RELATIVE citations** (`core/anti_halluc.rs`) : the 2026-05-28 absolute-path-existence-only policy had inadvertently removed the canonicalize-and-recheck guard for the relative branch too. Restored it (relative paths still get lexical jail + post-canonicalize containment check ; absolute paths remain trusted-by-existence). 2 new tests (unix-only `#[cfg(unix)]`) : a symlink-inside-project → outside-project is caught as `OutsideProject` ; a symlink-inside-project → inside-project (legitimate vendored alias) stays `Verified` so the guard doesn't over-fire.
- **P0-7 Migration idempotency × 61 (strengthened)** : the existing `migrations_idempotent` only ran migrations twice without asserting integrity ; pre-fix a migration that silently doubled-inserted seed rows or re-CREATEd an index with a different SQL would slip through. New test `migrations_idempotent_schema_stable_across_two_runs` compares `sqlite_master` (tables + indices + triggers + views + SQL) byte-for-byte between run 1 and run 2, AND captures row counts on 13 seed-candidate tables — both must match.

Total : **34 new backend tests** added, **0 frontend changes**. Backend coverage rises ~+1.5 % focused on the highest-stakes axes (Security: crypto + auth + symlink ; Reliability: rollback + migration idempotency). Remaining P0 (P0-6 streaming cancellation + SSE flush, P0-8 E2E clone path, P0-9 anti-hallu pill 3 layers, P0-10 useAsyncGuard + ChatInput send race) lined up as the next batch — see [[project_qa_coverage_75_roadmap]].

### Added — QA P0-10 + P1 Settings cards sweep (test coverage round 2)

- **P0-10 ChatInput send-race + useAsyncGuard** : useAsyncGuard turned out to be already covered (4 tests existed — second audit false positive, after P0-3 SSRF). The real gap was `ChatInput::handleSendMessage` — the closure-stale `sending` prop allowed two synchronous clicks to fire `onSend` twice in the same tick (pre-fix : double-POST on send). **Code patched** with a `sendInFlightRef` set+cleared in the same tick, released via `queueMicrotask` after `onSend` (so a synchronous throw still releases the guard and the user can retry). **5 new tests** in `ChatInput.sendRace.test.tsx` : 2 sync clicks → 1 onSend, Enter+Enter → 1, microtask flush allows next send, sync throw releases the guard, `sending=true` swaps the send button for stop (UI guard).
- **P1-6 IdentitySection** (zero coverage → 11 tests) : mount hydration (pseudo / email / bio / global context / mode), save-on-change for each field, gravatar preview rendering rules (no @ → no img), global-context save-on-blur only when dirty, mode change persists immediately, mount error resilience.
- **P1-7a OllamaCard** (zero coverage → 11 tests) : 4 explicit states (not_installed / offline / unreachable / online with-or-without models), canirun.ai hint always visible (2026-05-11 regression guard), default-model picker fires `setModelTiers` with optimistic update, refresh button re-fetches, health rejection degrades to offline rendering.
- **P1-7b ProfilesSection** (zero coverage → 12 tests) : list mount, refetch on `kronn:profiles-changed` (secret-code unlock case), delete button HIDDEN for builtin profiles (cascade-safety regression guard), delete confirm flow (accept → API + toast, cancel → no call), removed-from-list verification, inline persona-name edit save/Escape, create-form toggle + validation + submit.

**This round** : 39 new frontend tests, 0 frontend production changes outside the ChatInput race fix. Settings card zone (worst at 20.5 % per LOC ratio) now has 3/5 covered (Identity / Ollama / Profiles ; AntiHallucSection + UsageSection already had suites). Next round = the real coverage audit via `vitest --coverage` + `cargo llvm-cov` to replace LOC-ratio heuristics with real instrumentation data.

### Added — `lib/api.ts` coverage sweep + backend cold-API handlers

Real-instrumented audit (`vitest --coverage`, `cargo llvm-cov`) showed that the LOC-ratio audit had two critical blind spots — `lib/api.ts` was at **24 % statements / 11 % functions** despite being the ENTIRE HTTP boundary between every UI surface and the backend, AND multiple backend API handlers (`api/usage.rs`, `api/audit/info.rs`, `api/debug.rs`, `api/projects/migrate.rs`, `api/api_call_logs.rs`) were at **0 % Lines**.

- **Frontend** : `api.coverage.test.ts` exercises 100+ methods across 25 namespaces (setup, version, config, contacts, projects, agents, mcps, skills, profiles, directives, workflows, quickPrompts, quickApis, discussions, rtk, usage, ollama, apiCallLogs, debugApi, themes, docs, autoTriggersApi, userContext, stats). Each test mocks fetch + asserts (verb, URL) + body shape. Plus 11 wrapper-level tests for the core `api()` : auth bearer attach/detach, apiBase prefix + trailing-slash strip, non-JSON 4xx body trim + 500-char cap (axum 422 case + nginx 502 HTML case), `ApiResponse.success === false` → Err propagation, JSON body serialization, no body on GET. **+123 tests** total ; the `api.ts` Functions metric should land north of 70 % (from 11 %).
- **Backend** : `cold_api_handlers_tests` module hits every cold endpoint with a bogus id and asserts a clean ApiResponse envelope OR a 4xx (never a 5xx panic). **+51 tests** covering `api/usage`, `api/debug/logs`, `api/audit/info`, `api/api_call_logs` (list/get/purge), `api/projects/migrate-docs`, `api/projects/discover-repos`, `api/contacts/{list,network-info,invite-code}`, `api/agents`, `api/quick-apis`, `api/quick-prompts`, `api/user-context`, `api/discussions/.../meta` + a table-driven macro sweep on 31 more cold routes (anti-hallu inject, project git status/diff/commit/push/exec/pr-template, disc messaging + git + worktree, qp/qa/workflow individual CRUDs, project install-template + drift + cancel-audit + mark-bootstrapped, disc summarize).

**Real coverage after this round** : Backend at **75.30 % regions / 76.37 % functions / 73.32 % lines** (up from 74.79/75.17/72.66). Frontend full re-measurement pending — `api.ts` Functions targeted to climb from 10.98 % to ~75 %+. Audit doc saved at [[project_real_coverage_audit_20260528]].

**Session totals** : 291 new tests added across 9 files (0 production changes outside the ChatInput sendRace fix), 2 confirmed audit false positives ([[feedback_verify_subagent_audits]]), backend lifted from 72.7 % → 73.3 % Lines incrementally.

### Added — Final round: cold-handler CRUD + RTK/agent_api/bootstrap sweep

Continued grinding the cold backend handlers identified by `cargo llvm-cov`. Added :
- **Quick Prompts full CRUD lifecycle** (create → list → update → delete → list-confirms-absent) + validation rejection tests (empty name, name > 200, empty template, update unknown id).
- **Quick APIs / Skills / Profiles / Directives** lifecycle smoke (create + delete loop where the test env supports it).
- **RTK endpoints** (`/api/rtk/version` `/api/rtk/savings` `/api/rtk/activate` `/api/rtk/deactivate`).
- **Agent API broker** (`/api/agent-api/call`) — validation entry paths.
- **Project bootstrap / clone / add-folder** — validation rejection paths.
- **MCPs registry + overview** — happy-path GETs.
- **Skills / profiles / directives listing** — list endpoints.
- **Config full roundtrip** — exercises 18 config endpoints in one test (language, ui-language, scan-paths, scan-ignore, scan-depth, server, anti-hallu-mode, global-context, global-context-mode, tts-voices, tts-voice, stt-model, model-tiers, tokens, agent-access, db-info).

+35 backend tests on top of the earlier batch. **Backend real coverage now 75.69 % regions / 77.34 % functions / 73.82 % lines** (up from 74.79 / 75.17 / 72.66 at session start, Δ +0.90 / +2.17 / +1.16 across the whole session).

**To reach 80 % backend Lines** : the remaining 6.2 % gap requires happy-path integration tests against the 5 biggest cold pipelines (`api/audit/full.rs` 2027 LOC, `api/discussions/streaming.rs` 1608, `api/discussions/orchestration.rs` 1168, `api/mcp_remote.rs` 1161, `api/disc_git.rs` 932 — 5300+ uncovered lines total). Each needs fixture infrastructure (mocked agent runner, real disc + git repo, SSE response capture). Estimated 12-18 h focused work. Path documented in [[project_real_coverage_audit_20260528]].

**Final session totals** : **348 new tests** added (180 backend + 168 frontend) across 12 files, 0 production changes outside the ChatInput sendRace fix, all gates green. Last batch added MCPs detailed (refresh / create-with-unknown-server / update / delete / reveal-secrets / host-discovery / contexts), profiles/directives/skills PUT-unknown-id, ollama health/models, ai_docs file tree endpoints, setup health/version. **Backend coverage final: 75.89 % regions / 77.56 % functions / 74.02 % lines** (Δ session +1.10 / +2.39 / +1.36 from 74.79 / 75.17 / 72.66 baseline). Crosses the 74 % Lines bar.

The remaining ~6 percentage points to the 80 % target require a different testing strategy — happy-path integration tests against the 5 biggest async pipelines (`audit/full.rs`, `discussions/streaming.rs`, `discussions/orchestration.rs`, `mcp_remote.rs`, `disc_git.rs` — 5300+ uncovered lines collectively). Each requires fixture infrastructure (mocked agent runner, real disc + git repo, SSE response capture). Estimated 12-18 h focused work, deliberately deferred to a future session.

#### Session 2 sweep — crossed the 75 % Lines bar (2026-05-28)

A second pass introduced **happy-path tests on disc_git + project_git endpoints** with full git-tempdir fixtures, exercising `resolve_discussion_work_dir` + `git_ops` end-to-end. Added `seed_repo` + `seed_disc_with_repo` + `seed_project_with_repo` helpers in `tests/api_tests.rs` so 9 disc_git endpoints and 7 project git endpoints get real-repo coverage rather than just wrong-id envelope checks. Each test exercises 50-150 LOC of handler vs ~5-10 for wrong-id.

Plus pure-helper tests on cold modules : `core/context_files.rs` (+22 tests covering docx/pptx/pdf/xlsx error paths, image save/delete, mime mapping, suggest_skills branches), `workflows/batch_apicall_step.rs` (+11 tests covering parse_items error paths, value_kind variants, inject_item_vars bool/null, the `fail` helper), `api/discover.rs` (+12 tests on previously-untested `normalize_repo_url` + GitHub/GitLab parsers), `workflows/workspace.rs` (+5 tests adding `Workspace::create/cleanup/attach/hook` E2E with tempdir git repos — file jumped from 58 % → 97 % Lines), `api/contacts.rs` (+9 tests on `is_tailscale_ip` / `is_private_ip` IP-range classifiers). Plus envelope sweeps on disc_source (10), discussions cold paths (15), audit cold paths (8), mcp_remote (12), quick_apis (5).

**Backend coverage : 75.89 / 77.56 / 74.02 → 77.12 / 79.23 / 75.35** (Δ session 2 = +1.23 / +1.67 / **+1.33** Lines). Cumulative across both sessions (from 74.79 / 75.17 / 72.66 baseline) = +2.33 / +4.06 / **+2.69** Lines. The 75 % Lines floor is met across the full backend.

#### Session 2 — late add (helper coverage tail)

Final +30 tests targeting under-tested pure helpers : `core/tailscale.rs` +16 (classify_ip vpn/wg/utun/tap/ppp branches + 172.16-31 / 192.168 corners + public-IP fall-through + `detect_via_host_env` env-var path), `core/config.rs` +4 (load-none + is_first_run + scan-ignore + anti_halluc_mode/default_tier roundtrip), `db/contacts.rs` +10 (parse_invite_code edge cases + CRUD unknown-id reports + duplicate-PK rejection + ordering). **Cov after this batch : 77.31 / 79.40 / 75.53 Lines** (+0.19 / +0.17 / **+0.18** vs the prior in-session number).

Then a final +15 covering 0-test helpers that survived earlier passes : `api/api_call_logs.rs` +6 (parse_source / parse_status all known + unknown-case rejection + clamp range coverage) + `core/mcp_scanner.rs` +9 (`slugify_label` simple / adjacent-separator collapse / leading-trailing strip / alnum-only / empty-input + `is_default_mcp_context` unedited / user-edit / no-marker / bullet-only branches). **Cov after this batch : 77.33 / 79.51 / 75.58 Lines.**

Final tail : 5 **pre-seeded happy-path tests** on `mcp_remote.rs` — seed a `workflows` row + a terminal-status `workflow_runs` row, then exercise `workflow_wait_for_completion` (3 status variants : Success / Failed / Cancelled) + `workflow_run_status` + `workflow_run_discussions`. The long-poll loop exits on iteration 1, walks the happy-path serialization (run lookup → is_terminal_status → status format → tokens_used / elapsed_ms / finished_at). Pre-seeded > wrong-id ratio : **+0.11 Lines for 5 tests** vs +0.18 for 30 helpers. **Final cov : 77.42 / 79.54 / 75.69 Lines.** Cumulative cross-session delta = +2.63 / +4.37 / **+3.03** Lines. Total tests added across both sessions = **528** (287 backend + 241 frontend) ; backend test count = **3035**.

**Honest plateau** : the synthetic / wrong-id / helper-test pattern has clearly hit diminishing returns (~0.18 Lines per 30 tests, was ~0.7 per 50 in the early sweeps). The remaining 4.5 percentage points to 80 % Lines lock onto the big async pipelines (`audit/full.rs`, `discussions/streaming.rs`, `discussions/orchestration.rs`, `mcp_remote.rs` core paths) — those require fixture infrastructure (mocked agent runner, SSE response capture, persisted-state setup) that is **not a same-session lift**. Deferring to a future targeted session.

### Fixed — Anti-hallucination false positives on cross-repo absolute citations

- **An absolute `[src: file: …]` citation is now checked for existence on the host filesystem only — no project-root jail.** Pre-fix the `core::anti_halluc::verify_file_ref` resolver jailed every path to the discussion's single project root, which made every legitimate cross-repo reference (linked_repos / monorepos / sister sites — e.g. citing a `front_africanews` file from a `front_euronews` discussion where `front_africanews` is configured as a `linked_repo`) flagged as `not_found` / `outside_project`. The agent had no way to cite a file the user explicitly told Kronn to consider part of the working set. Fix: absolute paths in citations are translated through `scanner::resolve_host_path` (the same alias map the runner uses, so `/home/<user>/…` ↔ `/host-home/…` survives Docker) and then existence-checked directly. Relative paths still go through the lexical jail (so a `../etc/` smuggled inside a relative citation is still caught). Means we don't need to teach the lint about `linked_repos` at all — the agent emits real absolute paths, the lint asks the filesystem. **Tests**: 3 new `core::anti_halluc::tests` (`verify_relative_path_traversal_is_jailed` renamed + scoped, `verify_absolute_path_is_existence_only_no_jail`, `verify_absolute_path_to_sibling_dir_is_verified`, `verify_absolute_path_that_does_not_exist_is_not_found`).

### Added — Positive provenance pill (green ✓)

- **The per-message lint pill now has a third state: `verified` (green ✓ `N sources vérifiées`).** Pre-fix the pill only surfaced negatives — red on fabricated citations, amber on unsourced claims, silent when everything was fine. The silence meant a user couldn't tell at a glance "did the agent cite anything that actually resolved?". The green pill closes that gap: it fires when at least one source resolved mechanically (`status=verified`) AND no source failed AND the unsourced heuristic stayed silent. Suppressed when every cited source is only `unchecked` (urls / users / inferred / etc.) — a green chip on citations Kronn never actually verified would be misleading. `fabricated` keeps its precedence over `verified` (mixed report stays red). Clicking the pill opens a positive "Sources vérifiées" panel listing the resolved citations (parallel to the existing negative list). i18n FR/EN/ES (3 new keys × 3 dicts: `disc.lintVerified`, `disc.lintVerifiedTitle`, `disc.lintPillHintVerified`). **Tests**: 4 new in `MessageBubble.lintPill.test.tsx` (green pill renders + count + i18n key, suppressed on all-unchecked, fabricated outranks verified, clicked detail panel surfaces the verified list) + 1 pre-existing "no pill when empty" test re-anchored to a truly-empty report (a report with only verified sources now produces a green pill, which is the new contract).

### Fixed — Unread-badge inflation on workflow / tool-heavy discussions

- **The "messages à lire" counter no longer counts tool calls and cached-summary breadcrumbs.** The streaming layer (`api/discussions/streaming.rs`) persists every kronn-internal tool call, every native tool call, and every `summary cached | …` line as its own `MessageRole::System` row, all of which bumped `message_count`. Reported live: 26 workflow-run discussions (~2 user-facing messages each) showed an aggregate badge of 400+ because the System rows dominated. Fix: a new `Discussion.non_system_message_count` field (`SELECT COUNT(*) … WHERE role != 'System'`, computed in `DISC_SELECT_COLS`; also rebuilt from the loaded messages array on the two builder paths that override after `list_messages`). The frontend routes every unread basis through a single `unseenBasis()` helper in `SwipeableDiscItem.tsx` — covering the per-disc swipe row, the per-group sidebar counts, the top-level `totalUnseen` reduce, the mark-all-read seed, and the post-send markDiscussionSeen `+1`. `message_count` is preserved as the raw total for the "X messages" display. **Tests**: 3 backend `db::discussions_test` (mixed roles excludes System; empty disc → 0; clean disc → counts equal) + 5 frontend `SwipeableDiscItem.unread.test.tsx` (prefer non-System; aggregate 26×2 = 52 vs the buggy 1352; legacy fallback via messages filter; last-resort fallback to `message_count`; empty-disc safety) + the pre-existing `regression.test.ts` "unseen count uses message_count" updated to pin the new contract.

### Added — Convention authoring surface (MCP tool + builtin skill)

- **MCP tool `convention_get` on kronn-internal** — fetches any Kronn doc convention spec verbatim. v0.8.7 ships one: `(name="agents-md-format", version="v1")` → returns the `text/markdown` body Kronn's lint actually verifies against. Allowlisted server-side (an agent can't bait the tool into fetching arbitrary backend paths). Pairs with the `<!-- kronn:spec href="…" -->` pointer that audit STEP 0 deterministically inserts at the top of every `docs/AGENTS.md` — the pointer says *where* the spec lives, the tool fetches it on demand. Cost is zero when not called. 5 Python tests in `test_disc_introspection_mcp.py` (registration, default args, explicit args, unknown-name allowlist guard, unknown-version guard).
- **Builtin skill `kronn-doc-author`** — concise authoring cheat-sheet (≈60 lines, embedded via `include_str!`) covering the `<!-- kronn:section -->` markers, the `[src: kind: ref]` grammar, the 9 provenance tiers with their trust levels (`file`/`url`/`user`/`commit`/`api`/`code-comment`/`inferred`/`hypothesis`/`training-data`), and a closing pointer to `convention_get` for the full spec. Opt-in today (attach to discussions where the agent will edit/maintain AGENTS.md). Auto-triggers on the obvious patterns (`kronn:section`, `[src:`, "AGENTS.md", EN/FR/ES "rédige/écris/edit la doc IA"). 2 Rust tests in `core::skills::tests` pin (a) the skill is registered + builtin and (b) the body covers section markers + `[src:]` grammar + the three trust extremes + the `convention_get` pointer — so a future PR can't silently drop or strip the cheat-sheet. **0.8.8 follow-up** : under `enforce` mode, (a) auto-attach the skill at the runner chokepoint for any project that has `docs/AGENTS.md` — covers user-driven discs. (b) The audit pipeline doesn't need the skill (its prompts already embed the grammar inline), but Strict graduates the audit flow itself : step-level retry on `fabricated_count > 0` (cap 2-3), whole-doc re-lint before `AGENTS.md` is committed to disk, refuse-write if the final pass still flags. Auto-stamp `audit="<YYYY-MM-DD>"` on every `curated="ai"` section generated. Net effect : a Strict audit from scratch produces convention-perfect docs by construction. The whole enforce-mode chantier ships behind a **beta** tag (existing "Strict (preview · 0.8.8)" i18n strings promoted to "beta") — real-world validation will take iteration before the badge comes off.

### Added — Anti-hallucination program, Phase 1 + 2 (new `core::anti_halluc`)

- **Stage 1 — sourcing discipline injected into every agent prompt (P1)** + **post-output lint (P2)**, driven by a global `anti_hallucination_mode` (`off | warn | enforce`, default `warn`; `Settings → Anti-hallucination` + `config.toml`, mirrored to a process-global flag at load/save). P1 prepends a sourcing directive at the single runner chokepoint (`runner::start_agent_with_config`), so it covers **every** agent surface (disc, audit, AI architect, QP improver, batch, summarization, orchestration, **and Ollama** via `extra_context`) with one edit. **P2 is two-tier**: *niveau 0* — a cheap, lenient prose heuristic (EN/FR cues) flagging **unsourced** claims (low-confidence, `unsourced_count`); *niveau 1* — **mechanical verification** of every `[src: …]` citation the agent emits (high-confidence, `fabricated_count`). Niveau 1 resolves file refs **path-jailed to the project root** (`../../etc/passwd` → `outside_project`, never touches the FS), checks existence + line/range bounds (size-capped), and is language-agnostic + compression-proof + ungameable (you can't fabricate a file/line that exists). URLs/commits/user/inferred tiers are `unchecked` in 0.8.7 (SSRF-safe — no network at finalize). The lint runs at message finalize against the disc's effective host tree and persists to a new nullable `messages.lint_report` column (**migration 062**). UI: a non-blocking per-message pill — red for fabricated citations, amber for unsourced claims (color encodes only that binary headline; click to expand the detail panel). i18n FR/EN/ES. **Honest by design**: `verified` means the citation *exists*, not that the claim is *true* (that's the future LLM-judge / human review).
- **Phase 2 — the open convention spec** (`backend/docs/conventions/agents-md-format-v1.md`, embedded in the binary via `include_str!` and served on `/api/conventions/agents-md-format-v1` so it can't drift from the running anti-halluc semantics): `<!-- kronn:section name/curated/audit -->` markers, the 9-type provenance gradient (`file`/`url`/`user`/`commit`/`api`/`code-comment`/`inferred`/`hypothesis`/`training-data`), `curated="ai"` vs `curated="human"`, `audit="<date>"` vs git, bilingual parser aliases, fully annotated example. Readable + consumable without Kronn; the contract P3 (write-time refusal, future) will implement.
- **Tests**: 31 `core::anti_halluc` unit tests (mode parsing/gating, heuristic flag/suppress, fence-skip, UTF-8/emoji char-safety, span caps; niveau-1 extraction, line-spec parsing, classification, real-temp-project verification incl. path-traversal jail / out-of-bounds / not-found / no-root) + 3 config round-trip/back-compat tests + 6 frontend pill tests + i18n parity. Backend suite green (2491).

### Changed — Settings : Sourcing &amp; Anti-hallucination promoted to its own card (post-panel polish)

- **Section extracted from "Identity"** (`frontend/src/components/settings/AntiHallucSection.tsx`, new ; previously inlined in `IdentitySection.tsx`). The anti-hallucination toggle is a policy that frames how *every* agent documents — putting it inside Identity (nickname / avatar / bio) was a category error. Now a dedicated top-level card titled **"Sourcing &amp; Anti-hallucination"** sits **above** the Agent config accordion (Agents / Skills / Profiles / Directives), so the rule reads as the umbrella over the agents that follow. Title, intro, the 3-mode dropdown, 3 per-mode explanations, and an inline "View the full spec" disclosure that fetches `/api/conventions/agents-md-format-v1` (the embedded Phase 2 doc, served via `setup::get_agents_md_spec_v1`) and renders it via `react-markdown` + `remark-gfm`. i18n FR/EN/ES (8 new keys per dict).
- **"Strict" labelled "Strict (preview · 0.8.8)" / "Strict (aperçu · 0.8.8)" / "Estricto (vista previa · 0.8.8)"** + a disclosure toast on selection : *"Strict behaves like Warn until 0.8.8 — write-refusal ships then."* Closes the trust-killer where a user picked Strict expecting hard-stops and silently got Warn behaviour.
- **Expert-panel review fixes** (5 domains : UX, A11y, Security, FE code, BE code + Kronn tech-lead fact-checker validating each finding against doc + code) :
  - **A11y / WCAG 2.1 AA** — `aria-controls` + `aria-busy` + dedicated `aria-label` on the spec region (different key from the trigger button, no double-announcement for SR users), `tabIndex={0}` on the scroll container (keyboard-scrollable per SC 2.1.1), `prefers-reduced-motion: reduce` opt-out on the chevron rotation, new `.set-icon-btn:focus-visible` rule.
  - **Light / chartreuse theme contrast** — new `.set-sourcing-spec` class **pins** `--kr-text-primary` / `--kr-text-secondary` / `--kr-text-ghost` to dark-theme values inside the spec panel (`--kr-bg-code-panel` is intentionally dark in every theme, but the text tokens flip with the theme — without pinning, light/chartreuse rendered dark-on-dark). 4th occurrence of the same footgun pattern, now documented (`feedback_bg_code_panel_pin_text`).
  - **FE code resilience** — server-response typeguard `isAhMode` (corrupted / unknown mode string falls back to `warn` instead of being cast through), save-failure rollback + error toast (was silently swallowing on `.catch`), spec-fetch retry-after-error (previous bug : `if (specContent || specError)` shortcut prevented re-fetch — second click now retries), `useEffect` cleanup flag (no `setState` on unmount).
  - **UX wording** — "jail" / jargon stripped from the Warn explanation in FR/EN/ES, ES title fixed from `Sourcing y anti-alucinación` (English leak) to `Citación de fuentes y anti-alucinación`.
- **Backend** — new `/api/conventions/agents-md-format-v1` route + `setup::get_agents_md_spec_v1` handler returning the constant as `text/markdown; charset=utf-8`. Constant lives at `core::anti_halluc::SPEC_AGENTS_MD_V1` via `include_str!("../../docs/conventions/agents-md-format-v1.md")` (relocated under `backend/` so the file ships with the Docker builder context ; `backend/Dockerfile` gains a `COPY docs ./docs` to plumb it).
- **Tests** — 12 frontend unit tests on `AntiHallucSection` (mount-load + warn fallback, typeguard against unknown server value, save-success toast, save-failure rollback + error toast, enforce-preview disclosure toast, 3 explanations rendered once, spec-fetch first-click + no-refetch on toggle, error path + retry-after-error, `aria-controls` / `aria-busy`, region `role`+`tabindex`+`aria-label`) + 1 backend route integration test (`conventions_route_returns_markdown_spec` : 200 + `text/markdown` content-type + body byte-equal to the embedded constant). Backend now 2498, frontend 1603, all gates green (clippy `-D warnings`, tsc, i18n parity, Python 137).

### Added — Phase 4 (MCP Remote Control PR2 + PR3, carried over from 0.8.6)

- **`qp_batch_run` + `workflow_run_discussions` MCP tools (PR2)** + **`workflow_wait_for_completion` (PR3)** (`backend/src/api/mcp_remote.rs` 3 handlers + routes in `lib.rs` + Python wrappers/defs/DISPATCH in `disc-introspection-mcp.py`). Completes the mobile remote-control surface started in 0.8.6 PR1. `qp_batch_run` fans a Quick Prompt out to N discussions in one call (per-item `vars`, max 50, server-side fire-and-forget kickoff throttled by the agent semaphore) and returns a trackable batch `run_id`. `workflow_run_discussions` lists a run's spawned child discs (new targeted DB query `db::discussions::list_discussions_by_run`, backed by a shared `map_discussion_row` extracted from `list_discussions_paginated` to kill row-mapping duplication). `workflow_wait_for_completion` long-polls a run to terminal status or a clamped `[1,60]s` timeout, returning a `next_check` hint on timeout (`WaitingApproval` is deliberately non-terminal so Gate'd workflows time out by design). `workflow_run_status`'s description now points at the shipped `workflow_run_discussions` (dropped the "when shipped" hedge). **Tests**: +3 Rust unit (`default_batch_item_title`, `clamp_wait_timeout`) + 1 DB test (`list_discussions_by_run` filter/order/empty) + 4 Python test classes (batch coercion/validation/inheritance, run-discussions GET, wait timeout-int coercion, PR2/PR3 tool discovery+dispatch). 137 Python tests green.

### Added — Anti-hallucination architecture pivot : `docs/AGENTS.md` becomes the canonical source (PR1+PR2+PR3)

Multi-agent design review (5 domain experts + Kronn tech-lead fact-checker, 3 rounds to consensus) pivoted the anti-hallu architecture away from runtime-injected directives toward a **single source of truth in the project's `docs/AGENTS.md`**, so the discipline applies with OR without Kronn (any CLI reads it natively) and Kronn becomes the *tooling* layer (mechanical `[src:]` verification + pill + gates), not the carrier.

- **PR1 — audit STEP 0 + canonical section.** `docs/AGENTS.md` now opens with a `<!-- kronn:section name="anti-hallu" curated="ai" audit="<date>" -->` block (the cascade : read code → docs → external → ask → never assert without proof + the `[src:]` citation grammar). A new **deterministic** audit STEP 0 (`api::audit::anti_hallu_step`, NOT an LLM call) inserts/refreshes it before the 10 numbered steps, idempotent. Bootstrap + `install_template` also drop the convention spec into the project at `docs/conventions/agents-md-format-v1.md`. A `<!-- kronn:spec=… -->` self-describing header points any agent at the spec.
- **PR2 — de-duplication.** The runtime `core::anti_halluc::PREAMBLE` (280 words) collapses to a ~80-word **pointer** toward `docs/AGENTS.md`; the audit `PROMPT_PREAMBLE` (450 words) drops to a short preamble; the 3-lang `anti_halluc_doc_writer_block` becomes a 1-line pointer; the 11 sensitive skills' "Sourcing discipline" sections become 1-line pointers. New anti-regression tests pin no step re-inlines the doctrine + a 50-message **false-positive corpus** asserting ≤5% FP on the niveau-0 heuristic. CLAIM_CUES expanded for CVE/version vocabulary (real disc surfaced the gap).
- **PR3 — migrate existing projects.** `GET /api/projects/:id/anti-hallu/status`, `POST …/anti-hallu/inject` (idempotent), `POST …/redirectors/sync` + a ProjectCard badge ("Anti-hallu v1" / "inject", i18n FR/EN/ES) so legacy projects adopt the section in one click.
- **Open convention split (core vs RFC).** The spec was slimmed to a present-tense core (`backend/docs/conventions/agents-md-format-v1.md`, mirrored at repo-root with a byte-identity test) and the advanced/future semantics (status lifecycle, `claim_id` references, source-map aliases, runtime freshness, claim propagation) moved to `docs/research/provenance-rfcs.md` — keeping the convention adoptable in 30 seconds and honest about what 0.8.7 actually enforces.

### Added — Agent usage & cost panel (via `ccusage`)

- **`core::usage` + `GET /api/usage` + Settings "Agent usage & cost" card** (`UsageSection.tsx`, an RTK-style "eco mode" card placed in Settings, i18n FR/EN/ES). Shells out to **ccusage** (pre-installed in the Docker image, pinned `20.0.5`) to report the REAL token + cache breakdown + up-to-date pricing of the detected CLIs (Claude / Codex / Gemini …), read from their local logs. Replaces the blind spot of `core::pricing::estimate_cost`, which (a) used a static table + a guessed 60/40 split ignoring prompt caching (over-estimating cache-heavy sessions ~6×) and (b) only counted tokens that passed *through Kronn*, not the user's direct CLI sessions. Always-visible daily/weekly/monthly toggle + total cost/tokens + per-agent chips; a "Details" disclosure reveals a per-agent cost bar (rolled up from ccusage `modelBreakdowns`) + a paginated recent-periods table (per-bucket page size — 30 days / 15 weeks / 12 months; ccusage stamps only a bucket start, so weekly renders as a `start → end` range and monthly as a localised month). Reads host logs from the container via `HOME=/host-home` + a writable npm cache; ccusage's native binary is chowned to the runtime user so its first-run self-chmod succeeds. Per-Kronn-project attribution (session↔disc correlation) deliberately deferred. **Tests**: 5 `core::usage` unit (parse real shape incl. per-model token-sum, agents-from-metadata, empty/missing totals, garbage rejection, period whitelist anti-injection) + 8 frontend `UsageSection` tests (formatPeriod weekly-range/monthly-label/daily-passthrough/garbage-fallback, rowsPerPage per-bucket contract, pagination cap + next + single-page hide) + SettingsPage coverage (testids, period toggle, details). Validated live (56 days, agents claude/codex/gemini).

## [0.8.6] - 2026-05-23

Released 2026-05-23 (tag `0.8.6`), bundling all phases: Phase 1-2 (advanced features) + Phase 3 (pré-0.9.0 cleanup pass) + Phase 4 PR1 (MCP Remote Control) + the `qa_*` tools (PR1.6/1.7/1.8). Note: Phase 4 **PR2/PR3 did NOT ship** in 0.8.6 — they carry over to 0.8.7.

### Added — Phase 4 (MCP Remote Control PR 1)

- **3 new MCP tools to launch + track from a phone** (`workflow_trigger`, `workflow_run_status`, `qp_run` ; backend route module `api::mcp_remote` + shared smart-polling helper `core::run_eta` + Python wrappers in `disc-introspection-mcp.py`). Use case : Claude Code mobile linked to a PC session → MCP `kronn-internal` → launches a workflow or QP, polls progress, reads result discs — without ever opening the desktop UI. Each tool returns a `next_check: {wait_seconds, reason, confidence}` hint computed from historical averages (`workflow_runs.total_duration_ms` window of 10 completed runs / `qp_versions.avg_first_agent_duration_ms` weighted across versions). Mobile token cost down ~80% vs naïve 10s polling : the agent waits the suggested interval, calls back exactly when the run should be done. `next_check: null` on terminal status (`Success | Failed | Cancelled | StoppedByGuard`) signals "stop polling". Three confidence tiers : `baseline` (≥3 samples), `no_baseline` (cold start, 60s fallback), `overshoot` (past the average, 30s fixed backoff). `qp_run` server-side spawns the agent in a tokio task with the SSE handle dropped — the agent completes regardless of the MCP wrapper's lifetime (channel senders use `let _ = tx.send(...)` so a dropped receiver doesn't cancel the run). **20 tests** (9 `run_eta` polling-decision tests with boundary cases incl. char-boundary safety for French/emoji ; 9 `mcp_remote` shape + render-template + excerpt-truncation + terminal-status filtering tests ; 20 Python wrapper tests pinning the contract — required-field validation, str-coercion for LLM-typed ints, project-id inheritance, tool-listing discovery, dispatch wiring, next_check description references).

- **`qa_update` MCP tool + `qa_create_draft` probe-then-persist guidance** (PR 1.8). Closes the after-test iteration loop flagged during 0.8.6 phase 4 E2E test : an agent had drafted a JIRA-fetch QA without `api_extract`, the live test returned a 12k-token payload (changelog + ADF + renderedFields), but the agent had no MCP route to patch the QA — UI friction. **(A)** `qa_create_draft` description rewritten to push a PROBE-then-PERSIST workflow : (1) `api_call` once with `extract: null` to discover the response shape, (2) decide on the JSONPath that keeps only what downstream agents need, (3) `qa_create_draft` with optimised `api_extract` + vendor-side `api_query` filters. Concrete token-size anchors (10-40k for verbose vendors) and an explicit pointer to `qa_update` for post-test iteration. No vendor-specific JSONPath recipes (intentionally — those would rot as APIs evolve). **(B)** New `qa_update({qa_id, ...patch})` MCP tool. Backend route `PUT /api/quick-apis/:id` already existed but its bare-PUT semantics RESET `variables` / `profile_ids` / `directive_ids` to empty when those fields are absent — hostile UX for partial patches. The wrapper does load-merge-write : GET the QA from the list endpoint, merge the agent's patch field-by-field, PUT the full merged body back. Agent passes only what CHANGES (typically just `api_extract` or `api_query`), the rest stays intact. Required-field re-validation after merge + 200-char name cap as defensive guards. **11 new Python tests** (7 wrapper contract — missing-id, unknown-qa hint, extract-only patch preserves siblings, explicit empty list clears, multi-field patch, long-name rejection, discovery + dispatch ; 4 description-guidance pins ensuring PROBE / api_call / 10-40k tokens / qa_update reference all stay in the qa_create_draft prose).

- **`qa_create_draft` MCP tool — symmetry fix for the `*_create_draft` cluster** (Python wrapper around pre-existing `POST /api/quick-apis`). Closes the gap noticed during PR 1 review : `workflow_create_draft` + `qp_create_draft` existed but the QA equivalent was missing, forcing agents who wanted to persist a recurring API pattern to ask the user to click through the Quick APIs page UI. Now an agent can SAVE a QA in one tool call after converging on the request shape, then INVOKE it via `qa_run`. 4 required fields (`name`, `api_plugin_slug`, `api_config_id`, `api_endpoint_path`) validated client-side before HTTP so the agent gets a clean error instead of a 422. Project_id auto-inherited from current disc (same UX pattern as the rest of the cluster). 200-char name cap mirrors `qp_create_draft`. QAs have no `enabled` flag so the "draft" semantic is identical to `qp_create_draft` — no auto-fire risk, user reviews + launches via Quick APIs page when ready. **10 new Python tests** (6 wrapper contract — required-field validation x 4, name-length cap, route + payload pass-through, auto-inherit, explicit-wins, discovery + dispatch ; 4 cluster-symmetry assertions pinning the 3 `*_create_draft` tools form a coherent surface).

- **`qa_run` MCP tool — synchronous Quick API execution** (Python wrapper around the pre-existing `POST /api/quick-apis/:id/run` route ; `qa_list` enriched with `variables[]`). The deagentified twin of `api_call` : agents pass just `{qa_id, vars}` — the QA already encodes endpoint, method, headers, query, body, extract, pagination, so the agent never reconstructs request shapes. Synchronous (no `next_check` — QAs run sub-second to a few seconds), returns the parsed envelope `{success, duration_ms, envelope, error?}` inline. `vars` is renamed to `variables` at the wire (matches the backend serde shape), values are coerced to strings for defensive handling of LLM-typed ints. The enriched `qa_list` exposes `variables[{name, label, required, description}]` so agents discover what to pass without an extra `GET /api/quick-apis/<id>` round-trip. Empty descriptions are normalised to `None` so agents can branch on truthiness cleanly. The tool description explicitly calls out the absence of `next_check` and recommends `qa_run` over `api_call` whenever a matching QA exists — keeps the API broker as the low-level fallback. **12 new Python tests** (6 wrapper contract tests for missing-id / vars rename / str-coercion / unwrapped envelope / propagated errors ; 4 `qa_list` enrichment tests for variables shape / empty-description normalisation / zero-var QAs / legacy-field preservation ; 2 discovery-contract assertions for the 4-tools cluster + sync vs async description split).

### Phase 3 (pré-0.9.0 cleanup pass)

**Cleanup pass avant Continual Learning.** 7 PRs ciblées pour qu'aucune dette ne gêne le brief 0.9.0. Zéro feature breaking, juste de la mise en ordre.

### Added — Phase 3 (cleanup batch)

- **`core::redact` shared Rust module** (`backend/src/core/redact.rs` — new). Promotes the ad-hoc redactor that lived inside `db/api_call_logs.rs` into a first-class crate utility used by ALL future secret-sensitive log surfaces (including the upcoming 0.9.0 `learning_candidates` extractor). Patterns ported from `frontend/src/lib/bug-report.ts::redactSecrets` + extended : Authorization headers (Bearer/Basic/Token/Digest), JSON credential fields (`password`, `token`, `api_key`, `access_token`, `refresh_token`, `client_secret`, `private_key`), connection strings with embedded creds (postgres, mongodb+srv, mysql, redis, amqp), bare Bearer in logs, vendor prefixes (`sk-`, `p8e-`, `AIzaSy`, `gh[opsur]_`, `xox[abprs]-`), JWTs, AWS access keys, Stripe live/test keys. Exports two entry points : `redact_secrets(input)` for in-place masking + `looks_like_secret(input)` for fast boolean refusal (used by learning_candidates to reject secret-y content before persistence). Idempotent + UTF-8 boundary safe. Bonus contract change : `db::api_call_logs::redact_secrets` now re-exports the shared module, so the broker excerpts gain the wider regex coverage automatically. **30 unit tests** (per-pattern positive + false-positive guards + UTF-8/emoji boundary + multi-secret + idempotence).

- **Flaky `notify_renders_templates_in_url_and_body` fixed** (`backend/src/workflows/notify_step.rs`). Pre-fix the two `notify_*` tests bound a std `TcpListener` to port 0, read the port, dropped the listener, then re-bound with tokio — race window between drop and re-bind let a parallel test grab the port and flake the run. Fix : bind directly with `tokio::net::TcpListener::bind("127.0.0.1:0").await`, read `local_addr().port()` from the already-tokio listener, no drop. 5×stress-tested with `--test-threads=8`, all green.

- **`disc_summary` / continual-learning archive collision audit** (new memory `project_continual_learning_archive_unify.md`). Read the existing archive-time flow in `backend/src/api/discussions/streaming.rs:967-989` + the 0.9.0 brief's PR #4 plan. Decision : manual archive button triggers the future learning-validation modal, auto-archive (signal-driven, e.g. `KRONN:BOOTSTRAP_COMPLETE`) does NOT — popping a modal mid-pipeline is jarring. Pending learning candidates persist regardless of archive path + are surfaced via a global badge in `ChatHeader.tsx` (per the 0.9.0 PR #2 design). No collision with `summary_cache` (that's a compaction cache, not an archive ritual). Adjustment documented in the new memory ; 0.9.0 PR #4 scope unchanged otherwise.

- **Workflow + manual_test api_call logging** (`backend/src/workflows/api_call_executor.rs`). The 0.8.6 phase 2 audit table only captured `agent_broker` calls. Now also captures workflow runs (source=workflow, with `run_id` plumbed via new `ApiCallLogContext` plumbed through the executor entry point + batch fan-out) AND wizard "Test the call" + `/api/quick-apis/:id/run` (source=manual_test). New entry point `execute_api_call_step_with_db_as(... log_ctx)` for callers that want to override the default workflow source ; the old `execute_api_call_step_with_db(...)` signature is preserved and defaults to workflow logging. Batch `BatchApiCall` records ONE row per item (the executor fans out before the recording hook). Best-effort : DB errors NEVER short-circuit the step. **8 tests** added (parse `[SIGNAL: http_NNN]`, log context defaults, workflow / manual_test integration with in-memory DB).

- **Custom plugin rename orphan-env warning** (`backend/src/api/mcps.rs::compute_orphan_env_keys` pure-fn + `cleanup_orphan_env` route + 6 backend tests, FE toast + confirm prompt + cleanup button + 3 FE tests + 5 i18n keys × 3 langs). When a user renames a field on a Custom plugin via the edit drawer, the OLD env key may still exist in encrypted env on OTHER configs of the same plugin (multi-project setup). Pre-fix : silent orphan leaked through `host_sync` to disk. Post-fix : `PUT /api/mcps/custom/:server_id` now returns `{ server, orphan_env_keys }`. If non-empty, FE prompts the user "{N} ancienne(s) clé(s) d'env reste(nt) sur d'autres configs… Les nettoyer maintenant ?" + on confirm POSTs to the new `/cleanup-orphan-env` route. Secret-safe by construction : only KEY NAMES travel through the response/request, never values. Pure ranker = unit-testable without a DB.

- **API call logs UI page** (`frontend/src/pages/ApiCallLogsPage.tsx` + `.css` ; new `'api-logs'` nav tab in Dashboard ; 9 vitest tests ; ~32 i18n keys × 3 langs). Read surface for the audit table built by phase 2 broker + phase 3 workflow/manual logging. Filter bar (source × 4 chips / status × 5 chips / plugin slug input) + sortable table + click-to-drawer with redacted excerpts visible + auto-refresh toggle every 10s. The drawer hint reminds the user that excerpts go through `core::redact` so secrets are masked. UI page closes the "we have no visibility into API calls" gap user flagged.

- **Dropdown migration — 5 native `<select>` sites** (`ApiCallStepCard.tsx` HTTP method override, `ProjectLinkedRepos.tsx` kind picker, `NewDiscussionForm.tsx` agent picker, `QuickApiForm.tsx` project picker, `IdentitySection.tsx` global context mode). Each site replaces a native `<select>` with the theme-aware `<Dropdown<T>>` shipped in phase 2 — closes Firefox/Safari theme parity gaps (those browsers render `<option>` via OS chrome and ignore page CSS). 5 existing tests updated to use the new testIds. 11 remaining `<select>` sites can migrate incrementally without breaking the component contract.

- **Path B file-based plugin import / export** (`backend/src/api/mcps.rs::export_custom_plugin_file` + `import_custom_plugin_file` + `sanitize_imported_payload` pure fn + `sanitize_filename` pure fn + 6 backend tests ; FE "Télécharger le fichier" button in the export modal blob-downloads the same JSON the clipboard sees ; FE "Importer un fichier" input on the import form reads the user's `.json` via `FileReader` and POSTs as JSON ; 5 i18n keys × 3 langs). Secret-safe by construction : `build_custom_plugin_export` empties `fields[].value` BEFORE serialisation, and `sanitize_imported_payload` re-strips any values found in an imported file (defense-in-depth against hand-crafted files). No multipart parser needed — backend accepts the same `application/json` body shape on both paste (Path A) and file-upload (Path B) flows. Path B unlocks the "community plugin library" scenario (drop a `.kronn-plugin.json` from GitHub Gist into Kronn).

- **STT silent-failure fix** (`frontend/src/lib/stt-engine.ts` adds `onStatus` callback option + 5 new tests, `frontend/src/components/ChatInput.tsx` adds `'loading'` state + dedicated banner + toast on error / empty-text / empty-audio + trim-concat for existing input ; 4 i18n keys × 3 langs). Pre-fix the user clicked mic → recording visible → stopped → "rien" : transcription failed silently because the catch was `console.error` only (no toast), the worker's `status: 'loading'` events were ignored (no UI feedback during the 30s-2min first-time model download), empty audio threw a cryptic error, and Whisper returning `""` triggered no signal. Post-fix: every failure mode toasts a clear message, the model-download phase shows a dedicated "Téléchargement du modèle vocal (1ʳᵉ utilisation)…" banner, and existing-input concatenation trims whitespace properly (no more `"hello  world"` double-spaces).

- **Workflow detail — Steps panel collapses to a compact pipeline** (`frontend/src/components/workflows/WorkflowDetail.tsx` + `WorkflowsPage.css` ; 6 vitest tests ; 7 i18n keys × 3 langs). The Steps section dumped every per-step card (prompt + Test button) stacked vertically — heavy and rarely what you want at a glance, especially mid-run. It now collapses by default to a horizontal pipeline (`number + kind icon + name`) prefixed by an agent-vs-deterministic count split, with a "Voir en détails" toggle that reveals the legacy cards. Color encodes the ONLY distinction that matters at a glance — agent (LLM → costs tokens, violet) vs deterministic (0 token, green) — instead of a 6-color per-type rainbow ; the step *type* is carried by the chip icon. Agent steps also surface the agent name + brand color under the label (same `AGENT_COLORS` / `AGENT_LABELS` as the detail card, so both views read the same identity).

### Fixed — Phase 3 (cleanup batch)

- **MCP page — duplicate kind filter + Custom API mis-filtered** (`frontend/src/pages/McpPage.tsx` + `McpPage.css` ; 1 new vitest test). The Add-plugin panel had TWO kind filters : the new top row (`All / MCP / API / CLI`, drives `addMcpKindFilter`) and a leftover `MCP / API` pair inside the category pills (driven by a now-dead `kindFilter` state). The pinned Custom API tile was gated on that dead `kindFilter` (always null), so it stayed visible under the MCP and CLI filters even though it is an API-only plugin. Fix : removed the redundant kind buttons + the dead state + the redundant per-card re-filter, and gated the Custom API / Import-JSON tiles on `addMcpKindFilter` (visible under All/API, hidden under MCP/CLI). Page subtitle bumped to "(MCP / API / CLI)".

### Site — Phase 3 (cleanup batch)

- **OG social-preview image** (`site/og-image.png`, 1200×630). The `og:image` / `twitter:image` meta declared since Phase 2 pointed at a non-existent file (graceful text-only fallback until now). Added the real card — an AutoPilot workflow screenshot showcasing the new compact pipeline — cropped to the exact 1.91:1 OG ratio so it matches the already-declared `og:image:width/height`.

### Notes
- All 2133 backend tests + 63 Python tests + 1454 frontend tests pass ; cargo clippy clean ; `make lint-backend-local` exit 0.
- Released 2026-05-23 as tag `0.8.6` (phases 1-4 bundled). Phase 4 PR2/PR3 deferred to 0.8.7.
- Zero feature changes shipped to end users beyond the audit-log read UI + STT progress feedback. This phase is dette-paid + 0.9.0 prep.

### Phase 2 (2026-05-21) — post-merge quick-wins batch

Bundle de maintenance après la phase 1 du 2026-05-20 : 9 quick-wins shippés en une journée pour combler les gaps surfacés pendant la validation Didomi + les UX papercuts (Fichiers panel vide sur worktree, "+ N autres" imcliquable, "Copier comme JSON" silencieux dans Tauri, etc.). Aucune feature breaking, aucun bump.

### Added — Phase 2 (2026-05-21)

- **Disc Fichiers panel — committed-on-branch section** (`backend/src/api/git_ops.rs::run_git_status` returns `committed_files: Vec<GitFileStatus>` parsed from `git diff --name-status <default>...HEAD` ; `frontend/src/components/GitPanel.tsx` renders a second list "{N} fichier(s) déjà commit(s) sur cette branche (vs main)" + new `.git-committed-section` CSS ; +7 tests). Pre-fix the "Fichiers" tab showed *"Aucune modification"* on worktree-isolated discs once the agent committed — the working tree was clean and `git status --porcelain` returned empty. Now the disc's *cumulative* delta vs main is surfaced too, so a user looking at a Feasibility-Gated / batch disc sees what would land in the PR even when nothing is uncommitted. Section is hidden on default branch + when no default branch resolves. Pure-fn `parse_committed_diff` covers rename/copy paths (destination-path semantics). Cf. [[project_disc_files_panel_branch_diff]].

- **Custom plugin import / export — clipboard JSON + inline export modal** (`frontend/src/pages/McpPage.tsx` : "Copier comme JSON" button in detail header opens a modal with the JSON in a readonly textarea + best-effort clipboard write + execCommand fallback ; "Importer depuis JSON" tile next to Custom API tile, inline paste form ; `buildCustomPluginExport` + `parseCustomPluginImport` pure helpers ; 21 i18n keys × 3 langs ; +7 tests). Spec-only sharing : `fields[].value` is ALWAYS stripped on both export AND import — credentials never leak. Imported plugins land disabled-state (user fills env via "Edit secrets" afterwards). Reuses `POST /api/mcps/configs` with `custom_spec` — zero backend change. Validation light : `name` + `base_url` required, `auth` discriminator-shape checked, unknown variants fall back to `None`. **Modal addition 2026-05-21** : the initial clipboard-only export was silently failing inside Tauri's webview ("STRICTEMENT rien" reported live) — fix surfaces the JSON in a modal regardless of clipboard outcome so the user can always ctrl+C the pre-selected textarea. Path B (file-based download/upload) deferred ; clipboard+modal covers 80% of "share with a teammate / move to a new machine" cases. Cf. [[project_plugin_import_export_0_8_6]].

- **`linked_repos` picker — show-more toggle** (`frontend/src/components/ProjectLinkedRepos.tsx` : the "+ N autres" overflow label was rendered as a static `<span>` (uncliquable) → user reported "ne sert à rien" 2026-05-21. Fix : real `<button>` with state toggle `showAllCandidates`, reveals all hidden candidates on click and offers a "Voir moins" button to collapse back. Reset on draft cancel. 3 i18n keys × 3 langs ; 4 unit tests on the toggle behaviour.

- **Theme-aware `<Dropdown>` component — replaces native `<select>` for Firefox/Safari parity** (`frontend/src/components/Dropdown.tsx` + `.css` + 8 unit tests ; first migration site = TokenExchange inject picker in McpPage). Surfaced 2026-05-19 : the new auth picker's `<select>` rendered black-on-white on Firefox/Safari because they delegate `<option>` rendering to the OS chrome (ignores page CSS). Custom popover with full a11y : `role="listbox"`, ArrowUp/Down/Home/End/Enter/Space/Esc keyboard nav, click-outside close, focus-trap return to trigger, disabled options. Generic `<Dropdown<V>>` typed by the value union → reusable across the app (TokenInjection picker migrated as proof). Other native `<select>` sites can migrate incrementally without breaking the type signature. Cf. [[project_dropdown_native_options_theme]].

- **Unified API call logs — persistent audit table + read API** (`backend/src/db/sql/061_api_call_logs.sql` migration + `backend/src/db/api_call_logs.rs` with `record`, `list`, `get`, `purge_older_than` + 11 tests including secret-redaction + utf8-boundary-safe excerpt truncation ; `backend/src/api/api_call_logs.rs` exposes `GET /api/api-call-logs`, `GET /api/api-call-logs/:id`, `POST /api/api-call-logs/purge` ; broker route `agent_api_call` now records every call best-effort, NEVER short-circuits the agent response on a logging error ; `frontend/src/lib/api.ts::apiCallLogs` client wrapper). Closes the 0.8.6 "we have no visibility into API calls" gap user flagged. Captures source (workflow/agent_broker/manual_test), project, disc, run, agent, plugin+config, endpoint+method, http_status, duration, request+response excerpts (capped at 2KB after best-effort regex redaction of Bearer/Basic auth headers + vendor prefixes p8e-/AIzaSy/sk-/ghp_/xoxb-/JWT), error_message. Indexed on (project, plugin, run, disc, called_at DESC) for the common filter axes. UI page deferred ; the data is captured now so we don't lose history while the UI is built. Cf. [[project_api_call_logs_0_8_6]].

- **`disc_invite_peer` + `disc_create_room` MCP tools — full-MCP cross-agent bootstrap** (`backend/scripts/disc-introspection-mcp.py` adds two `disc_*` tools mirroring the existing `POST /api/discussions/:id/invite-peer` route + 5 Python tests). User flagged 2026-05-21 : *"ça pourrait éviter d'aller sur Kronn pour ça ^^ on fait tout en full MCP"*. Before, an agent that wanted to spin up a multi-agent room had to ask the user to click [+ Inviter] in the Kronn UI to mint a token. Now `disc_invite_peer({})` mints from the currently-bound disc and `disc_create_room({title})` chains disc_create + invite-peer in one call → returns `{disc_id, title, token, instruction_text, expires_at}`. Combined with `disc_join`, an agent can bootstrap a collab room → greet → wait for peer in ~3 tool calls, user just sees the result in the UI. Closes the last UI-required gap for full-MCP cross-agent flows. Cf. [[project_disc_create_via_mcp_0_8_7]] + [[project_cross_agent_collab_demo]].

- **Gate checkpoint commit (auto-commit before Gate, auto-reset on Goto)** (`backend/src/workflows/gate_checkpoint.rs` new module + `WorkflowStep.gate_checkpoint_before: Option<bool>` field + 6 tests). Wired in `workflows::runner` : before a Gate fires its decision-pending notify, if the step is in an Isolated-mode worktree AND `gate_checkpoint_before=true`, runner runs `git add -A && commit --allow-empty -m "kronn checkpoint: <step>"` and stashes the sha in `state.checkpoint:<step>`. On the resume_run RequestChanges/Goto branch, the runner reads that sha back and `git reset --hard <sha>` before continuing — the Gate→implement→re-review loop is now idempotent (no leftover artefacts from a rejected attempt poisoning the next one). Opt-in per Gate step ; off by default to keep existing flows untouched.

- **Gate auto-approve countdown** (`WorkflowStep.gate_auto_approve_after_secs: Option<u32>` + validator rejecting 0 + values >86400 in `validate_required_fields_per_type` + 5 tests + runner spawn). When a Gate carries `gate_auto_approve_after_secs=N`, the runner spawns a `tokio::spawn` timer right after firing the gate-pending notify that POSTs `Approve` to `/api/workflows/runs/:id/decide` after N seconds. Lets a long-running pipeline keep moving overnight without a human in the loop, while still allowing manual override before the timer fires. Bounded to 1-86400 by the validator (sanity guard against forever-or-instant misconfigs).

- **`linked_repos` picker** (`backend/src/api/projects/crud.rs::rank_linked_repos_candidates` pure function + `LinkedRepoCandidate` struct + `GET /api/projects/:id/linked-repos/candidates` route + 5 tests ; `frontend/src/components/ProjectLinkedRepos.tsx` chip-based picker visible in draft mode). Ranks other Kronn projects by path proximity (same-parent first, alphabetical fallback) so the user can pick 1-click instead of remembering project ids by heart. Excludes already-linked projects from the candidate list.

- **`.kronn.json` audit-state backfill** (`backend/src/core/kronn_state.rs::backfill_from_legacy_state` + 7 tests + wired in `scanner::analyze_audit_state`). Auto-creates `docs/.kronn.json` from legacy state signals (`docs/checksums.json`, `KRONN:VALIDATED` / `KRONN:BOOTSTRAPPED` markers in AGENTS.md) when first scanning a pre-0.8.4 project — kills false-positive "Not Audited" chips on projects audited before the canonical state file existed. Idempotent : skips if `.kronn.json` already present, returns false instead of overwriting.

- **Custom plugin auto-discovery banner — Part A UI hint** (`McpPage.tsx::isLegacyCustomNoEndpoints` detection + `.mcp-autodiscovery-banner` block + 3 i18n keys × 3 langs + 3 tests). On Custom plugins with `server_id` starting `custom-` AND empty `api_spec.endpoints[]`, surfaces an info banner pushing the user toward the AI helper. CTA reuses `openEditCustomPlugin` so the helper is one click from the detail panel. Hidden on registry plugins + on Custom plugins that already declared endpoints.

### Site updates — Phase 2 (2026-05-21)
- **STAGE 04 réécrit (3 langs)** : ex-"Multi-CLI / Onglet Plugins, connecte un second CLI" (pas-à-pas incorrect, signalé live) → "Workflow depuis un template" pointing to *Automation → Workflows → Nouveau → Ticket-to-PR / Feasibility-Autopilot / Feature Planner preset*. Note de fin lie aux Quick Prompts du STAGE 03 comme building blocks.
- **Glossaire concept "Plugin"** : "Custom (declare your own endpoints)" → "Custom API (declare your own endpoints)" — aligné sur le nom de la tuile dans McpPage et les `custom-{slug}-{nano}` IDs.

### Notes
- All 2080 backend tests + 63 Python tests + 1437 frontend tests pass ; cargo clippy clean (`make lint-backend-local`).
- VERSION stays on 0.8.6 — this is the **phase 2 maintenance bundle** of the 0.8.6 release, shipped as a separate PR / commit on the same version.


### Phase 1 (2026-05-20) — Agent ↔ Kronn ↔ APIs : la boucle se ferme

L'agent appelle les APIs configurées dans Kronn directement, sans jamais voir les credentials — Kronn handles auth, substitue les non-secrets via `${ENV.X}`, log tout. Discovery (mcp_list enrichi + hint), call direct (broker), edit unifié (spec + values en un seul form), auth schemes complets (Bearer + TokenExchange générique), endpoints auto via AI helper.

**Validé live sur l'audit RGPD Didomi 2026-05-20.** Didomi (token-exchange JSON body, organization_id en query) — débugué + shippé en 8h de dogfooding intense. Le pattern token-exchange + `${ENV.X}` substitution unblocks toute API enterprise-style à 2-step auth (Auth0 M2M, Salesforce, Stripe-Connect-style).

### Added

- **`mcp_list` enrichi : `description`, `docs_url`, per-endpoint `description` + `side_effect`, `is_custom` (corrigé), `hint`** (`backend/scripts/disc-introspection-mcp.py::call_mcp_list` + 4 tests). 0.8.5 ne surfaçait que la slug + endpoints (path/method), insuffisant pour qu'un agent décide *quel* plugin appeler sans demander à l'utilisateur. Maintenant chaque server expose : description (fallback server-level si api_spec.description est vide), docs_url, tags, `is_custom` (détecte `custom-{slug}-{nano}` ET la sentinelle `api-custom`, cf. `mcps.rs:82-86`), endpoints enrichis (description optionnelle + flag `side_effect`), et surtout un `hint` machine-actionable à 3 préfixes : `READY` (endpoints déclarés, ApiCall directe OK) / `NEEDS_RESEARCH` (endpoints vides mais `docs_url` connu — fetch la doc + propose endpoints) / `AMBIGUOUS` (rien d'actionnable, demander à l'user). Le hint embarque le `docs_url` verbatim dans le texte pour que l'agent puisse fetch sans 2e tool call. Cf. [[project_endpoints_autodiscovery_0_8_6]] pour la suite (UI banner + agent auto-declaration des endpoints).

- **Custom plugin auth-kind picker + generic `TokenExchange` variant** (`backend/src/models/mcp.rs::ApiAuthKind` gagne `TokenExchange { endpoint, method, body_template, body_format, token_jsonpath, ttl_seconds, inject, creds_env_keys }` + `TokenExchangeBodyFormat::{Json, FormUrlEncoded}` + `TokenInjection::{BearerHeader, CustomHeader, QueryParam}` ; `CustomApiPayload.auth: ApiAuthKind` exposé + propagé dans `materialize_custom_server` ; nouveau `core::oauth2_cache::resolve_token_exchange` réutilise le même Mutex<HashMap<config_id, CachedToken>> que OAuth2 pour la cache+TTL ; executor wire dans `execute_api_call_step_with_db` + `resolve_auth` route le token vers Bearer/Custom header/Query selon `inject` ; frontend Custom plugin form gagne une section Auth avec dropdown (None/Bearer/TokenExchange + Other quand variant ApiKey*/Basic*/OAuth2 d'un plugin existant) + sub-fields conditionnels pour TokenExchange (endpoint, method, body_template textarea avec ${ENV.KEY} interpolation, body_format, token_jsonpath, ttl_seconds, inject method) ; 20 nouvelles i18n keys × 3 langs). **Caught live 2026-05-19 sur l'audit RGPD Didomi** : l'agent dans une session Claude Code a appelé `api_call(/properties)` → 401 *"No access token"* parce que Didomi exige un flow 2-étapes (`POST /sessions {type:"api-key", key, secret}` → `{access_token}`) non couvert par le `OAuth2ClientCredentials` existant (qui hardcode form-encoded body + field names `client_id`/`client_secret`). Le pre-fix `materialize_custom_server` hardcodait `auth: ApiAuthKind::None` → tous les Custom plugins étaient muets côté auth, même pour un simple Bearer. Maintenant : (1) Custom plugin form expose 3 variants (None/Bearer/TokenExchange — les 4 autres viennent en 0.8.7 Layer A), (2) `TokenExchange` est générique (body template avec `${ENV.KEY}` interpolation, body JSON ou form-encoded, JSONPath custom pour l'extraction, TTL+refresh transparents via le cache OAuth2 existant), (3) le token mint/refresh est complètement invisible à l'agent — il appelle juste l'endpoint, Kronn injecte. Validé sur Didomi (POST /sessions JSON → `$.access_token` → Bearer). Backend tests : 2197 → toujours verts (la cache + le flow OAuth2 sont déjà tested ; le nouveau path partage l'infra). **Workaround pré-fix** : impossible, l'agent ne peut pas passer Authorization header (interdit par le tool MCP api_call par design). Cf. [[project_custom_plugin_auth_0_8_7]] + [[project_token_exchange_generic_0_9_0]] (mémoires consolidées dans cette release).

- **Agent API broker — host-CLI sessions no longer locked out** (`backend/src/api/agent_api.rs::AgentApiCallRequest` + `disc-introspection-mcp.py::call_api_call` + +1 Python test). Pre-fix le tool refusait outright si `KRONN_DISCUSSION_ID` était absent — ce qui locked out toute session Claude Code / Codex / Gemini lancée à la main dans un terminal (le env var n'est injecté QUE par Kronn quand il spawn l'agent depuis une disc). Caught live 2026-05-19 quand l'agent du user dans un `claude` terminal a fait `mcp_list` (✅ Didomi READY, 12 endpoints) puis `api_call(/properties)` → reject `KRONN_DISCUSSION_ID env var not set`. **Fix architectural** : project_id résolu maintenant via 3 sources priorisées : (1) `project_id` arg explicit, (2) `disc_id` → `disc.project_id` (Kronn-spawn case, behavior conservé), (3) **nouveau fallback** `api_config_id` → `config.project_ids[0]` (host-CLI). L'agent qui choisit un config via `mcp_list` a TOUJOURS son project derivable. `disc_id` devient `Option<String>`, plus de erreur "disc not found" quand absent. La doc du tool MCP énumère les 3 voies. +1 Python test (`test_missing_disc_id_no_longer_blocks_host_cli_sessions`) + 1 (`test_explicit_project_id_passes_through`).

- **Agent API broker — `POST /api/agent-api/call` + MCP tool `api_call`** (`backend/src/api/agent_api.rs` + 9 Rust tests, `backend/scripts/disc-introspection-mcp.py::call_api_call` + 8 Python tests). MVP du pattern killer : un agent (Claude Code, Codex, …) peut désormais invoquer une API Kronn-configurée **sans jamais voir les credentials**. Réutilise byte-pour-byte le `api_call_executor` des workflow ApiCall steps — zéro nouveau code d'exécution. Le tool MCP `api_call` accepte soit `api_plugin_slug` + `api_config_id`, soit `quick_api_id`, et `endpoint_path` (+ `method`/`path_params`/`query`/`headers`/`body`/`extract` optionnels). Le `disc_id` est auto-injecté depuis `KRONN_DISCUSSION_ID` côté Python (l'agent ne le passe pas, et SURTOUT ne passe jamais d'auth — la doc du tool refuse explicitement `auth`/`api_key`/`bearer`/etc.). Le backend résout le projet depuis la disc, hydrate l'env décrypté du config, applique l'auth scheme déclarée dans `ApiSpec.auth`, fire la call, et retourne l'envelope canonique `{success, data, status, summary, http_status, error?}`. **Deferred follow-up** : `side_effect` gate opt-in, rate-limit per-disc, audit log persistant ([[project_api_call_logs_0_8_6]]), UI counter. Le MVP shippe pour débloquer l'audit Didomi/RGPD en cours côté user. Cf. [[project_agent_api_broker_0_8_6]].

- **`WorkflowStep: Default`** (`backend/src/models/workflows.rs:255`). Nécessaire pour que le broker (`agent_api_call`) construise une `WorkflowStep` synthétique avec `..Default::default()` pour tous les champs non-ApiCall (prompt_template vide, exec_command None, notify_config None, batch_*  None, etc.). Tous les champs avaient déjà des Defaults (`AgentType`, `StepMode`, `StepOutputFormat` ajoutés en 0.8.5 ; le reste = Option/Vec), donc le derive est un free upgrade. Test guard : `synthetic_workflow_step_uses_defaults_for_non_apicall_fields`.

- **Custom API plugin — edit-existing-spec flow** (`PUT /api/mcps/custom/:server_id` route in `backend/src/api/mcps.rs::update_custom_spec` + 2 Rust tests, "Edit plugin" button in `frontend/src/pages/McpPage.tsx` config-detail panel + form reused in edit mode + submit branch + 3 i18n keys × 3 langs). Bug surfaced 2026-05-19 par user : *"on ne peut plus revenir dessus, ya pas moyen de pouvoir quand même l'avoir pour des modifications d'API ?"* — la seule voie pour fixer une typo / ajouter des endpoints sur un Custom plugin créé était delete + recreate. Maintenant un bouton **"Modifier le plugin"** dans le drawer existant ouvre le MÊME formulaire que la création, pre-rempli (name, base_url, description, docs_url, fields[].label, endpoints[]), et au save → `PUT /api/mcps/custom/:server_id` qui réutilise `materialize_custom_server`. **Invariants critiques préservés** : (1) `server_id` reste figé (le slug+nano est baked-in, sinon les configs et workflow ApiCall steps référençant ce server_id casseraient silencieusement) ; (2) `source` + `transport` re-imposés du prev row (pas de Manual→Registry sneak) ; (3) **encrypted env per-config NON touché** (l'edit drawer existant gère ça séparément). Le AI helper "Construire avec l'IA" est disponible dans le edit mode aussi → tu peux ajouter les endpoints Didomi (et autres) après-coup sans tout recréer. Délétion d'endpoints = sortie de l'allowlist → workflow ApiCall step qui les utilisait fail loud-and-clear au prochain run (pas de silent corruption). Cf. nouveau task #31.

- **Cross-agent collab — N CLI agents sur une disc partagée Kronn** (migration 060 `discussion_sessions` + `discussion_invite_tokens` tables, `backend/src/db/discussion_sessions.rs` + 18 tests, `backend/src/api/disc_invite.rs` avec 5 routes [invite-peer, peer-join, peer-leave, participants, wait] + 25 tests dont 2 E2E 2-peer dialogue full-handlers, `backend/scripts/disc-introspection-mcp.py` runtime disc-binding + `disc_join`/`disc_wait_for_peer`/`disc_leave` MCP tools + `disc_append` simple-mode `{content}` + 33 Python tests, `frontend/src/components/DiscParticipantsHeader.tsx` + modal + 6 vitest, `frontend/src/components/NewDiscussionForm.tsx` checkbox "Lancer un agent tout de suite" + 7 vitest, `frontend/src/lib/i18n.ts` ~30 keys × 3 langs). Le user lance N CLIs host-launched dans leurs terminaux (Claude, Codex, Gemini, Vibe...), clic UNE fois sur **[+ Inviter]** dans l'UI Kronn → token réutilisable 10 min collé à chaque CLI → tous joignent la même disc → dialogue free-form 100% via Kronn, **zéro messenger humain**. Validé live 2026-05-21 sur un match de tennis roleplay (Claude vs Codex, Vibe arbitre, 14 min, 59 messages, 1 point joué en 20 coups). Bridge auto-détecte l'agent_type via MCP `clientInfo.name` (Claude Code → ClaudeCode, codex-cli → Codex, vibe → Vibe) avec fallback `/proc/<ppid>/cmdline` quand clientInfo est inutilisable. `disc_wait_for_peer` long-poll bloquant (1-90s, exclude caller agent_type pour pas se réveiller soi-même). Pattern marketing-friendly : *"le cerveau partagé de tes CLI IA"*. Foundation pour le delegation pattern (next-level `agent_delegate({to, task, context})` à venir, cf. [[project_multi_agent_orchestrator]]).

- **Disc-first refactor — discussion devient un topic indépendant des sessions agents**. Avant 0.8.6 : `discussions.source_agent` baked-in, lifecycle disc = lifecycle session CLI, switch d'agent impossible sans nouvelle disc. Maintenant : `discussion_sessions(disc_id, agent_type, session_id, role, status, joined_at, left_at)` lie N sessions à 1 disc. Use cases gratuits : (1) **brainstorm sans agent** (créer une disc vide, écrire les idées, inviter un agent quand le brief est mûr), (2) **reprise** (revenir 3 jours plus tard, relire le fil, décider d'inviter ou pas), (3) **switch d'agent** (Claude bloque → invite Codex sur la même disc → il lit le contexte → continue), (4) **multi-agent collab** (le bullet ci-dessus). Form de création : checkbox "Lancer un agent tout de suite" (défaut coché, cas 80% inchangé) + tooltip ⓘ 23 mots validé user. Header disc montre les participants en chips (🤖 Claude, 💠 Codex, ✨ Gemini, 🐙 Kiro, 💻 Copilot, 🐱 Vibe, 🦙 Ollama, ⚙️ Custom, 👤 Unknown) + bouton **[+ Inviter]** toujours visible. Migration 060 backfille les discs existantes (`source_agent` set → 1 owner row).

- **Token invite multi-use within TTL** (`db::discussion_sessions::join_via_token` + `consume_invite_token` fix 2026-05-21). UX win majeur post-test live : le user devait cliquer [+ Inviter] N fois pour inviter N peers. Maintenant un seul token de 10 min suffit pour faire joindre Claude + Codex + Vibe + tous ceux qu'on veut. `used_at` + `used_by_session_id` deviennent un audit trail "first user" plutôt qu'un lock single-use. Join idempotent sur `(agent_type, session_id)` — re-join ne crée pas de phantom row.

- **`docs/operations/mcp-servers/kronn-internal.md` créé** : fiche complète du MCP interne (protocole multi-agent + disc_append simple/bulk modes + agent_type auto-detection). Règle `docs/AGENTS.md` relâchée : lire SI fichier existe, sinon procéder normalement (avant : bloquant). Côté Codex la lecture stricte de la règle créait une friction systématique au premier `disc_join`.

- **`agentSupportsIntrospection(Codex) = true`** (`frontend/src/lib/constants.ts` + backend mirror `agent_speaks_mcp` in `disc_prompts.rs`). Codex 0.132 a fixé upstream le sandbox exec-mode qui cancellait les MCP tool calls en 0.121 — confirmé live par `tools/call disc_meta` smoke test. La TD `TD-20260510-codex-mcp-sandbox-block.md` est supprimée. Le warning popover yellow sur les discs Codex disparaît, et l'agent_speaks_mcp prompt notice est désormais injectée en Codex aussi.

- **Custom API plugin `endpoints` field — auto-discovery via le CustomApiAiHelper existant** (`backend/src/models/mcp.rs::CustomApiPayload.endpoints` + `materialize_custom_server` + 4 Rust tests, `frontend/src/components/CustomApiAiHelper.tsx` + `pages/McpPage.tsx` + 6 Vitest tests + 9 i18n keys × 3 langs). Le formulaire "Create Custom API" gagne une section **Endpoints** éditable (path / méthode / description / remove-row, default empty), et l'AI helper ChatBubble qui aide déjà à remplir name+base_url+description+docs_url+fields apprend à **aussi proposer des endpoints** via un nouveau champ `endpoints[]` dans son bloc `KRONN:APPLY`. Le system prompt instruit l'agent : *"si `docs_url` set → fetch via WebFetch → emit 5-15 endpoints les plus utiles (préfère GET sauf si write explicitement demandé) ; sinon demande à l'user de coller les paths importants"*. Côté backend, `CustomApiPayload` gagne `endpoints: Vec<ApiEndpoint>` (serde default = back-compat front-end pré-deploy), `materialize_custom_server` normalise (uppercase method, default GET sur blank, drop blank-path) et copie dans `ApiSpec.endpoints` (déjà existant côté model). Net effect : à la création de la Custom API plugin Didomi (ou n'importe quelle autre), l'agent fetch la doc une fois, propose la liste, l'user valide ligne-par-ligne, et `mcp_list` passe `NEEDS_RESEARCH → READY` immédiatement → le broker `api_call` peut désormais les appeler. Anti-hallucination : la liste reste éditable AVANT save (no auto-persist), les valeurs des `fields` (= credentials) restent user-supplied uniquement. Cf. [[project_endpoints_autodiscovery_0_8_6]] (reframed 2026-05-19, ~4-5h vs estimation initiale ~9-11h).

### Test counts

- Backend : **2275 lib tests** (+95 net depuis 0.8.5) — agent_api broker (+9), validate_required_fields_per_type (+13), materialize_custom_server endpoints + back-compat (+4), update_custom_spec stitching + endpoints round-trip (+2), oauth2_cache substitute_env + flatten_form + jsonpath + resolve_token_exchange wiremock (+22), api_call_executor substitute_env + TokenExchange E2E (+8), discussion_sessions CRUD + invite tokens + join_via_token (+18), api_invite peer_join + peer_leave + wait_for_peer + list_participants + 2 E2E 2-peer collab (+25). Quelques tests OAuth2 consolidés. ⚠ 2 tests `mcp_scanner_test::atomic_write_checked_*` flakes occasionnels sous parallel execution (race tempdir filesystem) — passent en isolation (6/6 quand isolés). Pre-existing flake, non-lié à 0.8.6, à fix avec tempdir-per-test isolation strict en 0.8.7.
- Frontend : **1411 vitest** (+18) — CustomApiAiHelper.endpoints (+6), McpPage unified-edit (+4), NewDiscussionForm disc-first (+7), DiscParticipantsHeader (+6), DiscussionsPage launchAgentNow=false branch (+1), constants Codex flip update.
- Python : 22 → **58 unittest** (+36 net) — `mcp_list.config_keys` + auth_managed inference + lowercase `${env.x}` substitution (+3), api_call broker forwarding + envelope unwrap (+8), runtime disc-binding (+6), disc_join (+4), disc_wait_for_peer (+5), disc_leave (+2), client-info auto-detect + PPID fallback (+8), disc_append simple-mode (+4), stable session_id across calls (+4).

### Deferred to 0.8.7+

- **`[[project_api_call_logs_0_8_6]]`** — unified `api_call_logs` table + UI for workflow + broker + test calls. Audit trail + cost tracking + redaction sibling pre-req with [[project_continual_learning_0_9_0]].
- **Broker safety completeness** — side_effect opt-in gate (refuse mutating endpoints by default), per-disc rate-limit, audit log table. Cf. [[project_agent_api_broker_0_8_6]] follow-ups.
- **`[[project_endpoints_autodiscovery_0_8_6]]` Part A** — UI banner sur les Custom plugins legacy (created before 0.8.6) que les endpoints sont vides + bouton "Demander à l'IA d'aider".
- **`[[project_gate_checkpoint_0_8_6]]`** — auto-commit avant Gate, auto-reset sur Goto, makes Gate→implement loops idempotent (~5h).
- **`[[project_gate_auto_approve_0_8_6]]`** — `gate_auto_approve_after_secs` opt-in countdown per Gate step (~3-4h).
- **`[[project_plugin_import_export_0_8_6]]`** Path A — clipboard-JSON export/import on Custom plugins for sharing (~1.5h).
- **`[[project_disc_files_panel_branch_diff]]`** — committed-on-branch view sur le panneau Fichiers (worktree-isolated discs).
- **`[[project_dropdown_native_options_theme]]`** — custom popover dropdown component pour theme parity Firefox/Safari sur natives `<select>`.
- **`[[project_custom_plugin_rename_orphan_env]]`** — partiellement shipped (orphan warning + filter on save), restant : auto-rekey UX prompt.
- **SSE `DiscMessageAppended` event** — actuellement polling 5s côté UI pour rafraîchir les messages d'une disc multi-agent. Upgrade vers vrai push event (réutilise `ws_broadcast` existant) en 0.8.7. La latence 5s est OK pour MVP.
- **Multi-agent : tour-de-jeu enforced (opt-in)** — paramètre disc `enforced_turn_order: bool` qui refuserait `disc_append` si pas le tour du caller. Surface live test 2026-05-21 où Claude et Codex se sont coordonnés par convention (lecture timestamps). Acceptable pour free-form chat, mais souhaitable pour jeux/workflows stricts. Reporté en 0.8.7 ou 0.9.0.
- **`[[project_multi_agent_orchestrator]]` — delegation pattern** : agent A appelle `agent_delegate({to: Codex, task: "review ce diff", context})` pour confier une sous-tâche à un autre agent spécialisé. Le substrat `discussion_sessions` shippé en 0.8.6 est prêt à recevoir ce tool. Effort ~15-25h, target 0.9.x.

## [0.8.5] - 2026-05-17

**Inter-step plumbing homogénéisée + wizard refactor + 5 fixes critiques découverts en dogfooding AutoPilot.**

Release "irréprochable sur les workflows" — chaque step type émet désormais EXACTEMENT le même envelope canonique (markers `---STEP_OUTPUT---` + `[SIGNAL: …]`), la stratégie inter-step ne dépend plus du type producteur. 4 bugs critiques de plumbing (manual trigger var injection silencieusement droppée, endpoint `{{var}}` non-interpolée, `WorkflowStep` ApiCall serde required-without-default, body 422 swallowé côté frontend) trouvés et corrigés via le dogfooding sur EW-7247 + Ticket Autopilot sur DOCROMS_WEB. Pulled forward de 0.9.0 parce que le risque "un workflow user qui casse silencieusement" était inacceptable.

### Added

- **Canonical Kronn step-output envelope** (`backend/src/workflows/step_output_format.rs` + 6 unit tests). Single source of truth for ALL envelope-producing step types: `[optional human prefix]\n---STEP_OUTPUT---\n{data, status, summary}\n---END_STEP_OUTPUT---\n[SIGNAL: <primary>]\n[SIGNAL: <optional secondary>]`. Wired into `api_call_executor` (was bare JSON + signal), `json_data_step` (was bare JSON, no signal), `notify_step` (was bare JSON, no signal), `batch_step::build_structured_output` + `batch_apicall_step` (was bare JSON, partial signals). `exec_step` already canonical — left alone. Gate + Agent FreeText stay envelope-less by design. Cf. [[project_step_output_homogenisation_0_9_0]].

- **Cross-step output transmission test matrix** (`backend/src/workflows/template.rs::cross_step_transmission`) — 17 dedicated tests pinning that EVERY step type produces / EVERY consumer can read the canonical envelope. Per-step-type tests (`json_data_exposes_data_summary_status_and_nested_fields`, `apicall_exposes_nested_path_into_real_jira_payload`, `exec_exposes_exit_code_and_stdout_excerpt`, `agent_structured_exposes_typed_manifest_fields`, `notify_exposes_http_metadata_to_downstream_steps`, `batch_exposes_counters_and_discussion_ids`, `gate_exposes_only_output_no_envelope`, `agent_freetext_exposes_only_output_no_data_envelope`) + 7 canonical source→consumer pairs (ApiCall→Agent, JsonData→Agent, Agent→Exec, Exec→Agent, ApiCall→Notify, Gate→following, Batch→Agent) + 1 catch-all `canonical_keys_present_for_every_envelope_producing_step_type` that iterates the full matrix to catch any single-step regression + 1 dedicated `legacy_bare_json_envelope_still_extracts_correctly` for back-compat with pre-0.8.5 run records in DB.

- **Wizard `WorkflowQuickStartPicker`** (`frontend/src/components/workflows/WorkflowQuickStartPicker.tsx` + `lib/workflow-quick-start.ts` adapters + 31 tests across `workflow-quick-start.test.ts` and `WorkflowQuickStartPicker.test.tsx`) — unified entry point at the top of wizard step 0. Replaces three previously separate UI sections (STARTER_TEMPLATES buttons at top, project suggestions toggle/panel at top, v0.7 preset bandeau buried in Advanced→Step 2). Searchable + sortable + filter chips (complexity × source); applicable-state greying with explanatory tooltip. Disabled until the workflow name is filled (gates avoid the "selected template then bounced back to step 0" UX surprise). Cf. [[project_linked_repos_picker_0_8_5]] for the next 0.8.5 picker work.

- **Manual trigger variable injection: full safety extraction** (`backend/src/api/workflows.rs::build_manual_trigger_obj` + 9 dedicated tests). Pre-fix `POST /api/workflows/:id/trigger` only forwarded variables that appeared in `wf.variables` (the declared list), silently dropping any auto-detected `{{var}}` the frontend launch modal had asked the user to fill — so workflows fired with literal `{{issue_key}}` strings in step prompts → URL-encoded `%7B%7Bissue_key%7D%7D` → 404 from Jira. Caught during EW-7247 AutoPilot dogfooding 2026-05-17. Now accepts EVERY provided variable, with a conservative safety filter (`is_safe_trigger_var_name` — ASCII word chars + dot, ≤ 64 chars). Reserved keys (`type`, `triggered_at`) cannot be spoofed by the payload — pinned by `build_manual_trigger_obj_reserved_keys_cannot_be_spoofed_by_user`. Critical regression coverage — pre-fix this path had ZERO test coverage.

- **Endpoint `{{var}}` interpolation in ApiCall steps** (`backend/src/workflows/api_call_executor.rs:131` + 4 tests in `endpoint_double_brace_var_*`). Pre-fix the endpoint only honoured single-brace `{key}` (resolved against `step.api_path_params`), masking and restoring any `{{...}}` runs verbatim. Users who wrote `/rest/api/3/issue/{{issue_key}}` directly (the natural shape the AI helper suggests) got a URL-encoded literal and a confusing Jira 404. Now `ctx.render()` runs FIRST so `{{issue_key}}` → `EW-7247`, then `resolve_path_params` does its percent-encoded `{key}` pass on the result. Mixed forms `/rest/api/{{base}}/issue/{issue_id}` work correctly.

- **MCP read tools — `workflow_list` / `qp_list` / `qa_list` / `mcp_list`** (`backend/scripts/disc-introspection-mcp.py` + workflow-architect + qp-improver skills) — agents can now LIST existing artifacts before creating duplicates. Compact JSON payload (no full bodies — the agent calls `GET /<surface>/<id>` for details when needed). Skills now teach "always list before you create" so the agent reuses existing QPs / QAs via `quick_prompt_id` / `quick_api_id` instead of inlining duplicate prompts. Live-tested: `workflow_list` returns the user's 10 workflows with `enabled` + `step_count` + `last_run_status`, `qp_list` surfaces variable names + skill bindings, `mcp_list` enumerates both configured plugin instances + REGISTRY servers with `api_spec` so the agent can pick `api_plugin_slug` deterministically.

- **MCP auto-inherits `project_id` + `source_agent` from current discussion** (`backend/scripts/disc-introspection-mcp.py::_current_disc_meta`) — pre-fix every agent-created disc / workflow / QP landed in "Général" because the agent didn't know to look up the parent disc's project, AND agent-created discs were visually indistinguishable from UI-created ones because `source_agent` (the 0.8.4 cross-agent memory field that drives the sidebar `📥 ClaudeCode` badge) stayed null. Single helper `_current_disc_meta()` resolves `{id, project_id, agent}` once per process from `GET /api/discussions/<KRONN_DISCUSSION_ID>/meta`. `disc_create` now defaults TWO fields when the agent omits them: `project_id` (parent project) + `source_agent` (parent agent → makes `SwipeableDiscItem.tsx:147` render the badge). `workflow_create_draft` + `qp_create_draft` only inherit `project_id` (no source-binding columns on those entities). Important non-default: we deliberately **do not** auto-fill `source_session_id` from the parent disc id because `api/disc_source.rs:78` treats `(source_agent, source_session_id)` as an idempotency key — auto-filling both would collapse all sibling MCP-created discs from the same parent to the first one created. Agents pass `source_session_id` explicitly when they actually want one-disc-per-external-session semantics. Caught 2026-05-18 when the user noticed "tu as créé une disc dans Général alors que je suis sur front_euronews" + "je ne peux pas distinguer une conv créée via UI vs MCP dans le sidebar". Both fixed by the same lookup.

- **MCP autonomous draft creation — `workflow_create_draft` + `qp_create_draft` tools** (`backend/scripts/disc-introspection-mcp.py` + `models/workflows.rs::CreateWorkflowRequest.enabled` + 3 tests) — symmetric path to the existing `KRONN:WORKFLOW_READY` / `KRONN:QP_IMPROVED` signal+button flow. The MCP tools let an agent CREATE the artifact directly when the conversation has converged on a clear design. Safety contract: `workflow_create_draft` ALWAYS forces `enabled: false` server-side regardless of agent payload — drafts can't auto-fire on cron before user review. QPs have no enabled flag (manual launch only). Both tools surface the created id back to the agent so it can tell the user where to find the draft. Use case the user asked for: accelerate Kronn workflow adoption (`Ca [aiderait] aussi à l'adoption des Workflow Kronn`). Tests : `create_workflow_with_enabled_false_persists_as_draft` (the safety contract), `create_workflow_without_enabled_field_defaults_to_true` (back-compat with every UI-driven save), `architect_skills_teach_mcp_draft_creation_tools` (skill guards pin both architect skills explain the new tools). Cf. [[project_mcp_draft_creation_0_8_5]].

- **`validate_required_fields_per_type` — safety net behind `#[serde(default)]`** (`backend/src/api/workflows.rs::validate_required_fields_per_type` + `validate_api_call_minimum` helper + 13 tests). The 0.8.5 serde-default change on `WorkflowStep.{agent, prompt_template, mode}` made axum accept previously-rejected minimal payloads, but it ALSO accepted payloads that should still be rejected: `step_type: Agent` with an empty `prompt_template`, `ApiCall` with no `api_endpoint_path`, `BatchQuickPrompt` missing `batch_items_from`, `Notify` with no `notify_config`. Pre-fix those would persist and only blow up at run-time with "step emitted empty response" or "API returned 404 on /". Now the validator runs at every save site (POST `/api/workflows`, PUT, bundle-import wf_from_db) and rejects the payload at the wizard layer with a step-named, field-named error. Rules: Agent needs `prompt_template` OR `quick_prompt_id` ref; ApiCall needs `api_endpoint_path` + (`api_plugin_slug` OR `quick_api_id`); BatchQuickPrompt needs `batch_quick_prompt_id` + `batch_items_from`; BatchApiCall = ApiCall + `batch_items_from`; Notify needs populated `notify_config.url`; Gate / Exec / JsonData deferred to their existing dedicated validators so we don't double-report. Short-circuits on first offender (wizard surfaces one error at a time). Tests cover every variant's missing-field path + QP/QA-ref escape hatches + the deferred-variants no-op + first-offender-wins ordering. Closes the last "release-blocker" risk I'd flagged for 0.8.5.

- **Python tests for MCP auto-inherit helpers** (`backend/scripts/test_disc_introspection_mcp.py` + `make test-python` + `test-python` job in `.github/workflows/ci-test.yml`). The 0.8.5 `_current_disc_meta` / `_current_project_id` / `call_disc_create` helpers had zero unit-test coverage — only the user's live-by-hand smoke test the day they shipped. Now 10 stdlib-only `unittest` cases pin: cache hit/miss behaviour, `KRONN_DISCUSSION_ID` missing → returns `None` silently, backend unreachable → returns `None` + stderr log (does NOT crash the MCP server), `_current_project_id` derives from the shared cache (no separate HTTP), `call_disc_create` auto-fills `project_id`+`source_agent` from parent, explicit user values override the auto-fill, no parent meta → no inheritance (pre-0.8.5 fallback). The SAFETY-CRITICAL pin: `test_does_not_auto_fill_source_session_id` guards the idempotency-collision fix — if someone reverts this in 6 months thinking they're "improving" the cross-agent memory binding, the test will catch it. Sub-second run on stdlib only (no extra dev deps). CI job is its own lane so it doesn't gate behind the heavy Rust toolchain setup.

- **Sidebar + ChatHeader expose the discussion id** (`ChatHeader.tsx::disc-id-pill` + `SwipeableDiscItem.tsx::title` attr + `DiscussionSidebar.tsx::matchesFilters` extended with id prefix match + 4 i18n keys × 3 langs). Pre-fix the disc id was never rendered anywhere in the UI — when an agent (e.g. via `kronn-internal` MCP) referenced `04a9c927` in a summary, the user had no way to find that disc back. Now the ChatHeader shows a `#04a9c927` mono pill (click → copy full UUID to clipboard, hover → tooltip with the UUID), the sidebar title tooltip shows the UUID on hover, and the sidebar search input ALSO matches id prefix so pasting `04a9` filters to that disc. Round-trip "agent quotes id → user paste → land on disc" works in 3 keystrokes.

### Changed

- **`workflow-architect` skill — canonical envelope + full signal coverage docs** (`backend/src/skills/workflow-architect.md` + new test guard `workflow_architect_skill_teaches_canonical_envelope_and_signal_coverage`). Three sections rewritten: template-variables list now says `.data`/`.summary`/`.status` works for EVERY envelope-producing step type (was "only Structured Agent or ApiCall"); new "Canonical Kronn step-output envelope (0.8.5+)" subsection with byte-for-byte format + per-step-type matrix; Signals table now enumerates `Notify` (OK/ERROR), `JsonData` (OK), `BatchQuickPrompt` (OK/PARTIAL/ERROR/PENDING) as signal-emitting step types (pre-0.8.5 incorrectly said "branching not supported"). Without this update AI-generated workflows would keep emitting the pre-0.8.5 dialects and slowly drift back to two-strategy territory.

- **Preset `ticket-to-pr.createPrPrompt` × 3 langs** (`frontend/src/lib/i18n.ts`) — bad guidance `Output \`state.pr_url=<url>\`` replaced with the canonical `---STATE:pr_url=<url>---` marker syntax + explicit warning that the marker form is mandatory. Pre-fix the `notify_done` step's `{{state.pr_url}}` reference would resolve to literal because the agent followed the prompt's wrong syntax and Kronn's runner never extracted the state.

- **`WorkflowStep.{agent, prompt_template, mode}` now `#[serde(default)]`** (`backend/src/models/workflows.rs` + `models/setup.rs`). Pre-fix an ApiCall step's payload was rejected by axum's `Json<WorkflowStep>` extractor with `missing field "prompt_template"` because the fields were required-without-default at the type level — but they're irrelevant for non-LLM step types. Now `AgentType: Default` (variant `ClaudeCode`) and `StepMode: Default` (variant `Normal`) carry the safe defaults. 3 dedicated regression tests (`workflow_step_apicall_deserialises_without_llm_fields`, `workflow_step_agent_roundtrips_with_explicit_fields`, `test_api_call_request_accepts_minimal_step`) pin the contract.

### Fixed

- **`Server error (HTTP 422)` swallowed the actual reason** (`frontend/src/lib/api.ts:312-326` + 4 tests). Pre-fix when axum's `Json<T>` extractor rejected a request (returning 422 with `Content-Type: text/plain` and the deserialization failure in the body), the frontend's `api()` helper saw the non-JSON content type and threw a bare `Server error (HTTP 422)` with zero actionable info. Now reads the body via `res.text()`, includes up to 500 chars in the error message (`Server error (HTTP 422) — Failed to deserialize the JSON body: missing field 'agent' at line 1 column 234`). Defensive fallbacks: empty body / `text()` rejection both produce the bare form without throwing. Caught the user during the JIRA helper agent dogfooding when the QP-improver wasted minutes diagnosing a phantom 422 with no body.

- **QP Improver banner — busy guard + toast + persistent "déployé" state** (`frontend/src/pages/DiscussionsPage.tsx` + `frontend/src/lib/qp-improver-banner.ts` + 9 dedicated tests). Three follow-ups after the 0.8.4 ship: (1) the deploy CTA was a silent `console.warn` on PUT failure → the user saw "click does nothing" when the agent emitted invalid JSON; now `toast(t('qp.deployFailed', userError(e)), 'error')`. (2) `useRef` busy guard against fast double-click (closure-stale `disabled={busy}`, cf. [[feedback_race_guards]]). (3) localStorage-backed "deployed at v\<N\>" marker keyed by discussion id — once a QP is deployed, returning to the disc renders a disabled "✅ QP déployé en v3" banner instead of the active CTA. After successful PUT, fetches `quickPromptsApi.history()` to capture the freshly-snapshotted version index, persists, then navigates with toast success.

- **AgentQuestionForm — false-positive `{{var}}:` in code / inline backticks** (`frontend/src/lib/agent-question-parse.ts` + 6 dedicated tests). Pre-fix the parser matched `{{var}}:` anywhere in the text, so an agent reply containing `--after="{{date}}T{{h1}}:00"` (recommendation prose) or a fenced ` ```json` block with `git log --after=\"{{date}}T{{h1}}:00\"` produced a garbage mini-form with `h1`/`h2` as variable names and `00\" --before=…` as the question body. Fix: (1) new `stripCodeRegions()` blanks fenced ` ```…``` ` and inline ` `…` ` regions in place (preserving newlines so line offsets stay stable). (2) Regex re-anchored to start-of-line with optional bullet (`-/*/+/•`) or ordered-list marker (`1.` / `2)`). Real-form questions stay parsed, code/prose noise is silently ignored.

- **Wizard launch modal stayed open for the entire run duration** (`frontend/src/pages/WorkflowsPage.tsx`). Pre-fix `await fireTrigger(...)` resolved only when the SSE stream completed — so the launch modal stayed open until the workflow finished (sometimes 30+ min). Now closes immediately after validation; the `liveRun` pane takes over rendering.

- **CI `pnpm install` ETIMEDOUT on `onnxruntime-node` postinstall** (`frontend/package.json` → `pnpm.neverBuiltDependencies`). The transitive dep tries to download native Microsoft Azure binaries at install time, which times out on GitHub Actions runners. The Whisper STT worker uses `onnxruntime-web` (WASM) in the browser anyway → the Node binaries are never loaded at runtime. `neverBuiltDependencies: ["onnxruntime-node"]` skips the postinstall safely. Lockfile unchanged.

- **Residual `ai/` references in i18n + 4 source comments → `docs/`** (`frontend/src/lib/i18n.ts` `mcp.contextInfo` × 3 langs + `backend/src/models/mcp.rs:270` + `backend/src/models/workflows.rs:378` + `frontend/src/components/workflows/ApiCallStepCard.tsx:50` + `frontend/src/lib/workflow-templates/chartbeat-top5.ts:5`). Final residues from the 0.7.1 pivot — the `mcp.contextInfo` string was visibly wrong in the MCP drawer (`McpPage.tsx`) showing `ai/operations/mcp-servers/{1}.md` while the backend writes via `detect_docs_dir` → `docs/operations/...` since 0.7.1.

- **QuickStart picker preset titles showed raw i18n keys** (`frontend/src/lib/workflow-quick-start.ts::fromPreset`) — the adapter set `title: p.id` and `description: p.descKey` so the picker rendered `auto-dev` / `wiz.preset.autoDev.desc` instead of the human strings. Caught by Playwright E2E `wizard-presets.spec.ts` on 2026-05-18. Fix: the builder now takes a `t: Translator` argument and resolves `\`${p.icon} ${t(p.titleKey)}\`` / `t(p.descKey)`. Emoji prefix preserved so `🎫 Ticket Autopilot` stays distinguishable from `🎯 Big-ticket AutoPilot`. Tests fixture updated to pass a `tStub` translator.

- **desktop-build CI couldn't find DMG/EXE/DEB artifacts** (`.github/workflows/desktop-build.yml`). Since `.cargo/config.toml` set `target-dir = "target"` at the repo root (2026-05-15, cf. [[feedback_rust_target_bloat]]) to mutualise tokio/serde/reqwest between `backend/` and `desktop/src-tauri/`, Tauri builds now land in `/target/<triple>/release/bundle/...` instead of `/desktop/src-tauri/target/<triple>/release/bundle/...`. The macOS upload-artifact step failed with `No files were found` because the path glob only listed the legacy location. Fix: every artifact upload (Windows / macOS / Linux) now globs BOTH the legacy `desktop/src-tauri/target/...` AND the shared `target/...` paths. The macOS ad-hoc sign step's bundle-dir lookup also checks all 4 candidates (2 roots × 2 triple prefixes).

- **Playwright wizard specs broken by the 0.8.5 QuickStart picker refactor** (`frontend/e2e/pages/WorkflowWizardPage.ts` + `e2e/specs/wizard-{presets,save-error,create-button-validation}.spec.ts`). The 0.8.4-era specs queried preset cards on advanced step 2 via `getByRole('button', { name: /🎫\s*Ticket Autopilot/i })`, but 0.8.5 unified all 3 preset sources (STARTER_TEMPLATES, suggestions, v07 presets) into a single picker on step 0 (Infos), rendering rows as `<li>` with the title in a `<span>`. The page object now exposes `quickStartToggle` + `quickStartRow(re)` + `quickStartApplyButton(re)` + `openQuickStartPicker(name)` / `applyQuickStart(name, titleRe)` helpers. The 3 affected specs were rewritten to use the new flow; backward-compat shims on `presetAutoDev` / `presetTicketToPr` / `presetFeasibilityAutopilot` / `presetDailyHostAudit` return the new row locators so any future spec doesn't need to relearn the picker structure.

### Test counts

- Backend : 2123 → **2180** lib (+57 net since 0.8.4). Net new : +6 helper tests, +17 cross-step transmission, +9 manual trigger var injection, +4 endpoint `{{var}}` interpolation, +3 WorkflowStep ApiCall serde, +1 workflow-architect canonical-envelope skill guard, +13 required-fields-per-StepType validator + +4 extras absorbed into other fixes.
- Frontend : 1333 → **1387 vitest** (+54). `qp-improver-banner.test.ts` (+9), `agent-question-parse.test.ts` (+6 cases for code-region exclusion), `workflow-quick-start.test.ts` (+17), `WorkflowQuickStartPicker.test.tsx` (+14), `api.test.ts` (+4 for body-surfacing), `WorkflowQuickStartPicker.test.tsx::disabled gate` (+4).
- Python : 0 → **10** unittest cases on `backend/scripts/disc-introspection-mcp.py` helpers (new `make test-python` + dedicated `test-python` CI job).
- Playwright E2E : unchanged (covered by CI on `ci-test` label).

### Deferred to 0.8.6 / 0.9.0

- `[[project_linked_repos_picker_0_8_5]]` — UX: auto-suggest linked_repos from scan_paths candidates instead of manual path input (a tied-back issue surfaced during the EW-7247 setup).
- `[[project_audit_state_backfill_0_8_5]]` — backfill `docs/.kronn.json` from legacy `checksums.json` / `KRONN:VALIDATED` markers so older audited projects appear as `Validated` without a re-audit.

## [0.8.4] - 2026-05-17

**Désagentify + push→pull migration + QP polish (AI Improver, version history & metrics, bindings).**

Release qui consolide deux chantiers : (1) la sortie de la dette technique post-0.8.3 — désagentification du briefing, push→pull des linked_repos, sub-audits étoffés, cross-agent memory MCP, recap panel d'audit ; (2) une couche complète "QP comme produit" — bouton ✨ AI Improver, bindings skills/profils/directives, historique de versions avec metrics par version (avg tokens / duration / cost / Δ% gated derrière un floor de 3 lancements), suppression de version archivée, garde required-vars sur Launch+Compare.

### Added

- **Désagentified briefing — form + 0 LLM call** (`api/audit/briefing.rs::save_briefing_form` + `frontend/components/BriefingForm.tsx` #285) — nouvelle voie pour le briefing pre-audit : formulaire HTML avec les 6 questions ; submit → backend formate + écrit `docs/briefing.md` byte-for-byte compatible avec le format conversationnel précédent. Token cost = 0, latence = 1 HTTP roundtrip. La voie conversationnelle reste disponible (bouton "Briefing IA") pour les users qui préfèrent la guidance LLM. UI ProjectCard affiche les 2 boutons côte-à-côte avec tooltips explicatifs. Endpoint `POST /api/projects/:id/save-briefing`. Route + i18n FR/EN/ES + CSS inline form. Cohérent avec le pattern Phase 3 TD bulk-first de 0.8.3 (désagentify les surfaces où une discussion LLM est overkill).

- **Sub-audits Database + ApiDesign prompts étoffés** (`api/audit/mod.rs::DATABASE_STEPS` + `API_DESIGN_STEPS` #287 partiel) — pre-0.8.4 ces 2 sub-audits étaient des placeholders 0.8.2 ("placeholder body, content lands in S2.D4-5"). Maintenant DATABASE_STEPS couvre : schema + migrations safety / indexes + perf / ORM + N+1 / data integrity ; API_DESIGN_STEPS couvre : contract consistency / versioning + evolution / pagination + list responses / authn + authz + rate limiting / doc drift. Tous deux suivent le même schema TD detail-file + anti-repetition + marker discipline que la Full audit Step 9. Le sub-audit UI selector + le kind `Rgaa` sont également shipped (cf. bullet suivant).

- **Post-audit step recap panel** (`db/sql/055_audit_run_steps.sql` + `db/audit_runs.rs::{insert_audit_step_start,finalize_audit_step,list_audit_steps}` + `api/audit/run.rs::{audit_latest,audit_run_steps}` + `frontend/components/AuditRecapPanel.tsx` #298) — table durée + tokens par étape sur ProjectCard, collapsed par défaut. Nouvelle table `audit_run_steps` peuplée at `step_start` (insert) + `step_done` (finalize) par le SSE pipeline ; idempotent sur `(audit_run_id, step_index)` (UNIQUE index) pour le cas resume #311 où une étape déjà complétée se re-fire avant le skip. Front : `<AuditRecapPanel>` mounted dans la section docAi, refetch automatique sur `auditCompletedTick` quand un audit se termine. Sortable par durée / tokens DESC pour identifier l'étape qui crame. Highlighting rouge sur cli_success=false OU step_warning (#292), avec icône 🔧 sur les steps repaired_from_template. Empty state pour les runs pré-0.8.4. 2 endpoints REST : `GET /api/projects/:id/audit-latest` + `GET /api/audit-runs/:run_id/steps`. Tests : 5 backend (`insert_step_start_then_finalize_round_trip`, `insert_step_start_is_idempotent_on_resume`, `finalize_step_with_warning_marks_failure_and_repaired`, `list_audit_steps_is_ordered_by_step_index`, `list_audit_steps_returns_empty_for_unknown_run`) + 6 frontend vitest (`AuditRecapPanel.test.tsx`).

- **Sub-audit UI selector + AuditKind::Rgaa** (`models/projects.rs::AuditKind` + `api/audit/mod.rs::RGAA_STEPS` + `frontend/components/SubAuditModal.tsx` #287) — `Rgaa` variant ajoutée à AuditKind, ainsi qu'un step set RGAA 4.1 dédié (`docs/inconsistencies-rgaa.md`) qui couvre les 5 thématiques principales (images, couleurs, scripts, éléments obligatoires, formulaires) + une section "Pour aller plus loin" littérale écrite à chaque audit, qui :  *(a)* rappelle que **l'audit automatique ne remplace PAS un audit manuel** (~30-40 % des critères couvert par tooling) — W3C + DINUM cités comme autorités ; *(b)* différencie **Access42** (audit RGAA officiel + cursus certifiant, jurisprudence) de **Opquast** (qualité Web globale + RGAA en sous-ensemble, certif à vie pour toute l'équipe) ; *(c)* injonction explicite "re-tester soi-même OU faire appel à un pro" pour éviter le "j'ai fait un audit tout va bien". Frontend : `<SubAuditModal>` ouvre un picker `Audit global / Audit ciblé` avec 7 tuiles (Security / Docker / Performance / Accessibility / Rgaa / Database / ApiDesign) + descriptions courtes. Bouton chevron ▾ à côté du bouton "Lancer l'audit IA" sur TemplateInstalled/Bootstrapped + bouton "Audit ciblé" à côté du badge "audit OK" sur Validated. `handleFullAudit(undefined, kind)` passe le kind via `LaunchAuditRequest.kind` ; sub-audits affichent une barre de progression 1/1. Tests : 1 backend (`rgaa_kind_carries_french_criteria_and_distinct_index`) + tests existants étendus pour Rgaa (label round-trip, dispatch, index file distinctness) + 7 frontend vitest (`SubAuditModal.test.tsx`).

- **Cross-agent memory MCP — routes HTTP + outils MCP + UI** (`db/disc_source.rs` + `api/disc_source.rs` + `backend/scripts/disc-introspection-mcp.py` + `frontend/components/SwipeableDiscItem.tsx` + `frontend/components/DiscussionSidebar.tsx` #294) — l'infra DB de la migration `054_cross_agent_memory.sql` (4 colonnes source_*, table `disc_source_history`, `messages.source_msg_id`) est maintenant exploitable end-to-end :
  - **9 endpoints REST** : `POST /api/disc/create` (idempotent sur `(source_agent, source_session_id)`), `POST /api/disc/append` (dedup via `source_msg_id`, retourne `{appended, skipped_as_duplicates, diverged}`), `POST /api/disc/link` (last-link-wins), `POST /api/disc/unlink`, `GET /api/disc/find_by_session`, `GET /api/disc/search` (LIKE escapé, hits avec snippet 80 chars), `GET /api/disc/load_other` (range clampé à `[0, total]`), `GET /api/disc/sources` (batch — tous les bindings courants), `GET /api/discussions/{id}/source` (binding actuel + history chain).
  - **7 outils MCP** ajoutés à `disc-introspection-mcp.py` (en plus des 3 existants `disc_meta`/`disc_get_message`/`disc_summarize`) : `disc_create`, `disc_append`, `disc_link`, `disc_unlink`, `disc_find_by_session`, `disc_search`, `disc_load_other`. Chaque outil = wrapper urllib autour de la route correspondante, valide les args avant l'appel HTTP.
  - **UI badge + filter** : `SwipeableDiscItem` affiche un badge "📥 ClaudeCode" (ou ⚠ rouge si diverged) à côté du titre quand le disc a une source binding. DiscussionsPage sidebar fetch `discSources()` une fois au mount et expose un dropdown "Toutes les sources / Depuis X" filtrant la liste. i18n FR/EN/ES.
  - **Tests** : 9 DB unit (`db::disc_source::tests`), 6 API integration (`api_tests::disc_*`), 5 vitest UI (`DiscussionSidebar.sourceBadge.test.tsx`). Le bridge MCP est validé via `python3 -c ast.parse` (smoke) + couvert par les routes Rust qu'il appelle.

- **QP AI Improver** (`backend/src/skills/qp-improver.md` + `frontend/src/pages/WorkflowsPage.tsx::handleImproveQP` + `frontend/src/pages/DiscussionsPage.tsx` deploy banner) — bouton ✨ "Améliorer ce QP avec l'IA" sur chaque carte Quick Prompt. Click → spawn une discussion seeded avec le body canonique du QP (id + name + template + variables + bindings + agent + tier + description) dans un bloc ```json + le skill `qp-improver` épinglé. L'agent audite (table audit + recommandations + QP refactoré) et émet `KRONN:QP_IMPROVED`. La bannière dans DiscussionsPage parse le titre `[Improve QP <id>]` (source de vérité, NOT le `id` côté agent — anti-hallucination) + extrait le premier bloc ```json post-signal → CTA "Déployer le QP amélioré" PUT `/api/quick-prompts/:id` en un clic. Le skill suit le pattern de `workflow-architect` (sortie strictement structurée, signal load-bearing) avec 8 dimensions d'audit (role, intent, constraints, variables, output format, examples, bindings, anti-patterns). Tests : 1 backend (`qp_improver_skill_teaches_strict_output_protocol`) + 10 frontend (`qp-improver-signal.test.ts`) + 1 E2E (`qp-085-features.spec.ts`).

- **QP + QA profile/directive binding** (migration `056_qp_qa_profile_directive_binding.sql` + `models/quick.rs` + `db/quick_prompts.rs` + `db/quick_apis.rs` + `frontend/components/workflows/QuickPromptForm.tsx`) — Quick Prompts et Quick APIs gagnent les colonnes `profile_ids_json` + `directive_ids_json`, symétriques avec les bindings discussion/workflow déjà existants. Le QP form expose un nouveau bloc "Liaisons" en accordéon (skills · profils · directives) qui mirror le pattern de `NewDiscussionForm`. Le merge respecte la même règle que `skill_ids` : binding step-level explicite > binding QP-level > rien. Au lancement d'un QP, les bindings flow dans la discussion fille (`db/workflows.rs::create_batch_run`). QA carry-through silencieux : la forme ne montre pas le picker (un QA est un appel HTTP pur), mais les bindings round-trip via import/bundle pour usage en aval (chained QP, compare-agents). Tests : DB roundtrip étendu (`quick_prompt_crud`) + hydrate logic (`step_profile_and_directive_ids_inherited_from_qp_when_empty`, `..._win_when_explicit`) + 5 frontend (`QuickPromptForm.bindings.test.tsx`).

- **QP version history + per-version launch metrics + version delete** (migrations `057_message_duration.sql` + `058_qp_versions_and_lineage.sql` + `059_qp_versions_backfill.sql` + `db/quick_prompts.rs::{snapshot_quick_prompt_version,list_quick_prompt_version_metrics,delete_quick_prompt_version,current_version_index}` + `api/quick_prompts.rs::{history,metrics,delete_version}` + `frontend/components/QPHistoryDrawer.tsx` + `QPCardMetricsChip.tsx`) — système d'historique end-to-end qui rend la pertinence d'un QP **mesurable** au lieu de subjective. Trois briques :
  1. **Track real wall-clock duration** par message d'agent. Pre-0.8.4 la durée affichée venait du diff `prev_user_ts → msg.timestamp`, gonflée par le temps de frappe utilisateur — inutile pour de l'agrégation. Le streaming layer capture maintenant `Instant::now()` au début de `make_agent_stream` et écrit le delta réel en `messages.duration_ms`. NULL sur les rows User/System/legacy/imported.
  2. **Snapshot append-only à chaque mutation du QP**. Table `quick_prompt_versions` (id, quick_prompt_id, version_index, …) avec UNIQUE(qp_id, version_index). `insert_quick_prompt` seed v1 ; chaque `update_quick_prompt` écrit vN+1 BEFORE l'UPDATE (panic-safe : un crash entre snapshot et UPDATE perd la mutation, pas le snapshot). Migration 059 backfill v1 pour tous les QPs legacy au moment où elle tourne (idempotent via NOT EXISTS). `discussions.originating_qp_id` + `originating_qp_version` stampés au lancement du QP — le metrics aggregator GROUPe par cette paire pour calculer avg tokens / duration / cost du **premier message agent** uniquement (les tours suivants reflètent la réaction utilisateur, pas la pertinence du QP).
  3. **UI mirror `AuditRecapPanel`** — bouton `🕒 N versions` sur chaque carte QP → drawer latéral, accordéon par version (v_n marquée "actuelle", strip accent à gauche), méta-chips `🚀 launches · 💬 avg tokens · ⏱ avg duration · 💰 avg cost`, **Δ% vs version précédente** (vert si baisse de tokens / durée, orange sinon) **gated derrière un floor de 3 lancements par version** (sous ce seuil le Δ est masqué — un seul run rapide ne doit pas faire passer v3 pour +60% meilleure que v2). Expansion d'une version révèle un **diff side-by-side** ligne à ligne du `prompt_template` (helper pur `diffLines()` avec classification same / changed / added / removed — pas de dépendance externe). Chip compact sur la carte (`QPCardMetricsChip`) affiche `🚀 N · 💬 ~X tk · ⏱ ~Ys` de la version courante quand au moins 1 lancement existe. Bouton 🗑 sur chaque version archivée (jamais sur la courante — backend la refuserait) → confirm + cascade `originating_qp_id/version = NULL` sur les discs qui référençaient la version supprimée (les discs restent, la lineage drop).
  CSS 100% `--kr-*` tokens (theme-aware dark / light / sakura / matrix / batman). Tests : 7 backend (`quick_prompt_insert_seeds_version_v1`, `quick_prompt_update_snapshots_v2_v3`, `quick_prompt_metrics_aggregates_first_agent_reply_per_version`, `quick_prompt_metrics_empty_for_qp_without_launches`, `quick_prompt_metrics_ignores_non_first_agent_replies`, `quick_prompt_delete_version_refuses_current_and_succeeds_on_older`, `quick_prompt_delete_version_clears_discussion_lineage`) + 17 frontend (`QPHistoryDrawer.test.tsx` — diffLines 7 cases + drawer UX 10 cases) + 1 PW E2E (`qp-history-drawer.spec.ts` — open drawer, Δ% renders for launches≥3, expand version reveals diff toggle, Escape closes).

- **Seed-message UX collapse + post-deploy QP focus** (`MessageBubble.tsx::splitMessageSeed` + `KronnSeedToggle` + `WorkflowsPage.tsx::handleImproveQP` + `DiscussionsPage.tsx` post-deploy nav) — deux follow-ups UX après les premiers retours sur l'AI Improver. (1) Le seed technique posté en première User-message (QP JSON + catalogue + protocole d'audit) est désormais enveloppé dans des marqueurs HTML `<!--KRONN_SEED_START-->…<!--KRONN_SEED_END-->`. L'UI rend seulement le préfixe visible (`✨ Audit et amélioration du Quick Prompt « X » en cours…`) et expose un bouton `▸ Contexte technique envoyé à l'agent` qui dévoile le seed dans un `<pre>` scrollable au clic — l'agent continue de lire le message complet verbatim depuis la DB. (2) Le clic "Déployer le QP amélioré" pose `sessionStorage['kronn:postQpImproved']`, navigue vers `workflows`, switch sur l'onglet Quick Prompts, scroll-into-view sur la card cible (`data-qp-id={qp.id}`) + flash CSS 1.5s (border accent + glow, respecte `prefers-reduced-motion`). Tests : 8 frontend (`MessageBubble.seedToggle.test.tsx`).

- **Catalog injection + skill clarification dans le QP Improver** (`WorkflowsPage.tsx::handleImproveQP` + `backend/src/skills/qp-improver.md`) — fix de la première version qui laissait toujours les bindings vides dans le QP refactoré. Le seed inclut maintenant la liste complète des skills / profils / directives installés (`- \`id\` — description (120 char max)`, ~30 lignes), et le skill `qp-improver` dit explicitement "utilise le catalogue, des bindings vides = sous-utilisation". Hard rule revisée : **toujours préserver l'existant + proposer du nouveau quand pertinent** (ex: skill `security` sur un QP audit sécu, `concise` directive sur un QP triage). Skill guard test `qp_improver_skill_teaches_strict_output_protocol` étendu pour pinner cette règle.

- **Required-vars guard sur Launch + Compare-Agents** (`WorkflowsPage.tsx::collectMissingRequiredVars`) — pré-fix les boutons fire pouvaient fire le QP avec des `{{var}}` non substituées (template literal visible dans le prompt de l'agent). Guard côté handler : `handleLaunchQP` et `handleCompareAgents` listent les vars marquées `required` (≠ false) non remplies et toast un message localisé listant les labels manquants au lieu de fire. Variables `required: false` skippées, `required: undefined` = required (compat legacy). 8 tests unitaires (`WorkflowsPage.requiredVars.test.tsx`).

### Changed

- **linked_repos push → pull migration** (`api/projects/mod.rs::sync_linked_repos_doc*` + `format_linked_repos_for_docs` + `compute_companion_context*` #295) — pre-0.8.4 le block `## Linked repositories (companion repos)` était injecté dans le system prompt de CHAQUE message disc + CHAQUE step de workflow + CHAQUE step d'audit (4+ sites). 500-2000 tokens/message gaspillés sur des chats qui ne touchent pas aux companion repos. Fix : 2 fonctions `compute_companion_context` (disc/WF, sans linked_repos) et `compute_companion_context_for_audit` (audit, KEEP linked_repos car cross-repo findings = -39% tokens sur un big-ticket réel). Côté docs : auto-write `docs/linked-repos.md` sur (a) CRUD `PUT /linked-repos` (b) audit Phase 1. L'agent lit ce fichier on-demand via la mention dans `docs/AGENTS.md` § 5. Empty list = file supprimé (no stale doc). Tests : `format_linked_repos_for_docs_renders_pull_friendly_header`, `sync_linked_repos_doc_in_writes_then_removes`, `compute_companion_context_drops_linked_repos_for_disc_wf_pulls`, `compute_companion_context_for_audit_keeps_linked_repos_inline`.

- **Tests parallèles : sérialisation `KRONN_TEMPLATES_DIR`** (`api/audit/validation.rs` + `core/mcp_scanner_test.rs`) — 7 tests qui mutent l'env var partagée `KRONN_TEMPLATES_DIR` étaient flakies sous `cargo test --lib` (parallel par défaut). Tagged `#[serial(kronn_templates_env)]` via le crate `serial_test` (déjà en dev-dep). Les tests qui ne touchent pas l'env restent parallèles. 3 runs consécutifs verts (vs ~1/5 d'échec avant).

- **`audit/mod.rs` prompt warnings cleanup** — 4 warnings rustc "multiple lines skipped by escaped newline" sur Step 9 du PROMPT_PREAMBLE (séparateurs `\n\n\` suivis d'une ligne vide). Supprimé les lignes vides intermédiaires entre `\n\n\` et l'en-tête suivant : sémantique du prompt préservée byte-for-byte, warnings tombent à 0.

### Fixed

- **CI clippy** (`api/projects/mod.rs::compute_companion_context_for_audit` + `api/audit/mod.rs::HELPER_MCP_NAMES,is_helper_only_mcp_setup` + `api/audit/helpers.rs:99` doc) — les 3 fns/consts "kept for unit tests + future use" déclenchaient `-D dead-code` en CI ; `#[allow(dead_code)]` posé avec la rationale en doc. Le doc-lint `clippy::doc_lazy_continuation` sur `build_sub_audit_validation_prompt` venait du `+ Phase 4` parsé comme bullet — remplacé par `AND Phase 4` + ligne vide avant la liste. CI clippy `-D warnings` : 0 errors.

- **CI `pnpm install` ETIMEDOUT sur `onnxruntime-node` postinstall** (`frontend/package.json` → `pnpm.neverBuiltDependencies`) — la dep transitive `onnxruntime-node` (tirée par `@huggingface/transformers` pour le worker Whisper STT) tente de télécharger des binaires natifs Microsoft (`150.171.109.230:443`) à chaque `pnpm install`, ce qui timeoute sur runner GitHub Actions et bloque la CI. Le worker tournant exclusivement côté **browser** via `onnxruntime-web` (WASM), les binaires Node ne sont jamais utilisés à runtime → skip propre via `"pnpm": { "neverBuiltDependencies": ["onnxruntime-node"] }`. Aucun impact runtime, lockfile inchangé.

- **QP Improver — banner deploy CTA silencieux + persistant** (`frontend/src/pages/DiscussionsPage.tsx` + `lib/qp-improver-banner.ts` nouveau + i18n FR/EN/ES `qp.deployInProgress` / `qp.deployFailed` / `qp.deploySuccess` / `qp.deployedAtVersion`) — pre-fix le clic sur "Déployer le QP amélioré" swallowait silencieusement les erreurs PUT 400 (agent JSON malformé : champ `agent` non-enum, `tier` invalide, required manquant) via un `console.warn` sans toast → l'utilisateur voyait "rien". Ajouté : (a) `useRef` busy guard contre le double-clic ([[feedback_race_guards]]), (b) `toast(t('qp.deployFailed', userError(e)), 'error')` sur échec, (c) spinner `Loader2` + texte "Déploiement en cours…" pendant le PUT, (d) après PUT OK, fetch `quickPromptsApi.history()` → récupère le `version_index` du snapshot fraîchement écrit → stocke dans `localStorage` (`kronn:qpDisc:<discId>:deployedVersion`) → toast `qp.deploySuccess` avec la version, (e) au re-render de la disc, si marker présent → banner désactivé "✅ QP déployé en v\<N\>" au lieu du CTA actif (avant : le banner restait actif éternellement car dérivé du contenu du message qui contient toujours `KRONN:QP_IMPROVED` + le bloc JSON). Tests : 9 (`qp-improver-banner.test.ts` — round-trip localStorage + Safari private mode fallback + dedup par disc).

- **AgentQuestionForm — faux-positifs `{{var}}:` dans du code/inline** (`frontend/src/lib/agent-question-parse.ts`) — le parser des questions structurées `{{var}}: question` matchait n'importe où dans le texte (`/\{\{(\w+)\}\}:[ \t]*([^\n]+)/g`), donc une réponse d'agent contenant `--after="{{date}}T{{h1}}:00"` (recommandation du QP Improver, inline backticks) OU un bloc \`\`\`json avec `git log --after=\"{{date}}T{{h1}}:00\"` (le QP refactoré lui-même) produisait un faux mini-form `{ h1: "00\"...", h2: "00\"..." }` au-dessus du ChatInput. Fix : (a) nouvelle `stripCodeRegions()` qui blank les fences \`\`\`…\`\`\` et inline \`…\` en préservant les newlines (offsets de ligne stables), (b) regex ré-ancrée `/^[ \t]*(?:(?:[-*+•]|\d+[.)])[ \t]+)?\{\{(\w+)\}\}:[ \t]*([^\n]+)/gm` (début de ligne obligatoire, bullet markdown `-/*/+/•` ou ordered-list `1.` / `2)` optionnel). Les vraies questions restent reconnues, le bruit code disparaît. Tests : +6 cas (mid-sentence rejected, inline code rejected, fenced code rejected, bullet/ordered list still match, repro exacte du bug remonté en dogfooding 0.8.4).

- **i18n + commentaires `ai/` → `docs/`** (`frontend/src/lib/i18n.ts` `mcp.contextInfo` × 3 langs + `backend/src/models/mcp.rs:270` + `backend/src/models/workflows.rs:378` + `frontend/src/components/workflows/ApiCallStepCard.tsx:50` + `frontend/src/lib/workflow-templates/chartbeat-top5.ts:5`) — derniers résidus pré-pivot 0.7.1 : la chaîne i18n `mcp.contextInfo` affichait `ai/operations/mcp-servers/{1}.md` dans le drawer MCP de `McpPage` alors que le backend écrit via `detect_docs_dir` → `docs/operations/...` depuis 0.7.1. 4 commentaires de doc référençaient encore `ai/operations/deagent-apicall.md`. Tous fixés. Le code Rust de scan/detect (rétro-compat layout legacy) intact.

### Test counts

- Backend : 2043 → **2123** lib (+80) — linked_repos push→pull (+5) + audit_run_steps recap (+5) + RGAA kind (+1) + cross-agent memory DB helpers (+9) + ts-rs / shape pinning (+14) + qp_improver skill guard (+1) + QP/QA bindings hydrate logic (+2) + QP versions/metrics/delete aggregator (+7) + ~36 autres tests dérivés des chantiers ci-dessus.
- Backend integration : **172** (unchanged from 0.8.3 — surface non touchée par 0.8.4).
- Frontend : 1260 → **1348** vitest (+88) — `AuditRecapPanel.test.tsx` (+6), `SubAuditModal.test.tsx` (+7), `DiscussionSidebar.sourceBadge.test.tsx` (+5), `BriefingForm.test.tsx` (+5), `QuickPromptForm.bindings.test.tsx` (+5), `qp-improver-signal.test.ts` (+10), `MessageBubble.seedToggle.test.tsx` (+8), `QPHistoryDrawer.test.tsx` (+17), `WorkflowsPage.requiredVars.test.tsx` (+8), `qp-improver-banner.test.ts` (+9, dogfooding follow-up), `agent-question-parse.test.ts` (+6, code-region exclusion fix) + petits ajouts (CTA / signals).
- Playwright E2E : +2 specs (`qp-085-features.spec.ts` — 2 tests : improve button POST + bindings accordion ; `qp-history-drawer.spec.ts` — 1 test : drawer open + Δ% + diff toggle + Escape). All pass against live backend on dev DB.

### Verified ALREADY shipped

Pendant le sweep, deux items prévus en quick-win pour cette release ont été trouvés DÉJÀ implémentés en branche `feat/multi-audit-states-and-internal-mcp` :

- **QP Chain Phase 3 (DnD reorder)** — `WorkflowWizard.tsx:1764-1850` implémente native HTML5 DnD + ↑/↓ arrow buttons + remove.
- **QP Chain Phase 4 (`{{previous_qp.output}}` vars)** — `api/discussions/runtime.rs::render_chain_qp_prompt` + 6 unit tests + hint FR/EN/ES `wiz.batchChainHint`.

La mémoire associée a été mise à jour pour refléter le shipping.

## [0.8.3] - 2026-05-14

**Feasibility-Gated AutoPilot + cross-repo evidence + email pipelines — le pattern killer pour les gros tickets, validé end-to-end.**

Release centrée sur la capacité de Kronn à orchestrer un agent contraint
sur un gros ticket sans perdre le contrôle, et à brancher l'envoi
transactionnel/lifecycle email en aval avec ~0 token sur la pelle.
Mesuré sur un **big-ticket réel** (migration multi-brand cross-repo de
phase 0, ~100k tokens en autopilot flat) : **-39 % de tokens**
(104,9k → 63,9k) et **-40 % de durée** vs un autopilot flat — en bonus
la détection d'une discrepancy ticket↔prod (champ de config) que l'agent
a remontée avec le fichier:ligne legacy en référence.

### Added

- **Feasibility-Gated workflow template** (`workflows/big_ticket_template.rs`) — 7 steps en primitives mixtes : `fetch_issue` (JsonData) → `triage` (Agent + TypedSchema(Fail)) → `review_triage` (Gate) → `implement` (Agent) → `run_tests` (Exec) → `drift_check` (Exec) → `pr_draft` (Agent). Token cost = 0 sur les 4 steps mécaniques. Preset frontend `feasibility-autopilot` + CTA "AutoPilot" sur les discussions d'audit validé.
- **Triage manifest schema** strict (`workflows/triage.rs`) : `clear[]` / `decided[]` / `mocked[]` / `blocked[]` + `files_touched[]`. Le runner détecte un step triage (description `[TRIAGE]` ou shape du schema) et injecte un addendum "audit, don't code" + une section CROSS-REPO EVIDENCE qui exige le format `evidence: <repo>/<path>:<line>` pour chaque `decided` / `mocked` quand un linked_repo peut servir de source.
- **`StepOutputFormat::TypedSchema { on_invalid }`** — accepte `Continue` (legacy) ou `Fail` (court-circuit hard si le manifest est invalide après repair). `Fail` empêche un manifest cassé d'arriver à `implement`.
- **KRONN-(ASSUMED|MOCKED|TODO) markers** — insérés par l'implement step à chaque décision tracée du manifest, grep-és par `drift_check` (zéro token). L'audit IA pickup les `KRONN-TODO` comme tech debt avec provenance ticket (table `agent_decisions`).
- **Bundle creator** `POST /api/workflows/bundle` (`api/bundle.rs`) — création atomique workflow + QuickPrompts + QuickAPIs + CustomAPIs en une transaction unique via sentinel `@bundle:<id>`. Rollback complet si la moindre insertion échoue ; substitution points : `quick_prompt_id`, `batch_quick_prompt_id`, `quick_api_id`, `api_config_id`. Frontend : signal `KRONN:BUNDLE_READY` rend un CTA "Create everything (1 workflow + N supporting artifacts)".
- **Linked repos / companion projects** — `models/projects.rs::LinkedRepo` + `PUT /api/projects/:id/linked-repos`. L'utilisateur déclare manuellement les dépôts compagnons d'un projet (kinds : `api` / `iac` / `design` / `shared-lib` / `docs` / `other`). UI dans ProjectCard entre Skills et AI Context ; cap à 20 entries pour borner les prompts.
- **Cross-repo evidence injection (audit-pipeline symmetry)** — helper `compute_companion_context(state, project_id)` qui consolide les blocs `## Linked repositories` + `## Other Kronn projects on this machine`. Injecté sur **toutes** les surfaces agent : audit (`api/audit/{full,run,drift}.rs`), workflow runner (`workflows/runner.rs`), test-step preview (`api/workflows.rs`), discussions chat (`api/discussions/streaming.rs`), orchestration debate + synthesis (`api/discussions/orchestration.rs`). Les 3 sites de summarization interne (`orchestration.rs` lignes 286 / 689 / 864) restent volontairement vides — companion repos = noise dans une compression de conversation. L'implement step a une règle 6 : si une entrée du manifest cite `evidence: <linked_repo>/<path>:<line>`, **lift** la valeur concrète au lieu d'en inventer.
- **Structured Gate panel pour manifests triage** (`components/workflows/RunDetail.tsx::TriageManifestPanel`) — détecte un manifest dans la `gate_message`, le parse, et remplace le dump JSON brut par 4 sections collapsibles (clear / decided / mocked / blocked) avec cards par entrée + footer `files_touched`. `tryParseTriageManifest` exporté avec un brace-counter robuste aux strings et aux échappements. Fallback transparent vers le texte brut pour les Gates non-triage. i18n FR/EN/ES.
- **Skill `workflow-architect`** — sections "Feasibility-Gated pattern", "Cross-repo evidence", "Bundle protocol" (`KRONN:BUNDLE_READY`) ; `api_plugin_slug` désormais REQUIRED quand pertinent (endpoint→slug map Jira/GitHub/Adobe/Resend/Mailjet) ; post-emission disclaimer "⚠ Template — review before triggering" ; compte officiellement "eight step types".
- **`AgentDecision` table** (`db/sql/051_agent_decisions.sql` + `models/agent_decisions.rs`) — chaque entrée triage `decided`/`mocked`/`blocked` ingérée auto avec UNIQUE(run_id, decision_id). Read via `GET /api/agent-decisions?run_id=…` ou `?project_id=…`.

- **Resend (hybride MCP + API) + Mailjet (API native)** — `mcp-resend`
  passe d'une entrée MCP-only à une entrée hybride (Stdio MCP + REST
  API spec), même convention que `mcp-github` et `mcp-atlassian` :
  une seule fiche dans le drawer, une seule credential
  (`RESEND_API_KEY`), deux surfaces — MCP pour les Quick Prompts
  riches, ApiCall pour les workflows déterministes à 0 token sur
  l'envoi. Nouvelle entrée `api-mailjet` (Basic / `MAILJET_API_KEY` +
  `MAILJET_API_SECRET`) — pas d'MCP officiel Mailjet, donc API-only —
  qui couvre le segment EU/RGPD (médias, banque, secteur public) pour
  qui Resend n'est pas envisageable. Les deux plugins embarquent un
  `default_context` agent dense (~200 lignes chacun) couvrant les
  pièges réels : pour Resend → domaine vérifié obligatoire (422
  `from address is not valid` = #1 piège), `Idempotency-Key` pour
  CSM replay-safety, contraintes du batch (≤100, body en array, pas
  d'attachments, pas de `scheduled_at`), tags ASCII-only droppés en
  silence ; pour Mailjet → sender validé obligatoire (`Sender not
  allowed` = #1 400), envelope `{Messages:[…]}` v3.1 contre legacy v3
  flat, **toujours boucler sur `Messages[].Status`** car HTTP-200
  cache des partial failures, `SandboxMode: true` pour valider sans
  envoyer (parfait dans un Gate de preview), `managecontact` comme
  primitive de segmentation CSM (`at-risk` / `churned` / `power-user`).
  Côté frontend, `apiCallPluginTips.ts` ajoute les tips FR pour
  `mcp-resend` et `api-mailjet`, injectés dans le prompt de l'AI
  Helper du WorkflowWizard pour éviter les hallucinations sur la
  forme des appels. Cas d'usage débloqué : pipelines CSM /
  lifecycle email (synthèse usage → Gate humain → envoi à ~0 token
  l'envoi unitaire).

- **Audit progress instrumentation** (`api/audit/full.rs` + `components/ProjectCard.tsx`) — la barre de progression de l'audit IA affichait juste `Step N/M — file.md` sans aucune info de coût ni de durée, rendant impossible de répondre à "quel step optimiser ?". Le SSE émet maintenant un événement `start` enrichi avec `total_steps` + `started_at` (ISO-8601 wallclock anchor sans drift client) et chaque `step_done` carry `tokens` (max(input+output) pour le step — agents reportent un cumul par appel, on prend donc le max et NON une somme), `duration_ms` (wallclock du step), `total_tokens` (running sum). Frontend : nouveaux chips `💬 4,521 tk` (dernier step) + `Σ 23,890 tk` (cumul) à côté du `⏱ 2m 13s` existant ; reset propre à chaque nouveau run pour éviter le flash de valeurs stale. Backwards-compat : les handlers nouveaux sont optionnels, les agents qui ne parlent pas stream-json (Vibe direct, Ollama) gardent `tokens=0` ce qui cache simplement les chips au lieu d'afficher des zéros trompeurs. **Fix wallclock drift** : le `started_at` envoyé au frontend dans `start` event était re-déclaré juste avant la boucle audit (Phase 2), shadowant la valeur posée en début de handler (Phase 1 incluse). Conséquence : le compteur live affichait `Date.now()` au moment du clic, puis sautait en arrière de ~26 s (durée Phase 1 = template install + legacy migration + bootstrap inject) quand le SSE landed. Fix : supprimer le shadow, réutiliser le `audit_started_at` de ligne 119. `chrono::DateTime<Utc>` est `Copy`, le `move` dans la closure DB ne consomme pas l'original.

- **Unaudited-project warning banner** (`DiscussionsPage.tsx` #276) — UX gap d'adoption corrigé : un nouvel utilisateur Kronn qui lance une discussion sur un projet fraîchement enregistré n'a aucun signal qu'il existe un audit IA à faire d'abord. Il brûle des tokens à ré-expliquer son projet à chaque tour. Le banner persistant en haut de toute discussion sur un projet en état `NoTemplate` / `TemplateInstalled` / `Bootstrapped` surface l'audit manquant avec un CTA adaptatif : si `briefing_notes` est vide → CTA principal `📝 Faire le briefing du projet` (le briefing donne le contexte business à l'agent et multiplie la qualité de l'audit) ; si `briefing_notes` est rempli → CTA `▶ Lancer l'audit IA` (friction zéro, navigation directe vers le ProjectCard sur la section Audit). Le banner s'efface dès que `audit_status === Audited` ou `Validated`. Discussions système (briefing / bootstrap / validation) sont exclues pour ne pas empiler avec leurs propres CTAs dédiés ; discussions sans `project_id` également (rien à auditer). i18n FR/EN/ES.

### Changed

- **Workflow runner** charge le projet UNE fois en début de run et passe `extra_context` aux Agent steps via le nouveau paramètre `execute_step::extra_context: &str` — symétrique avec le pipeline d'audit. Les steps non-Agent (JsonData, Exec, Gate, ApiCall, Notify) ne reçoivent rien.
- **Prompt assembly factoré** dans `workflows/steps.rs::build_step_prompt` (pure fn) — render + extra_context append + output-format addendum + triage addendum. Extrait d'`execute_step` pour le rendre unit-testable indépendamment du spawn de l'agent.
- **`kronn-internal` MCP** wired sur 5 agent configs (Claude Code, Cursor, Codex, Kiro, Vibe), exposé dans le ProjectCard. Codex reste exclu du notice agent (sandbox exec-mode bloque l'appel, TD-20260510-codex-mcp-sandbox-block).

### Fixed

- **Pre-audit legacy docs migration** (`core/legacy_docs.rs`) — **bug critique d'adoption corrigé** : avant 0.8.3, un audit IA lancé sur un projet ayant déjà un `docs/` hand-curé (installations, ADRs, onboarding, etc.) installait les templates Kronn à côté **sans jamais lire le contenu existant** → l'agent remplissait les templates depuis le README + le code seulement, perdant des mois de connaissance humaine. Pire : si l'utilisateur avait un `docs/architecture/overview.md` perso, `copy_dir_nondestructive` le préservait silencieusement et l'audit créait un fichier Frankenstein partiellement réécrit. Fix : `migrate_user_docs_to_legacy` détecte un `docs/` non-Kronn (signature absente dans `docs/AGENTS.md` : `# AI agent context — Entry point`) et déplace TOUT le contenu existant sous `docs/legacy/` AVANT l'install des templates. Le `PROMPT_PREAMBLE` de l'audit est étendu d'une section **"Legacy docs (PRIMARY SOURCE)"** qui ordonne à l'agent de lire `docs/legacy/**/*.md` AVANT de remplir les templates Kronn, et de citer les références inline (`see docs/legacy/installation.md`) pour que l'utilisateur puisse vérifier le mapping puis supprimer `docs/legacy/` quand il a validé. Idempotent : un re-audit sur un projet déjà Kronn-managé → no-op (la signature dans AGENTS.md court-circuite). Data-safety prioritaire : symlinks jamais déréférencés (cible hors `docs/` intacte), collisions dans `legacy/` suffixées sans clobber (`installation.md-1`, `-2`...), dossiers `protected` (`var/`, `legacy/`) laissés en place, dotfiles + unicode/emoji + sous-arborescences profondes préservés byte-identical. **Navigation surfacée** : un `docs/legacy/README.md` est auto-écrit après la migration (ancre de navigation pour les futurs agents/utilisateurs ouvrant le projet semaines plus tard) et l'addendum du prompt audit oblige l'agent à ajouter UNE ligne pointant vers `docs/legacy/` dans le `docs/AGENTS.md` rempli — sans ça le dossier serait invisible. Hand-edits préservés : un `legacy/README.md` modifié manuellement par l'utilisateur post-audit n'est jamais clobberré par une migration ultérieure. SSE event `legacy_docs_migrated` + frontend handler optionnel `onLegacyDocsMigrated` pour rendre un toast + liste des fichiers déplacés (cap à 50 noms mais `moved_count` exact).
- `compute_companion_context` factorise les helpers `format_linked_repos_for_prompt` + `format_kronn_projects_universe_for_prompt` désormais réutilisés depuis 5 sites au lieu d'être dupliqués (était : 3 sites audit + 1 workflow ; devient : 1 helper, 5+ callers).
- **CI clippy pass (post-merge `commit` branch)** — `execute_step` (8 args) annoté `#[allow(clippy::too_many_arguments)]` avec rationale en doc ; `repair_valid` match-like-matches simplifié en `matches!()` ; `ModelTier.clone()` dans `api/bundle.rs:164` remplacé par copy (le type est `Copy`) ; doc lists dans `big_ticket_template.rs` re-indentées pour la nouvelle règle `doc_lazy_continuation` ; `tests/api_tests.rs` 2 fixtures `Project` mises à jour avec le champ `linked_repos: vec![]` ajouté en 0.8.3.
- **CI E2E preset collision** — `wizard-presets.spec.ts` + `wizard-create-button-validation.spec.ts` + `wizard-save-error.spec.ts` échouaient en strict-mode parce que `getByRole('button', { name: /Ticket Autopilot/i })` matchait à la fois `🎫 Ticket Autopilot` (ticket-to-pr) ET `🎯 Big-ticket AutoPilot` (feasibility-autopilot, ajouté 0.8.3). Fix : `WorkflowWizardPage.ts::presetTicketToPr` ancre désormais sur l'emoji 🎫 unique (contrat stable, frozen dans `workflow-templates/v07-presets.ts:489`) ; `presetFeasibilityAutopilot` ajouté en miroir pour les futurs E2E sur le big-ticket flow.

- **Audit resume after page refresh** (`ProjectCard.tsx` #280-fix) — bug visuel : un user qui cliquait "Lancer l'audit" puis rafraîchissait la page voyait à nouveau le bouton "Lancer l'audit" alors que l'audit tournait toujours côté backend. Root cause : l'effet resume au mount du `ProjectCard` ne lançait le poll backend QUE si un checkpoint localStorage existait. Or, n'importe quel scénario qui wipe le localStorage entre le clic et le refresh (dev-mode HMR, navigation cross-domain, browser qui nettoie le storage) laissait l'audit invisible côté frontend. Fix : poll inconditionnel au mount — le backend est désormais la source de vérité, le localStorage devient une optim UX (seed instantané sans attendre le round-trip réseau). Cleanup gates : pas de `onRefetch` spam sur les cards idle (qui spamerait la liste projects à chaque mount + 2s sur 50+ projets).

### Fixed

### Removed

- **`templates/docs/templates/exchanges.md`** (AI Exchange Template) — obsolète depuis l'arrivée des discussions Kronn (et a fortiori avec [[project_cross_agent_memory_0_8_4]]) qui font tout ce que ce template essayait de faire à la main. Dossier vide supprimé aussi. 3 références résiduelles purgées (`backend/src/core/user_context.rs` prelude + test + `templates/docs/AGENTS.md` off-limits list).

### Changed

- **CTA "Voir tous les Tech Debts" jumpait sur le projet mais pas sur le dossier tech-debt** (`components/MessageBubble.tsx` + `components/ProjectCard.tsx` #314) — pre-fix le bouton de la conversation validation faisait `window.location.hash = #project-<id>` + `onNavigate('projects')` → Dashboard expand la card → user atterrissait sur le tab AI Context par défaut, devait expand manuellement la section docs/tech-debt. **2 clics au lieu de 1**. Fix : MessageBubble pose `sessionStorage[kronn:postValidation:<projectId>] = "docs/tech-debt"` ; ProjectCard, dans un useEffect au mount (gated sur `isOpen`), lit + clear le flag + déclenche `setExpandedTab('docAi') + setDocDeepLink('docs/tech-debt')`. Un seul clic dans la conv → atterrissage direct sur les TDs. Test mis à jour pour valider l'écriture sessionStorage.

- **MCP context7 "tools not exposed in 3 consecutive audits"** (`backend/entrypoint.sh` #313) — root cause : pendant l'audit Step 8 (MCP introspection), Kronn lance 4 serveurs MCP npx-launched en parallèle (context7, sequential-thinking, memory + parfois GitHub). npm `_npx` cache race sur `rename node_modules/ajv → .ajv-<hash>` → `ENOTEMPTY` → installs half-baked → tous les npx subséquents fail à démarrer. L'agent voit "no tools" et insère `<!-- TODO: ask user -->` dans `docs/operations/mcp-servers/context7.md`. Reproduit en direct : `rm -rf ~/.npm/_npx` puis retry → context7 boote en 2s + expose ses tools. Fix : entrypoint container nettoie `_npx` au startup. Tradeoff : un cold-start de 5-10s par MCP au premier audit après restart container, mais 0 race condition récurrente.

- **Audit resume mechanism + placeholder leakage detection + gated validation disc** (`api/audit/full.rs` + `validation.rs::count_raw_placeholders` + `db/audit_runs.rs` + `models/projects.rs` + `components/ProjectCard.tsx` #310-312) — **bug critique** : quand claude rate-limit / crash / OOM en plein milieu de l'audit (DOCROMS_WEB step 5/10), trois choses cassaient en même temps :

  **(F8a #310) Placeholder leakage non détecté** : `validate_and_repair_step_output` comparait la taille au template (≥25%). Mais le fichier IDENTIQUE au template (Phase 1 a copié le template, claude a crashé avant de toucher → file === template) passait la validation. Step considéré success → audit continuait → marquait Audited → créait discussion validation, alors qu'il n'avait rien produit. Fix : nouvelle fonction `count_raw_placeholders` scan `{{UPPERCASE_SNAKE}}` tokens (n'inclut PAS la syntaxe Twig `{{ asset(…) }}` — lowercase + parens). Si placeholders restent après step, step failed quel que soit la taille. `repaired: false` car le fichier EST le template — re-run est le seul chemin.

  **(F8b #311) Audit resume mechanism** : `audit_runs.last_completed_step INTEGER` (migration 053) tracké à chaque step_done success via `update_last_completed_step`. `LaunchAuditRequest.resume_from: Option<u32>` permet de relancer en sautant les steps déjà faits. Endpoint `GET /api/projects/:id/audit-resumable` expose la dernière run `status='Interrupted' AND last_completed_step in 1..=9`. UI ProjectCard fetch ça au mount + change le bouton "Lancer l'audit AI" en "Reprendre à l'étape N/10" + passe `resume_from` au stream. Les steps avant le resume yield `step_skipped` côté SSE pour que la barre de progression reflète l'historique.

  **(F8c #312) Validation disc gated sur full success** : ne crée plus la discussion validation que si `last_successful_step == total_steps && !any_step_warning`. Sinon émet `audit_interrupted` event SSE + `mark_interrupted` côté DB (au lieu de `complete` Audited). Frontend décide via `resumableAudit` priority sur `validationInProgress`. Résultat : un audit cassé au step 5 ne ment plus "Validation en cours" — il dit clairement "Reprendre".

  10 nouveaux tests : `placeholder_leakage_is_detected_even_when_size_matches_template`, `count_raw_placeholders_recognizes_uppercase_snake_tokens`, `update_last_completed_step_bumps_only_forward_on_running_rows`, `mark_interrupted_writes_status_and_preserves_last_completed_step`, `latest_resumable_only_returns_interrupted_partial_runs` + apiMock + Dashboard mock updates.

- **Zombie audit detection** (`api/audit/full.rs::full_audit_handler` SSE loop #309) — **bug critique** : quand un child `claude` exit cleanly mais qu'un descendant npx-launched (sequential-thinking, memory, context7) garde le stdout pipe ouvert, le `process.next_line().await` bloquait indéfiniment. L'audit restait "auditing step N/10" pour toujours, brûlant 100k+ tokens sur un run mort. Le user devait killer le container pour s'en sortir. Fix : `tokio::select!` avec un idle timer 60s ; tous les 60s sans nouvelle ligne, on probe `process.child.try_wait()` ; si le child est mort, on break le SSE loop normalement (yield step_done) au lieu d'attendre l'EOF du pipe qui ne viendra jamais. Détecté + corrigé en live sur l'audit DOCROMS_WEB du 2026-05-15 (figé sur Step 10 ~10 min sans aucun token increment).

- **Audit quality overhaul** (`api/audit/mod.rs` PROMPT_PREAMBLE + Step 8/9/10 + `helpers.rs` Phase 2 + `templates/docs/decisions.md` + `api/projects/template.rs` subfolder READMEs #302-306) — 5 fix structurels après analyse approfondie d'un audit DOCROMS_WEB :

  **F1 — `decisions.md` jamais rempli** : était noyé dans Step 9 § E (200 lignes de prompt tech-debt), `validate_and_repair_step_output` ne pouvait pas l'attraper car `target_file` = tech-debt. Step 10 (REVIEW) devient maintenant `target_file: "docs/decisions.md"` avec un prompt 2-phases (1. final review cleanup, 2. fill decisions.md from observations). Le validate guard catche un decisions.md vide → toast warning immédiat. Step 9 a une note "decisions.md is intentionally filled in Step 10". Template enrichi avec exemples concrets.

  **F2 — Marker discipline dans PROMPT_PREAMBLE** : pre-fix, 26 `<!-- TODO: verify -->` sur testing-quality.md alors que l'agent avait *réellement* vérifié l'absence des configs. Nouvelle section MARKER DISCIPLINE qui distingue les 3 types : (a) `TODO: verify` = pas pu vérifier (sandbox / hors repo), JAMAIS après un Glob réussi ; (b) `TODO: ask user` = décision humaine ; (c) `TODO: unknown` = unknown préservé d'une pass précédente. Exemple WRONG/RIGHT inline.

  **F4 — Phase 2 validation scan TOUS les markers** : pré-fix, seul `TODO: unknown` était traité. Les 26 `TODO: verify` de DOCROMS_WEB restaient dans la doc pour l'éternité. Maintenant Phase 2 (FR/EN/ES) instruit un `grep -rn 'TODO: '` systématique + traitement des 3 types : verify → retry Glob puis escalader, ask user → question directe, unknown → re-ask. Marker supprimé une fois résolu.

  **F3 — context7 MCP "did not expose tools"** : Step 8 ne distinguait pas "server pas configuré" de "server qui cold-start lentement" (npx download + boot). Note "Cold-start: retry once after 5-10s before concluding no tools" ajoutée.

  **F5 — `conventions/` `gotchas/` `people/` README explicit empty-by-design** : users ouvraient ces dossiers post-audit, voyaient un README de 281 B, pensaient que l'audit avait raté. README enrichis avec `> Empty by design after the initial audit. This folder fills up over time...` en HEAD.

  **F6 — Mermaid sequenceDiagram safety rules** : user a hit un parse error sur `docs/architecture/sequences/page-request.md` ligne `FP-->>U: 103 Early Hints (Link: …; rel=preload)` — combo `…` Unicode + `:` + `;` + parens dans la message string confuse le lexer Mermaid 11.x. Step 6 prompt durci avec 4 règles explicites : (a) ASCII-only dans message text (pas de `…`/`→`/em-dash), (b) éviter `:` et `;` dans la string après la flèche, (c) pas de chains `(`/`)`/`[`/`]`/`{`/`}` inline → redirect vers `Note over`, (d) cap 100 char/ligne. Test `step6_prompt_enforces_mermaid_safety_rules` pin la régression.

  6 nouveaux tests verrouillent la régression : `step10_target_is_decisions_md_for_validate_and_repair_guard`, `step9_does_not_duplicate_decisions_md_instruction`, `preamble_documents_marker_discipline_three_types`, `phase2_scans_all_three_marker_types_and_drives_to_resolution`, `ensure_subfolders_readme_explicitly_says_empty_by_design`.

- **Tri AiDocViewer : dossiers d'abord A-Z, puis fichiers A-Z** (`api/ai_docs.rs::build_ai_file_tree` #301) — convention file-explorer attendue (Finder, VS Code, IntelliJ). Avant : tri alphabétique plat (`architecture/`, `briefing.md`, `coding-rules.md`, `operations/`) → ergonomie cassée. Maintenant : 2-tier sort `(is_file, name_lowercase)` — dirs groupés en haut, files en bas, tri case-insensitive dans chaque groupe. 3 nouveaux tests (top-level dirs-first, récursion sub-dirs, case-insensitive `Banana`/`apple`).

- **Banner "audit en cours" remplace le CTA "Lance un audit" pendant l'audit** (`components/ProjectCard.tsx` #300) — confusion UX : pendant l'audit, le banner AI Context affichait toujours "Lance un audit IA pour..." alors qu'il tournait + placeholders visibles dans les fichiers. Nouvelle branche prioritaire when `auditActive` avec Loader2 + texte "Audit en cours — la documentation se construit progressivement... les placeholders restants seront remplacés avant la fin". i18n FR/EN/ES.

- **Spinner sur le titre du ProjectCard quand audit/validation tourne** (`components/ProjectCard.tsx` + `pages/Dashboard.css` #299) — avant : spinner uniquement sur le badge `AI audit x/10` (visible mais pas évident sur une liste de 10+ projets) et sur la vue dépliée "AI Context". Maintenant : petit Loader2 (12px, couleur accent) inline avec le nom du projet, déclenché par `auditActive || validationInProgress`. Visible d'un coup d'œil même card collapsée — l'user voit lesquels projets moulinent sans avoir à déplier. `aria-label` localisé FR/EN/ES.

- **Chips audit (tokens / tool en cours) qui disparaissaient à partir du step 2** (`models/projects.rs` + `lib.rs` AuditTracker + `api/audit/full.rs`+`run.rs`+`drift.rs` + `components/ProjectCard.tsx` #297) — symptôme : élapsed continuait à ticker mais les 3 chips disparaissaient au passage en step 2 (testé sur DOCROMS_WEB). Cause probable : SSE buffer (nginx) ou agent en mode thinking-only qui ne flush pas d'Usage events pendant un long moment → pas de `step_progress` reçu côté frontend pendant des minutes. Fix push→pull : `AuditProgress` expose maintenant `step_tokens`, `total_tokens_so_far`, `current_tool` (Option<…>, backwards-compat) ; le `AuditTracker.update_chips()` est appelé à chaque Usage/ToolStart côté backend ; le poll `/api/audit-status` (déjà existant) re-seed les chips frontend à intervalle régulier. `clear_step_chips` au début de chaque step pour éviter le stale tool name. Résout aussi le scénario page-refresh (re-mount perdait les chips). Aucune régression : SSE continue de pusher les chips en temps réel comme avant, le poll est juste un fallback robuste.

- **Mermaid "Syntax error in text" parasite pendant le streaming** (`components/MermaidDiagram.tsx` #296) — pendant qu'un agent stream du markdown contenant un bloc `` ```mermaid `` non terminé, notre `<MermaidDiagram>` tentait `mermaid.render()` sur le source partiel. Or **Mermaid 11.x ne throw plus systématiquement** sur syntaxe invalide — il retourne un **SVG d'erreur** (`aria-roledescription="error"` + "Syntax error in text · mermaid version 11.15.0") que `innerHTML = svg` injectait verbatim. L'user voyait l'erreur native Mermaid dans les bulles en cours d'écriture. Triple fix : (a) **streaming guard** — skip render si le source ne commence PAS par un mot-clé Mermaid racine (allowlist 23 keywords : `flowchart` / `graph` / `sequenceDiagram` / `classDiagram` / `stateDiagram-v2` / `erDiagram` / `gantt` / `pie` / `gitGraph` / `C4Context` / `mindmap` / `timeline` / etc.) — pendant streaming, le bloc partiel n'a pas encore le keyword → silence. (b) **error-SVG detection** — après `mermaid.render`, on inspecte le SVG retourné pour `aria-roledescription="error"` ou `Syntax error in (text|graph)` ; si match, route vers notre fallback `setError(…)` au lieu de l'injection brute. (c) garde le fallback existant pour le throw-path. 3 nouveaux tests : error-SVG → fallback, source non-Mermaid → skip render, allowlist 13 keywords accept render.

- **Validation TD : bulk-first au lieu de 1-par-1 + CTA "Voir les TDs"** (`api/audit/helpers.rs` Phase 3 + `components/MessageBubble.tsx` + `components/ProjectCard.tsx` #293) — l'agent présentait les TDs un par un en Phase 3 ; sur 20-30 TDs c'était une heure de discussion → l'user abandonnait avant les Critical. Nouveau protocole Phase 3 (FR/EN/ES) : (1) lecture de tous les `docs/tech-debt/TD-*.md` en une passe, (2) table markdown compacte `| ID | Severity | Area | Title | Status | Effort |` en un seul message, (3) **une seule question** avec 3 options bulk : (a) tout valider → `Confirmed by user` partout, (b) tout rejeter → `Rejected` + retire du index (anti-repetition les saute au prochain audit), (c) détailler certains IDs uniquement → les autres `Confirmed by user` par défaut. Tickets MCP : batch question pour les High/Critical, pas par TD. Côté UI : **bouton "Voir les N TDs" sur le ProjectCard** à côté de la pastille "Audit validé" (visible quand `tech_debt_count > 0`) + **CTA dans le message `KRONN:VALIDATION_COMPLETE`** qui jump direct via `window.location.hash = '#project-<id>'` (réutilise le deep-link Dashboard). 1 test backend (anti-régression du protocole bulk-first FR/EN/ES) + 5 tests frontend (CTA visible avec marker + projectId, hidden si orphan, hidden sans marker, click → hash+navigate, marker strippé de la rendition).

- **Détection ROOT du step qui produit un fichier vide** (`api/audit/validation.rs` + `api/audit/full.rs` SSE `step_warning`) — fix de la **cause racine** du bug `inconsistencies-tech-debt.md` à 0 octet. Avant : on faisait confiance au code de retour 0 du CLI (Claude Code / Cursor / …) ; si l'agent crashait mid-Write ou écrivait `""` dans un fallback parse-error, le `step_done.success` était `true` et l'audit continuait silencieusement → l'user ne s'apercevait du trou qu'à la validation (ou jamais). Maintenant : check post-step (`validate_and_repair_step_output`) qui compare la taille du `target_file` au template source (≥ 25 % requis) ; si suspicious → (a) log `tracing::warn!`, (b) émet `step_warning` SSE avec `reason` + `repaired_from_template`, (c) auto-repair depuis le template pour que l'audit termine sur un baseline propre, (d) reporte `success: false` dans `step_done`. Côté frontend, `onStepWarning` handler dans `ProjectCard.tsx` surface un toast erreur localisé (FR/EN/ES) immédiat. L'user voit la défaillance LIVE au lieu d'un tick vert mensonger. 9 nouveaux tests : REVIEW pseudo-step / empty path / non-docs path / cli-already-failed / healthy dest / empty dest repair / truncated dest repair / threshold-edge / missing-template-no-repair.

- **Bug DOCROMS_WEB : `inconsistencies-tech-debt.md` vide après un re-audit** (`api/projects/template.rs::copy_dir_nondestructive` + nouveau `is_corrupted_template_file`) — root cause : un audit précédent avait planté en Step 9 (timeout / CLI crash) et laissé le fichier à 0 octet. Le re-audit voyait le fichier "existant" → `copy_dir_nondestructive` skip → Step 9 demandait à Claude de remplir un fichier vide sans template à hériter → 0 TD produit. Fix : heuristique de réparation — si la source template est ≥ 200 B ET la destination < 25 % de la source, on ré-écrase depuis le template. Conservative pour ne JAMAIS toucher au contenu user légitime (un user qui a supprimé 70 % d'un template reste au-dessus du seuil). 7 nouveaux tests : missing→create, healthy→skip, empty→repair, truncated→repair, small-template→skip-heuristic, just-above-threshold→preserve, nested→recurse.

- **Bug Mermaid plein écran qui se ferme tout seul toutes les 3 secondes** (`components/MermaidDiagram.tsx` + `AiDocViewer.tsx`) — root cause : le polling Dashboard `auditStatusAll` à 3s re-renderait l'arbre, et le `components` prop de ReactMarkdown était défini **inline** dans `DocMarkdown` → nouvelle référence à chaque render → ReactMarkdown unmount+remount chaque enfant → `<MermaidDiagram>` est unmounted+remounted → state `fullscreen: useState(false)` reset à false → l'overlay disparaît "tout seul". Triple fix : (a) `components` map hoistée au niveau module dans `AiDocViewer.tsx` (référence stable), (b) `MermaidDiagram` enveloppé dans `React.memo` avec compare sur `source` (re-render skippé sur toute autre prop), (c) overlay rendu via `createPortal(…, document.body)` (survit même si un parent dans le subtree remount).

- **Mermaid rendu visuel dans AiDocViewer + chat** (`components/MermaidDiagram.tsx` + `MermaidDiagram.css` #289) — les fichiers `docs/architecture/overview.md` (flowchart) et `docs/architecture/sequences/*.md` (sequenceDiagram) émis par l'audit Step 6 affichent maintenant un **vrai diagramme** au lieu du source markdown brut. Composant agnostique du type (flowchart / sequenceDiagram / classDiagram / stateDiagram / erDiagram / C4Context… tout ce que Mermaid 11.x supporte). Lazy-load du package `mermaid` (~600 kB) via dynamic import — pas dans le bundle initial. Theme `neutral` + `securityLevel: 'strict'` (les bindings JS `click ... "javascript:…"` sont désactivés). Boutons **Plein écran** (overlay modal, fermeture Escape/click-outside/X) et **Imprimer** (popup window dédié, SVG inliné, `window.print()` auto-déclenché → bypass des 100+ nœuds DOM de la page principale). Bouton "Voir le code source" pour débugger les diagrammes générés par l'IA. Fallback explicite en cas d'erreur de parsing : notice + détails + source brut visible. Wiré dans `AiDocViewer.tsx` (pre→MermaidDiagram quand `language-mermaid`) ET dans `MessageBubble.tsx` (même override pour les blocs Mermaid émis en chat). i18n FR/EN/ES. 3 tests : SVG valide, erreur de parsing avec fallback, toggle source.

- **Active-audits popover sur la nav Projets** (`components/ActiveAuditsPopover.tsx` + `api::audit::audit_status_all` #288) — symétrique de `ActiveRunsPopover` côté Workflows : quand au moins un audit tourne, le bouton "Projets" affiche un badge orange avec le count + un loader spinner, click intercepte la nav et déroule un popover listant chaque audit (nom du projet · étape N/M · fichier · ⏱ elapsed live · bouton Stop). Click sur une ligne navigue vers le ProjectCard correspondant. Footer "Voir tous les projets". Nouveau endpoint backend `GET /api/audit-status` (sans project_id) qui retourne `Vec<AuditProgress>` depuis `state.audit_tracker.progress`. Polling intelligent côté Dashboard : 3s si au moins un audit tourne, 10s si page='projects' sans audit, 60s sinon. i18n FR/EN/ES.

- **Mermaid diagrams dans l'audit IA** (`api/audit/mod.rs` step 6 + templates `architecture/overview.md` + `architecture/sequences/` #286) — Step 6 (architecture) demandait jusqu'ici un "ASCII flow diagram". Remplacé par : (a) **Mermaid `flowchart TD/LR` obligatoire** dans `docs/architecture/overview.md` rendant les services + main data flow + systèmes externes (option C4-style via `subgraph Person/System/Container/Component` si projet multi-tier), (b) **jusqu'à 3 sequence diagrams Mermaid** dans `docs/architecture/sequences/<flow-name>.md` pour les flows critiques détectés (auth, request lifecycle, deploy pipeline, etc.). Le hard cap à 3 évite l'explosion tokens sur projets complexes. Tout reste en syntaxe Mermaid universelle (rendu natif GitHub/GitLab/Obsidian/VS Code, pas de PlantUML/Structurizr requis). Les fichiers `sequences/<flow>.md` restent Tier 3 dans `docs/AGENTS.md` — agents les chargent à la demande quand ils travaillent sur le flow correspondant, zéro coût per-turn. Templates ship avec `sequences/README.md` (conventions) + `sequences/TEMPLATE.md` (skeleton).

- **Live in-step UX during audits** (`api/audit/full.rs` + `ProjectCard.tsx` #281) — l'audit affichait un loader sans signal pendant 30-120s par step. Backend émet maintenant deux nouveaux SSE events typés en plus du `chunk` raw : `step_progress` (carry `step_tokens` + `total_tokens_so_far` à chaque `Usage` event du stream-json claude → tokens chip ticke en LIVE pendant le step au lieu d'attendre `step_done`) et `tool_call` (carry le nom de l'outil que l'agent vient d'invoquer : `Read`, `Glob`, `mcp__Sequential Thinking__...`). Frontend : nouveau chip `🔧 <tool>` à côté de `⏱`/`💬`/`Σ` qui se met à jour à chaque tool call et se vide à `step_done`. Backwards-compat : les handlers `onStepProgress` / `onToolCall` sont optionnels, anciens callers continuent de marcher.

- **MCP allowlist audit-mode** (`core/audit_mcp_filter.rs` #280) — perf optimization majeure : sur projets avec 10+ MCP servers wired (Fastly, Docker, GitLab, M365, Playwright…), le system prompt de l'agent claude balloonait à 12-15K tokens de tool descriptions AVANT que l'agent commence à réfléchir. Un audit IA local n'a besoin que de quelques MCPs (introspection, raisonnement, lookup) — le reste = ballast. `AuditMcpSwap` RAII guard installe un `.mcp.json` filtré contenant uniquement l'allowlist (`kronn-internal`, `Sequential Thinking`, `Memory`, `context7`, `Git`) pendant la durée de l'audit ; restaure l'original sur Drop (incluant panic). Override utilisateur via `KRONN_AUDIT_MCP_EXTRA=Fastly,GitLab`. Discussion qui spawn pendant l'audit : skip du `sync_project_mcps_to_disk` pour préserver le filtre + banner "Audit IA en cours sur ce projet — certains MCPs temporairement désactivés" (poll auditStatus toutes les 8s, auto-hide à la fin). Impact mesuré : step 1 d'un audit DOCROMS_WEB (15 MCPs config) devrait passer de ~7-10 min à ~2-3 min. SSE event `audit_mcp_filtered` carry `{kept, dropped, kept_count, dropped_count}` pour le rendu UI. i18n FR/EN/ES.

- **Injection Kronn dans les fichiers agent root user-curés** (`core/root_agent_files.rs` #278) — bug critique d'invisibilité corrigé. Avant 0.8.3, la boucle Phase 1 de l'audit copiait `CLAUDE.md` / `.cursorrules` / `.windsurfrules` / `.clinerules` UNIQUEMENT quand le fichier n'existait pas (`if src.exists() && !dst.exists()`). Un utilisateur ayant ses propres règles dans `CLAUDE.md` voyait le template Kronn skippé **silencieusement** : ses règles workflow étaient préservées (bien) mais Kronn devenait **invisible** pour l'agent (mauvais) — Claude Code lisait `CLAUDE.md`, n'y trouvait aucune mention de `docs/AGENTS.md`, et ignorait toute la structure docs/ que Kronn venait de mettre en place. Fix : nouveau helper `inject_or_update` qui **injecte un bloc managed en tête** (`<!-- KRONN-MANAGED-BLOCK:START/END -->`) au-dessus du contenu user existant, avec pointer explicite vers `docs/AGENTS.md`. Trois cas couverts : (a) fichier absent → create avec bloc + template Kronn ; (b) fichier user sans markers → **prepend** du bloc, user content préservé byte-identical en dessous ; (c) fichier avec markers déjà présents → re-render UNIQUEMENT entre les markers (idempotent sur re-audit — pas de duplication même après 3 audits successifs). Writes atomiques via tmp + rename (un crash mid-write ne tronque pas le fichier user). Data-safety : user content jamais perdu (verified per-byte), unicode/emoji préservés, malformed markers gracefully handled.

- **Compteur de messages non lus inflationnel** (`Dashboard.tsx` + `DiscussionSidebar.tsx` #277) — UX bug récurrent : des utilisateurs accumulaient des centaines de messages "non lus" fantômes (cas observé : 559 messages signalés non lus alors que toutes les discussions étaient ouvertes). Deux causes additives :
  - **Bug de seed** dans `markDiscussionSeen` : il marquait `activeDiscussion.messages.length` comme nombre de messages vus, mais l'endpoint de liste retourne par design `messages: []` (seul `discussions.get` peuple le tableau). Sur la première frame où une discussion s'ouvre (avant que `get()` résolve), on marquait donc "0 vu" et la disc gardait son `message_count` complet en non-lu. Fix : `Math.max(messages.length, message_count ?? 0)` garantit qu'on ne sous-compte jamais.
  - **Legacy non-seeded** : `lastSeenMsgCount` n'est peuplé que sur l'ouverture explicite d'une disc, donc les discussions archivées et les batch children jamais consultés gardent leur `message_count` entier en non-lu, accumulé sur des mois. Fix UX : nouveau bouton `<CheckCheck />` "Tout marquer comme lu" dans le header de la sidebar, conditionnel (visible uniquement si `totalUnseenAll > 0` ET handler wired), avec tooltip qui affiche le count total qu'il va clear. `markAllDiscussionsSeen` dans Dashboard bulk-seed `lastSeenMsgCount[d.id] = Math.max(messages.length, message_count)` pour TOUTES les discs (archives + batchs inclus). Defensive : ne baisse jamais un seed existant (snapshot lag-tolerant). i18n FR/EN/ES.

### Tests

- Backend : 1952 → **2012** (+60) — +2 tests `audit::mod` : `step6_architecture_step_requires_mermaid_diagrams` (verrouille prompt content) et `architecture_template_carries_mermaid_placeholder_and_sequences_pointer` (verrouille template + sequences/README + sequences/TEMPLATE). — `triage_addendum_mandates_cross_repo_evidence`, `implement_step_teaches_linked_repos_evidence_lift`, `build_step_prompt_*` (3), `compute_companion_context_*` (4), 5 source-grep regression guards (workflow runner, test-step endpoint, discussions/streaming, orchestration debate+synthesis, orchestration summarization stays empty), guard `workflow_architect_skill_teaches_feasibility_gated_pattern` étendu cross-repo, 3 tests `core/registry.rs` Resend hybride + Mailjet shape, **20 tests `core/legacy_docs.rs`** (10 fonctionnels + 7 data-safety + 3 navigation : README créé / skip pas créé / hand-edit jamais clobberré). Couverture data-safety : symlinks unix, dotfiles, deep subtree, unicode/emoji, collision suffix, AGENTS.md user-curé, garde "hors docs/". **12 tests `core/root_agent_files.rs`** (create missing avec/sans template, prepend sans markers, re-render idempotent, 2e run = no-op via mtime check, unicode/emoji, markers en fin de fichier, malformed markers, empty file, atomic write cleanup, 3 audits successifs sans duplication, slice files locked). **14 tests `core/audit_mcp_filter.rs`** (allowlist content lock, case-insensitive matching, env override avec whitespace, empty env passthrough, payload sans mcpServers gracefully ignored, malformed JSON Err, swap install + drop restore, nothing-to-filter no-op, missing/malformed `.mcp.json` no-op, idempotent restore, panic survival via RAII Drop).
- Backend : 2012 → **2043** (+31) — +5 tests audit resume (placeholder leakage, count_raw_placeholders, update_last_completed_step, mark_interrupted, latest_resumable) — +1 `step6_prompt_enforces_mermaid_safety_rules` — +5 tests audit quality overhaul (#302-306) — +3 `ai_docs::tests` (dirs-first ordering) — +1 `phase3_is_bulk_first_not_one_by_one` (FR/EN/ES regex pin) — +9 `audit::validation` (REVIEW pseudo / empty / non-docs / cli-failed / healthy / empty-repair / truncated-repair / threshold-edge / missing-template) ; +7 `copy_dir_nondestructive` corruption-repair. — corruption-repair heuristic dans `copy_dir_nondestructive`.
- Frontend : 1172 → **1260** (+88) — apiMock + Dashboard mock updates pour audit-resume support — +3 tests `MermaidDiagram` pour le streaming guard + error-SVG detection + allowlist roots — +5 `MessageBubble.validationCta` (CTA visible w/ marker + projectId, hidden orphan, hidden no-marker, hash+navigate, marker strip) — +2 tests `MermaidDiagram` pour fullscreen overlay (open + Escape close + aria-modal) et print popup (window.open mocké, assert SVG inliné + `window.print()` trigger). — +3 tests `MermaidDiagram` (SVG valid render via mocked mermaid module, parse error fallback with raw source visible, Show/Hide source toggle). — +9 tests `ActiveAuditsPopover` (empty state, row per audit, click → onNavigateToProject, Stop btn calls cancelAudit + onAfterCancel + stopPropagation, Escape close, footer onViewAllProjects, NaN-safe elapsed clamping, fallback project_id label when projects list lags). — 4 tests `ProjectCard.audit-resume.test.tsx` (resume sans checkpoint, idle sans spam onRefetch, seed localStorage, transition active→idle déclenche refetch), 5 tests SSE dispatch #281 (`step_progress` forwards 3-tuple, default 0 sur cumul manquant, ignore non-numeric, `tool_call` forwards N-th-call, handlers optionnels backwards-compat) + 2 Playwright E2E `audit-banner-lifecycle.spec.ts` (banner appears/disappears au cycle audit, banner reste absent sans audit) — frontend-pure (route mocks, zéro token claude). — `TriageManifestPanel` happy + fallback non-triage, `tryParseTriageManifest` (10 edge cases : malformed JSON, escaped quotes, nested, missing categories, non-array values, prose preamble, braces in strings, empty manifest), `TriageManifestPanel` empty arrays / files count / no options / toggle, 13 tests `apiCallPluginTips` Resend + Mailjet, **3 tests SSE-dispatch `fullAuditStream` legacy_docs** (handler appelé avec payload complet, handler optionnel ne crashe pas les anciens callers, fields manquants → defaults safe), **4 tests SSE-dispatch enriched audit progress** (start event forwards `totalSteps`+`startedAt`, `step_done` forwards `tokens`/`durationMs`/`totalTokens` positionnellement, backwards-compat sans tokens, `onAuditStart` optionnel), **8 tests unaudited-project warning banner** (visible NoTemplate/TemplateInstalled/Bootstrapped, hidden Audited/Validated, hidden sur briefing/bootstrap/validation discs, hidden sans project_id, CTA adaptatif briefing vs launch, navigation vers projectId), **8 tests "Mark all as read" sidebar button** (visible avec unread + handler, count dans le tooltip, click invoque le handler une fois, hidden si tout vu, hidden sans handler, archives comptent, active disc compte, `Math.max(messages.length, message_count)` lock sur le seed).

### Validated against a real big-ticket (multi-brand cross-repo migration)

Run A/B sur le même ticket, même workflow, back-to-back :

| Métrique | v4 (baseline) | v5 (linked_repos + cross-repo) | Δ |
|---|---|---|---|
| Total tokens | 104,939 | 63,924 | **-39.1 %** |
| Triage tokens | 35,020 | 24,509 | -30 % |
| Implement tokens | 64,708 | 35,402 | **-45 %** |
| Durée | ~33 min | ~20 min | -40 % |
| Mocked | 3 | 1 | -67 % |
| Blocked | 3 | 2 | -33 % |
| Cross-repo `evidence:` cites | 0 | ≥4 | ubiquitous |

L'agent a détecté + remonté avec citation fichier une **discrepancy ticket↔prod sur un champ de config** (ticket=2, prod=1 dans `parameters_brand.yaml:2`) — bug que la prod aurait silencieusement absorbé dans une release sans contrôle.

## [0.8.2] - 2026-05-13

**Audit drastique + boucle audit → AutoPilot + worktree discoverability.**
Release centrée sur la qualité de l'audit IA et la fermeture de la
boucle "audit → tickets → AutoPilot → PR". L'audit ne se contente plus
de produire des constats : il a une baseline mandatory non-skippable,
une anti-répétition (slug-matching + reconciliation pass + two-tier
Status), un dispatch par kind (Security / Docker / Performance / A11y /
Database / ApiDesign / Custom) avec cluster detector qui recommande la
prochaine spécialisation, et une table `audit_runs` qui donne au badge
santé sa sparkline + delta. Côté workflow : un bouton "Continuer avec
l'AutoPilot" apparaît après la validation, qui pré-remplit le wizard
sur le ticket le plus ancien du tracker (GitHub / GitLab / Jira) avec
detection du repo. Côté Exec : nouveau `exec_setup_command` (composer
install / npm ci / etc.) avec preset dropdown, plus le fix du
docker-in-docker volume mismatch (self-mount + cwd translation pour les
worktrees), plus un meilleur signaling de "ta commande tourne dans un
worktree git". WebSocket `WorkflowRunUpdated` ajouté pour que la
transition vers un Gate s'affiche live sans refresh quand on arrive
d'un autre onglet.

### Added

- **Audit baseline mandatory checklist (Step 9)** — 4 checks
  non-skippables (auth, persistence, external input, secrets) qui
  émettent une TD baseline même quand le scan dimensionnel n'a rien
  trouvé. Les audits ne reviennent plus "vides" sur du code qui mérite
  au moins un signalement.
- **Audit cap relaxation** — 15-20 → 30 TDs max par run, Critical/High
  exempts (jamais omis). Sur les gros repos l'audit ne s'arrête plus
  artificiellement après Medium 15 en ignorant des Highs.
- **Audit anti-repetition** — trois protections : (1) slug-matching sur
  TDs existantes (un nouveau scan ne crée plus de doublon avec un slug
  légèrement différent), (2) reconciliation pass qui marque les TDs
  obsolètes comme `Resolved` au lieu de les laisser orphelines, (3)
  two-tier Status (`Active` / `Reopened`) pour distinguer une vraie
  régression d'un faux positif. Le slug-churn (le pire anti-pattern
  d'audit) est désormais bloqué par construction.
- **AuditKind enum + per-kind dispatch** — `Full` reste la base, plus
  `Security`, `Docker`, `Performance`, `Accessibility`, `Database`,
  `ApiDesign`, `Custom`. Chaque kind a son prompt système dédié et son
  set de checks baseline. Un audit Security n'est plus un audit Full
  avec un peu de focus sécu.
- **Cluster detector + AuditRecommendation** — Step 10 du Full audit
  inspecte la distribution des TDs et recommande la prochaine
  spécialisation à lancer (ex : 4+ TDs Security → "lance un audit
  Security"). Surfaceé en chip cluster dans le health badge.
- **`audit_runs` table + health badge cluster** — chaque audit crée une
  row avec `started_at`, `ended_at`, `duration_ms`, `td_critical/high/
  medium/low/total`, `td_resolved_since_last`, `td_new_since_last`,
  `td_carried_over`, `health_score` (0-100). Source de vérité pour le
  badge santé du dashboard.
- **AutoPilot CTA after audit validation** — bouton "Continuer avec
  l'AutoPilot" qui apparaît sur la discussion de validation une fois
  l'audit clôturé. Pré-remplit le wizard de workflow sur le ticket le
  plus ancien du tracker du projet (GitHub / GitLab / Jira), avec
  detection automatique du repo (`parseRepoUrl` +
  `inferTrackerSlugFromRepoUrl`). En un clic : audit → TDs → ticket →
  AutoPilot prêt à tirer.
- **Exec `exec_setup_command` + `exec_setup_args`** — phase setup avant
  la commande principale d'un step Exec, avec preset dropdown
  (`composer install`, `npm ci`, `pnpm install --frozen-lockfile`,
  `yarn install`, `poetry install`, `pip install -r requirements.txt`).
  Indispensable pour que la commande principale (tests / build) trouve
  ses dépendances dans un worktree fraîchement créé.
- **WS `WorkflowRunUpdated` event** — broadcast à chaque transition
  d'étape + flip de status du run. Le frontend rafraîchit la liste des
  runs quand on ouvre la page d'un workflow en cours depuis un autre
  onglet, sans devoir F5. La transition vers un Gate apparaît live.
- **Per-step token badge in WorkflowDetail** — le compteur de tokens
  n'est plus seulement au niveau du run, il est aussi affiché par
  step. Plus de surprise sur quelle étape consomme.
- **Authoritative `step.started_at` timestamp** — chaque `StepResult`
  capture l'heure wall-clock de démarrage côté backend (plus d'estimate
  côté frontend basé sur la somme des durées précédentes). La durée
  vraie d'un step est désormais persistée et survit aux reloads.
- **Gate pause duration tracking** — le `duration_ms` d'un step Gate
  reflète maintenant la vraie durée de la pause (now - started_at)
  quand l'opérateur valide. Avant : ~0ms (temps de rendu), maintenant :
  le temps que l'humain a mis à décider.
- **`effectiveLiveRun` cross-tab persistence** — quand on navigue vers
  un workflow en cours depuis un autre onglet, on synthétise un état
  "pseudo-live" à partir du dernier run non-fini de la liste. Plus de
  "page collapsée vide" qui fait croire que le run est bloqué.
- **Tracker hint banner on ProjectCard** — surface l'URL du tracker
  détectée (`parseRepoUrl(project.repo_url)`) avec un dismissible
  localStorage flag, pour amorcer la conversion repo → AutoPilot.
- **`buildOldestIssueRequest` helpers** — switch par tracker
  (`github` / `gitlab` / `jira`) qui produit la bonne requête HTTP pour
  récupérer le ticket ouvert le plus ancien. 9 tests unitaires.
- **Exec step worktree discoverability hints** — hint dédié pour Exec
  step au premier rang (fresh worktree) vs steps suivants (sees
  previous changes), plus warning visible quand `project_id` est null
  (commande tourne dans le CWD de Kronn, pas de worktree).
- **Audit elapsed time counter** — ticker côté client (1s) qui affiche
  le temps écoulé depuis le démarrage de l'audit en cours, calé sur le
  `started_at` du serveur. Plus d'incertitude pendant les 10-20 min
  d'un audit Full.
- **Volume mounts for non-standard CLI paths in Docker** — `cargo`,
  `bun`, `~/.rustup`, plus un `/host-bin/extra` escape hatch. Auto-
  detection dans le `Makefile` qui écrit `.env` si les répertoires
  existent. Couvre les ~20% d'users qui n'ont pas leurs outils dans
  `/usr/bin` ou `~/.local/bin`.
- **GitHub Community Standards files** — `CODE_OF_CONDUCT.md`
  (Contributor Covenant 2.1), `SECURITY.md` (private advisory route,
  SLA, scope), `.github/ISSUE_TEMPLATE/{bug_report,feature_request,
  config}.{md,yml}`, `.github/pull_request_template.md`.
- **README EN + FR section 5 & 6 rewrites** — la section "Audit your
  codebase with an AI that doesn't forget" reformulée pour couvrir les
  6 hardenings 0.8.2 (Mandatory baseline, Anti-repetition, Two-tier
  Status, Specialized kinds, Health badge cluster, Community-standards
  gate). Nouvelle section "Close the loop: audit → tickets →
  AutoPilot → PR".

### Changed

- **CSS extraction for `ActiveRunsPopover`** — déplacé hors de
  `pages/WorkflowsPage.css` vers un fichier co-located
  `components/workflows/ActiveRunsPopover.css`. Avant : le popover des
  runs actifs (rendu depuis Dashboard, donc visible sur tous les
  onglets) apparaissait unstyled quand on cliquait dessus depuis
  Discussions tant que WorkflowsPage n'avait pas été monté au moins
  une fois.
- **Docker volume mounting strategy** — self-mount + cwd translation
  `/host-home/` → `${KRONN_HOST_HOME}/` pour les worktrees git
  créés sur le host et lus depuis le container. Le path parity est
  désormais préservé inside/outside container, prérequis pour les
  steps Exec qui touchent des worktrees.
- **`RUSTUP_HOME` propagation** — le container reçoit la même valeur
  que le host pour que les shims `cargo` / `rustc` trouvent leur
  toolchain. Mount du dossier `~/.rustup` au même chemin absolu.
- **Tracker MCP detection precedence** — `repo_url > project-scope >
  global` au lieu de `is_global > everything else`. Empêche un Jira
  global de masquer un GitHub spécifique au repo.

### Fixed

- **CSS missing on live-WF box when arriving from another tab**
  (TD #248) — le popover des runs actifs apparaissait sans style sur
  les onglets Discussions/Projects/Settings tant que WorkflowsPage
  n'avait pas été mounté.
- **Live Gate transition without page refresh** (TD #247) — la
  transition d'un run vers un Gate (status `Running` → `WaitingApproval`)
  ne se voyait pas live quand le panel était ouvert depuis un autre
  onglet : la SSE est tab-local, l'autre tab ne recevait rien. Le WS
  `WorkflowRunUpdated` mirror les transitions sur tous les clients.
- **Docker-in-docker volume mismatch for worktree Exec steps**
  (TD #249) — un step Exec qui tournait sur un worktree créé côté host
  voyait un `work_dir` invalide à l'intérieur du container (le path
  host n'existait pas), faisant échouer toute commande qui faisait du
  `find` ou de l'IO. Self-mount + traduction de chemin garantissent
  que le `cwd` est valide des deux côtés.
- **GitHub API 422 on `buildOldestIssueRequest`** — User-Agent manquant
  sur le reqwest builder. Ajout de `.user_agent(concat!("Kronn/",
  env!("CARGO_PKG_VERSION")))`.
- **bash + `["make test"]` foot-gun** — validator catché à la
  sauvegarde du workflow, avec message actionnable qui explique de
  splitter `["-c", "make test"]` ou d'utiliser directement `make`
  comme binaire.
- **Per-disc sendingMap leak on batch fan-out** — `BatchRunProgress`
  inclut maintenant le `discussion_id` de l'enfant qui vient de
  terminer pour que le frontend puisse clear son indicateur local
  (les enfants de batch n'ont pas de consommateur SSE).
- **Cargo `rustup` shim toolchain lookup** — les shims ne trouvaient
  pas la toolchain dans le container parce que `~/.rustup` n'était pas
  monté au même chemin absolu. Mount + `RUSTUP_HOME` env propagation.

### Tests

- 2 round-trip serde tests pour `WsMessage::WorkflowRunUpdated`
  (variant complète + variant `current_step=None`).
- 8 validator tests pour `validate_exec_steps`
  (`bash`-multi-word foot-gun + `exec_setup_command` allowlist +
  path-separator + shell-vs-bin distinction).
- 9 tests `buildOldestIssueRequest` (GitHub / GitLab / Jira shapes).
- Mock `useWebSocket` ajouté à `WorkflowsPage.test.tsx` +
  `WorkflowsPage.qp-launch.test.tsx` (le hook réel essayait d'ouvrir
  une WS dans jsdom).
- Suite complète au vert : 1870 tests backend, 1161 tests frontend.

## [0.8.1] - 2026-05-12

**Custom API plugin + AI helpers UX refactor + tech-debt prominence + doc rebrand.**
Release de "vraies features qui débloquent du monde" : N'importe quelle
API REST peut maintenant être pilotée par Kronn (plus uniquement
Chartbeat/Adobe/Jira), les helpers IA ouvrent direct sur le chat (plus
de modal séparé pour choisir l'agent), la dette technique est visible
en un coup d'œil sur chaque projet, et toute la terminologie
"AI documentation" passe en "project documentation" (le pivot
`ai/` → `docs/` du 0.7.1 est désormais complet jusque dans les UI strings
et les agent prompts).

### Added

- **Custom API plugin** — sentinel `api-custom` dans `core/registry.rs`,
  pinnée en tête du drawer "Add plugin". Picking it swap le panneau de
  droite vers un éditeur freeform (Name + Base URL + Describe + Docs
  link + N {Label, Value} fields). Le backend matérialise un fresh
  `McpServer` (id `custom-{slug}-{nano}`, source = `Manual`, transport
  `ApiOnly`) avec `ApiSpec` construite depuis le payload. Auth = `None`
  par design : l'agent lit la description + docs URL + fields et figure
  out l'auth lui-même. Helpers `slug_env_key` (slugifier
  `Bearer Token` → `BEARER_TOKEN`) + `materialize_custom_server` +
  `name_slug`. 5 tests Rust + 2 tests vitest. Couvre tous les use cases
  "j'ai une API interne / Salesforce / Stripe / autre vendeur non listé".
- **Custom API AI helper bubble (`CustomApiAiHelper.tsx`)** — chat
  éphémère qui pré-remplit le formulaire Custom API depuis un curl, un
  lien doc ou une description libre. Mirror du pattern
  `ApiCallAiHelper` (KRONN:APPLY blocks, ephemeral discussion,
  agent dropdown). System prompt dédié qui extrait
  `{name, base_url, description, docs_url, fields[]}`. Apply merge
  intelligent : préserve les valeurs utilisateur déjà saisies, accepte
  les nouveaux labels de l'agent. 16 unit tests pinent le wire
  contract + le rendu.
- **AI helper UX refactor (option B)** — passe `ApiCallAiHelper` de 3
  phases (closed/picking-agent/chatting) à 2 (closed/chatting). Click
  trigger → bulle ouverte direct avec le 1er agent installé. Header de
  bulle accueille un dropdown agent (avatar + nom + chevron) qui
  permet de switcher au milieu d'une conversation (reset le chat, prime
  une nouvelle discussion avec le même system prompt). Context chip
  remonté en haut de la bulle (sous le header) pour qu'on voie ce que
  l'agent sait avant le scroll. Welcome state avec 3 starter chips
  cliquables (pré-remplissent l'input avec un template) à la place de
  l'agent qui s'auto-fire à l'ouverture — économise ~200 tokens par
  helper-open. Tests mis à jour. CSS extraite dans
  `frontend/src/components/aiHelper.css` pour que les styles chargent
  aussi sur McpPage (le bug qui rendait la bulle non-stylée sur
  d'autres pages).
- **Tech-debt count badge on ProjectCard** — nouvelle field
  `Project.tech_debt_count: u32` peuplée par `scanner::count_tech_debt`
  qui compte les TD-* uniques (union dédupliquée des fichiers sous
  `docs/tech-debt/` + des lignes `| TD-` dans
  `docs/inconsistencies-tech-debt.md`). Affichée comme badge orange
  `⚠ N TD` sur la ligne du titre du projet. Click → ouvre la card si
  elle est fermée + déplie la section docs + deep-link
  `initialExpandFolder='docs/tech-debt'` qui auto-sélectionne le
  premier TD-*.md. README/TEMPLATE.md exclus du compte (scaffolding).
  4 tests Rust dont un dédié à la régression de double-comptage.
- **"Régler ce problème" CTA on TD files** — quand l'AiDocViewer
  affiche un fichier `docs/tech-debt/TD-*.md`, le bouton
  "Discuss this file" devient "Régler ce problème" (warning-tone,
  bouton bold). Même action sous-jacente (lance une discussion avec le
  fichier en contexte) mais le prompt est résolution-oriented : ask
  l'agent un plan court, exécuter les modifs, mettre à jour le
  TD-*.md (statut résolu) et la ligne d'index. Détection via regex
  permissive `/tech-debt/.*TD-*.md` — symétrique avec
  `count_tech_debt` côté backend.
- **Docs viewer always-visible + state banners** — la section
  "Project documentation" sur la ProjectCard n'est plus gatée sur
  `audit_status === 'Validated'`. Elle s'ouvre quel que soit l'état
  d'audit. Une bannière contextuelle dans le viewer guide vers la
  prochaine étape :
  - `NoTemplate` / `TemplateInstalled` : "Lance un audit IA pour
    (re)documenter intégralement le projet…"
  - `Bootstrapped` : "Bootstrap terminé. Lance l'audit complet…"
  - `Audited` : "Valide l'audit pour avoir une documentation à jour…"
  - `Validated` : pas de banner (état "propre")
  Auto-fix : quand on clique le badge TD sur une card fermée, la card
  s'ouvre + déplie la section docs (avant on cliquait dans le vide).
- **AI audit Step 9 (tech-debt) enrichi** — `ANALYSIS_STEPS[8]` dans
  `backend/src/api/audit/mod.rs` passe de 7 dimensions à **10** :
  ajout d'**Accessibility** (form labels, contrast 4.5:1, ARIA,
  keyboard-nav, focus traps, semantic HTML), **Observability**
  (logging hot paths, error tracker, health endpoints, SLI metrics),
  **Documentation drift** (cross-check des 8 fichiers `docs/` que
  l'agent vient d'écrire contre le code source — détecte
  contradictions type "coding-rules.md dit X mais aucun linter ne
  l'enforce"). Le detail file gagne 3 champs : **Status**
  (Draft / In progress / Blocked upstream / Mitigated),
  **Effort** (S/M/L/XL), **Blast radius**
  (local / module / cross-cutting). Calibration de la severity
  avec exemples concrets (Critical = data leak / SQL injection,
  High = test suite red / build broken, Medium = test suite >30s
  / N+1, Low = cosmetic) pour limiter la sur-classification en
  Medium. Nouvelle règle "tickets dedup" : si un MCP tracker
  (Jira/Linear/GitHub) est configuré, l'agent fait une recherche
  read-only avant de créer un TD pour éviter de dupliquer un ticket
  existant. Tests audit (13) toujours verts. Compatible 100% backwards :
  les TDs déjà créés avec l'ancien format restent valides.
- **Persistent AI audit section dans le README** — nouveau §5 dans
  "What you can do" qui détaille les 8 fichiers générés
  (`docs/AGENTS.md`, `glossary.md`, `repo-map.md`, `coding-rules.md`,
  `testing-quality.md`, `architecture/overview.md`,
  `operations/debug-operations.md`, `operations/mcp-servers.md`) +
  le status flow `NoTemplate → TemplateInstalled → Bootstrapped →
  Audited → Validated` + le drift detection granulaire par section.
  Sells "Kronn = knowledge persistence layer, not just a prompt
  launcher".
- **`AiDocViewer` props `initialExpandFolder` + `banner`** — slots
  optionnels qui ne cassent aucun consumer (props
  `?`). `initialExpandFolder` déplie tous les prefixes du folder en une
  seule passe + pre-sélectionne le premier fichier qui matche.
  `banner` est un React node libre, le caller contrôle icône + ton.
  Helper `findFirstFileUnder` ajouté.
- **Custom API helper E2E spec** (`custom-api-helper-bubble.spec.ts`) —
  smoke Playwright qui couvre les ouverture de la bulle, les starter
  chips, l'agent dropdown, et la fermeture. Vérifie
  `getComputedStyle(bubble).position === 'fixed'` comme proxy pour la
  régression CSS qui avait initialement motivé l'extraction
  `aiHelper.css`.
- **README + dark screenshots EN/FR** — 8 PNG en thème sombre (4 ×
  EN + 4 × FR) pour le dashboard, Quick Prompts, QP launch
  (compare-agents avec 7 chips), workflow wizard. Banner
  `Kronn_Hero.png` + 4 SVG diagrammes (decomposition + data-flow, FR/EN)
  dark-only pour cohérence visuelle avec le logo. Script
  `scripts/seed-demo-fixtures.sh` reproductible + page
  `docs/operations/screenshot-sandbox.md` qui documente le workflow.
  Section "Any REST API works" ajoutée pour expliquer le Custom API
  flow.

### Changed

- **Doc rebrand `ai/` → `docs/` complet** — passe sur tous les
  `.md` du repo Kronn lui-même (~30 refs dans `docs/AGENTS.md`,
  `glossary.md`, `decisions.md`, `repo-map.md`,
  `architecture/overview.md`, `operations/mcp-servers/drawio.md`).
  Tooling ne lit plus jamais `ai/` (la migration shippée en 0.7.1 est
  désormais complète en surface ET en profondeur). Les refs
  historiques type "legacy `ai/` directory was migrated to `docs/` in
  0.7.1" sont gardées comme notes historiques.
- **Terminology "AI documentation" → "project documentation"** —
  13 strings i18n × 3 langues (FR/EN/ES) plus les hardcoded JSX
  badges sur `ProjectCard.tsx`. Le badge "AI context" devient
  "Project docs". Les agent prompts (`audit.validationPrompt` ×3,
  ~1k tokens chacun) sont récrits pour pointer vers `docs/` (au lieu
  de `ai/`) — l'agent va donc maintenant écrire dans le bon dossier
  après le pivot.
- **Templates de bootstrap** — `templates/docs/AGENTS.md` :
  "Modify business code when the task is only about AI context" devient
  "...only about project documentation". `templates/docs/architecture/
  overview.md` : "Architecture (AI context)" → "Architecture". Tout
  nouveau projet bootstrappé naît avec la nouvelle terminologie.
- **Sandbox screenshot pipeline** — em-dashes nettoyés du
  `scripts/seed-demo-fixtures.sh` (préférence user : "we never do that"),
  3 phrases bancales (après suppression em-dash) rephrasées pour rester
  grammaticales. CSS shared move vers `frontend/src/components/aiHelper.css`
  (avant : `WorkflowsPage.css`) — corrige le bug qui rendait la bulle
  helper non-stylée sur McpPage.

### Fixed

- **Workflow trigger: variables non-déclarées auto-détectées** —
  user-reported sur "autoBot" workflow : step 1 utilise `{{issue}}`
  dans le prompt mais `Workflow.variables` était vide → le launch
  modal était skippé → step fire avec literal `{{issue}}`. Fix :
  nouveau helper `lib/workflowVariables.ts` qui scanne TOUS les
  champs templated d'un workflow (`prompt_template`, `api_endpoint_path`,
  `api_query`/`api_headers`/`api_body`, `notify_config.url`/
  `body_template`/`headers`, `exec_args`, `batch_items_from`) +
  retourne les `{{var}}` non-runtime. `handleTrigger` merge
  declared + auto-detected, ouvre le modal s'il y a quelque chose à
  saisir. Change connexe : `isRuntimeToken` (apiCallPlaceholders.ts)
  filtre désormais UNIQUEMENT les `ns.X` multi-segments — un
  `{{batch}}` bare est maintenant traité comme user-var (avant : eaten
  silently). 12 tests neufs dans `lib/__tests__/workflowVariables.test.ts`
  dont une régression dédiée `autoBot {{issue}} regression`.
- **`docs_migration` re-runs rewrite pass sur AlreadyMigrated** —
  user-reported : projets déjà migrés vers `docs/` gardaient des refs
  `ai/...` stales dans le contenu de leurs `.md` parce que le early
  return `AlreadyMigrated` skippait `rewrite_internal_refs` +
  `rewrite_root_redirectors`. Fix : variant devient
  `AlreadyMigrated { refs_rewritten: usize }`, les deux rewriters
  (idempotents) sont appelés systématiquement, le compteur retourné
  dans la réponse HTTP pour que l'opérateur voit "12 refs cleaned"
  quand il re-clique sur "Migrer". `MigrateDocsResponse.refs_rewritten`
  désormais peuplé même pour `status: "already_migrated"`. 1 test neuf
  `already_migrated_cleans_stale_ai_refs` qui prouve qu'un repo déjà
  à `docs/` avec des `ai/X` refs résiduelles sort propre après
  re-trigger.
- **`count_tech_debt` double-counting (régression flaggée user)** —
  avant : 5 fichiers + 7 lignes index = 12 sur le badge alors que
  l'utilisateur ne voit que ~7 unique TDs dans la doc. Maintenant
  dédupliqué par ID (extrait du `file_stem` côté fichiers + du
  premier token `TD-...` côté lignes). Sur Kronn lui-même : 12 → 7
  (cohérent). Test dédié `count_tech_debt_dedupes_file_and_index_pair`
  pin la régression.
- **E2E `custom-api-helper-bubble.spec.ts` count-before-visible** — le
  test échouait en CI parce que `expect(toBeVisible)` s'exécutait
  avant le check `skip if no agents installed`. Ordre inversé + tick
  de settle DOM ajouté. Skip cleanly maintenant quand le sandbox CI
  n'a pas d'agents installés.
- **TD badge click + card fermée** — avant : clic sur `⚠ 12 TD`
  appelait `setExpandedTab('docAi')` mais la card étant fermée, le
  body n'était pas rendu → l'utilisateur cliquait dans le vide.
  Maintenant : `if (!isOpen) onToggleOpen()` ajouté avant le
  setExpanded. Un seul click suffit pour passer de "card fermée" à
  "viewer ouvert sur le premier TD".
- **`docs/architecture/overview.md` heading `(AI context)`** —
  cohérence avec le rebrand global, ce reliquat se balladait.

### Tests

- Backend : **1614 tests** (1613 + 1 nouveau test `count_tech_debt`
  pour la régression dédup). `cargo clippy --lib -- -D warnings` clean.
- Frontend : **1128 tests** (1112 + 16 nouveaux `CustomApiAiHelper`).
  `pnpm tsc --noEmit` clean. `pnpm lint` : 0 errors, 100 warnings
  (toutes pré-existantes).
- E2E : nouveau spec `custom-api-helper-bubble.spec.ts`.

### Docs

- `docs/architecture/overview.md` : nouveaux paragraphes
  Custom API plugins + AI helper bubble (UX 0.8.1, shared CSS,
  TD-helpers-unify noté).
- `docs/operations/screenshot-sandbox.md` : nouveau, ~45 lignes,
  documenté + référencé depuis `CONTRIBUTING.md`.
- README.md + README.fr.md : new section "Any REST API works", new §5
  Persistent AI audit, 0 em-dashes (préférence user).

---

> **Older releases (0.8.0 and below)** are no longer kept in this file to keep it readable. Full history available via `git log -- CHANGELOG.md` and the GitHub releases page.

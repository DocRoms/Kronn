# Changelog

All notable changes to Kronn will be documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

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

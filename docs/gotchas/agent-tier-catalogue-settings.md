# Agent tier settings must preserve other editors

The Config > Agents editor used a static model list even though the catalogue
table below it had loaded dynamic runtime data. Its load loop also omitted
`open_code`, while its save payload included that key with null defaults.
Changing another agent's tier therefore erased saved OpenCode settings.

The correction reads the shared catalogue, selects the exact runtime namespace
and preserves an unknown configured model as a disabled, explicitly labelled
option. Cache provenance never becomes a live discovery merely because the
snapshot request succeeded. Clearing an override is an explicit action; its
placeholder and card preview show the catalogue's tier assignment when known.
[src: file: frontend/src/lib/modelCatalogSelection.ts:76-113]
[src: file: frontend/src/components/settings/AgentsSection.tsx:1215-1238]

The settings endpoint replaces the whole model-tier document. Before a save,
the editor rereads it and merges only the selected agent/tier, guarded against
synchronous re-entry. It updates its displayed state after a successful write,
not optimistically. This preserves edits already saved by another card, but is
not a server-side compare-and-swap guarantee against another client writing
between that GET and POST.
[src: file: frontend/src/components/settings/AgentsSection.tsx:149-168]

The component regressions cover dynamic IDs, runtime separation, OpenCode,
manual Kiro/Vibe entries, unavailable/cache metadata, empty/error recovery,
explicit clearing, independent Ollama edits, duplicate clicks and write failure.
They mock the API and do not mutate provider accounts or user settings.
[src: file: frontend/src/components/settings/__tests__/AgentsSection.catalog.test.tsx:75-235]

## Shared picker identity and provenance

### One-time catalogue bootstrap

Historical model IDs are initial migration data, not normal runtime fallbacks.
On an unbootstrapped database, an old default no longer claims a tier explicitly
configured to another model. Configured Ollama tiers are imported alongside
the CLI tiers. Seed insertion preserves an existing model row and fills only
an unassigned tier in the exact runtime namespace; it does not overwrite a
manual/live row or compete with an unavailable existing assignment.
[src: file: backend/src/core/model_catalog/mod.rs:101-219]
[src: file: backend/src/db/model_catalog.rs:799-830]

Bootstrap records its durable marker in the same database operation. Reopening
the database and invoking bootstrap again leaves rows and timestamps unchanged,
does not restore deleted seeds, and does not import later configuration edits.
This correction does not reset that marker or repair already-bootstrapped
databases. No user database or configuration was changed during qualification.
[src: file: backend/tests/model_catalog_migration.rs:39-87]

Configuration remains unchanged and retains priority at resolution/preflight.
Several explicitly configured tiers can still name the same model even though
one catalogue row has only one tier assignment. The integration tests preserve
that distinction, including an existing historical identity assigned to another
tier and a refusal for the explicitly configured unavailable model.
[src: file: backend/tests/model_catalog_migration.rs:159-263]
[src: file: backend/src/agents/runner.rs:2650-2725]

Qualification on backend tree `189eb0f577ac44372ea5e85540ff5052321729e7`:
the first five migration controls produced four failures on unchanged code;
the later Ollama control separately failed before its fix. Final focused
production-library replay: 20 PASS across migration, HTTP resolution and
catalogue API suites. Full unfiltered offline run 12790: 6,866 PASS, 0 failed,
6 historical ignored tests, 22 suite results; library 380.31 seconds and cold
API 155.04 seconds. All-target strict Clippy passed in 15.31 seconds. Existing
ts-rs diagnostics and a linker compact-unwind-size warning remain. Frontend
tree `9f9df7f4` is unchanged; no new browser, coverage, provider or final KT-531
qualification is implied. The remaining selector inventory is separate.

### Resolution precedence

An explicit configured model wins over a different catalogue tier assignment.
An unknown explicit ID remains visible, never silently replaced by a known
assignment. A named HTTP connection cannot inherit another connection's
agent-family settings; even rows inside its view must match the exact runtime
namespace. HTTP tiers may fall back to their own configured default, unlike
CLI tiers. No embedded model list participates in the display helper anymore.
[src: file: frontend/src/lib/constants.ts:60-75]
[src: file: frontend/src/lib/modelCatalogSelection.ts:12-31]
[src: file: frontend/src/components/AgentSwitchPicker.tsx:116-128]

The shared picker keeps an unavailable configured entry disabled. A failed
snapshot read reports the error and retains previous data as cached, not live.
Saved per-discussion and Quick Prompt overrides are shown for the current
selection; alternative choices resolve their own tiers. The Quick Prompt
caller still explicitly clears its old override when choosing a new agent/tier.
[src: file: frontend/src/hooks/useModelCatalogSnapshot.ts:5-27]
[src: file: frontend/src/components/ChatHeader.tsx:418-428]
[src: file: frontend/src/pages/WorkflowsPage.tsx:1223-1244]
[src: file: frontend/src/pages/WorkflowsPage.tsx:2850-2857]

KT-627 separately removed the HTTP runner's embedded fallback and qualified
production-library resolution/preflight. The final migration and cross-surface
audit still keeps KT-531 open; a historical test result never qualifies a later
candidate by itself.
[src: file: backend/tests/http_model_resolution.rs:1]

### Search and keyboard interaction

The shared agent picker filters agent labels, exact connection identities and
configured/assigned model IDs or aliases. Search ignores case and diacritics
without rewriting the selected identity. Typing never selects a target or
refetches the catalogue. An unknown configured ID remains searchable, and a
known unavailable choice remains disabled; accessible descriptions retain the
model, provenance and unavailable state after filtering.
[src: file: frontend/src/lib/modelCatalogSelection.ts:36-45]
[src: file: frontend/src/components/AgentSwitchPicker.tsx:129-159]
[src: file: frontend/src/components/AgentSwitchPicker.tsx:334-373]

The search field is outside the menu, within a portalled non-modal dialog.
Arrow keys move among enabled choices; Home/End retain their caret behavior in
search and navigate choices when an option has focus. Escape inside the picker
does not propagate to its parent form. Closing with Escape or completing an
asynchronous selection restores trigger focus. Reopening clears the query,
an empty result is explicit, and the popup's height is bounded by the viewport.
[src: file: frontend/src/components/AgentSwitchPicker.tsx:134-216]
[src: file: frontend/src/components/AgentSwitchPicker.tsx:296-321]
[src: file: frontend/src/components/__tests__/AgentSwitchPicker.catalog.test.tsx:123-247]
[src: file: frontend/src/components/__tests__/AgentSwitchPicker.accessibility.test.tsx:42-73]

These are component regressions, not a new browser layout qualification.
The remaining KT-531 review covers the final cross-surface inventory. Comparison
and orchestration deliveries are recorded separately, without carrying these
component results forward as browser qualification.

### Composer mention catalogue contract

Both composer palettes share tier buttons and the same catalogue resolver and
opening-time snapshot reader as the agent picker. A named principal connection
uses only its own tier configuration; an empty HTTP tier may use that same
connection's default. Exact model IDs, provenance and verification time remain
accessible even when an alias is displayed. Failed reloads retain stale data
and an explicit error; typing within an open palette does not refetch.
[src: file: frontend/src/lib/modelCatalogSelection.ts:12-31]
[src: file: frontend/src/hooks/useModelCatalogSnapshot.ts:5-27]
[src: file: frontend/src/components/MentionTierChoices.tsx:7]
[src: file: frontend/src/components/ChatInput.tsx:663-725]

Untouched principal insertion preserves its saved model override and does not
write a tier preference. Explicit tier choices resolve their own configuration.
Unavailable tier buttons remain visible and disabled; horizontal keyboard
navigation skips them without wrapping. A disabled tier's pointer event must
not bubble into the new form's implicit default selection. Joined CLI mentions
keep their exact session and stable ordinal, expose no model tier, and do not
change native preferences. New-discussion submissions retain the immutable
connection and explicitly selected tier.
[src: file: frontend/src/lib/mentionTierSelection.ts:6-16]
[src: file: frontend/src/components/NewDiscussionForm.tsx:717-745]
[src: file: frontend/src/components/__tests__/ComposerMentionCatalog.test.tsx:142-256]

The new regressions use API fixtures, including colliding IDs in two HTTP
namespaces and a real send/creation callback assertion. They neither call a
provider nor qualify browser layout or the separate comparison worker.

### Shared mention search

Both palettes and `AgentSwitchPicker` now share the same case/diacritic-insensitive
search over exact target namespaces and already-resolved configured IDs/model
aliases. Connection names are searchable independently of model aliases. Known
unavailable models remain visible and disabled; unknown configured IDs stay
explicitly unverified. A joined CLI's results never inherit a native catalogue
tier. Canonical alias prefixes rank before additional substring matches, so
typing `@co` still highlights Codex before a Claude Code substring result.
[src: file: frontend/src/lib/modelCatalogSelection.ts:36-45]
[src: file: frontend/src/components/ChatInput.tsx:674-695]
[src: file: frontend/src/components/NewDiscussionForm.tsx:209-213]
[src: file: frontend/src/lib/mention-autocomplete.ts:7-26]

The unfinished search token accepts Unicode letters and common model-ID
characters (`/`, `.`, `:`). This changes autocomplete only: selection replaces
the original caret range with the canonical trigger. Dispatch parsing is
unchanged, and typing alone never changes preferences or submits an agent.
ArrowDown on an empty result keeps a nonnegative index; when the catalogue
arrives, Enter can select the displayed row rather than indexing `-1` or
throwing. Display and keyboard use the same filtered list.
[src: file: frontend/src/components/__tests__/ComposerMentionCatalog.test.tsx:73-140]
[src: file: frontend/src/components/__tests__/ComposerMentionCatalog.test.tsx:216-247]
[src: file: frontend/src/lib/__tests__/mention-autocomplete.test.ts:20-27]

Qualification on frontend tree `9f9df7f4f06401a83b4e5383179fefa614c2f46e`:
10 initial RED controls, then three additional valid RED cases for connection
name and asynchronous keyboard results. The first asynchronous new-form fixture
incorrectly matched a connection name before catalogue arrival; it was corrected
and the intended RED reproduced. Expanded replay: 103 tests PASS in 1.85 seconds.
Unfiltered frontend run 13486: 313 files / 4,084 tests PASS in 56.98 seconds.
Both TypeScript compilers, Oxlint, ESLint (0 errors/63 existing warnings), i18n
(4,481 keys × 4, 708 static unused warnings) and Vite (7.14 seconds) passed.
Existing harness warnings remain; no rule was disabled, test excluded or
threshold relaxed. Backend tree `6040c898` is unchanged. No new browser,
coverage, provider or final KT-531 approval is implied.

## Quick Prompt and workflow model editors

Both editors now use `ModelCatalogPicker` and the shared searchable control.
The saved snapshot supplies model provenance, availability and reasoning modes
for the exact runtime. Opening the form no longer queries Ollama's separate
model endpoint. An operator can still explicitly enter a model ID absent from
the loaded catalogue; that option is labelled as unverified, never live.
Unavailable known entries remain disabled. Clearing an override is explicit
and leaves tier resolution to the caller/backend.
[src: file: frontend/src/components/ModelCatalogPicker.tsx:22-68]
[src: file: frontend/src/components/workflows/QuickPromptForm.tsx:308-315]
[src: file: frontend/src/components/workflows/WorkflowWizard.tsx:4131-4146]

Reasoning choices come from the effective model's `reasoning_modes`, not a
fixed low/medium/high list. An existing value absent from that list is retained
and explicitly labelled, not silently rewritten. The QP save path preserves
the edited reasoning value and the existing `max_tokens`; previously it
replaced both with null on every save. Choosing a different agent/tier in the
QP form explicitly clears the old model and reasoning override, while a
catalogue read or failure never changes either value.
[src: file: frontend/src/components/ModelCatalogPicker.tsx:33-63]
[src: file: frontend/src/components/workflows/QuickPromptForm.tsx:187-202]
[src: file: frontend/src/components/workflows/QuickPromptForm.tsx:248-257]

The regressions exercise the actual form payload, explicit unknown Unicode IDs,
reload failures, namespace switching, disabled entries and per-model reasoning.
They use API fixtures only; these tests are not provider discovery or a browser
layout qualification. The final cross-surface inventory and migration proof
still require the remaining KT-531 audit. Discussion target persistence
is covered separately below.
[src: file: frontend/src/components/workflows/__tests__/QuickPromptForm.catalog.test.tsx:37-88]
[src: file: frontend/src/components/__tests__/ModelCatalogPicker.test.tsx:25-66]

## Explicit target changes are not catalogue refreshes

Choosing another target or tier in a QP/workflow clears the old explicit model
and model-specific reasoning mode, while preserving unrelated settings such as
`max_tokens`. The payload pins the exact selected connection, or explicitly
clears it for a connectionless agent. A change between two connections of the
same agent family and tier is still a real target change, not a no-op. Merely
opening, editing or rereading the catalogue preserves the saved overrides.
[src: file: frontend/src/lib/agentSelection.ts:1-16]
[src: file: frontend/src/components/workflows/WorkflowWizard.tsx:261-277]
[src: file: frontend/src/pages/WorkflowsPage.tsx:821-844]
[src: file: frontend/src/pages/WorkflowsPage.tsx:1223-1244]

The creation wizard, full editor, inline inspector, pipeline and QP card receive
the configured named targets. Their selectors display the saved explicit model
and pass the selected immutable connection instead of carrying an old one via
an object spread. Component tests inspect the actual update payloads and exact
connection callback; these are not live workflow/provider executions.
[src: file: frontend/src/components/workflows/WorkflowDetail.tsx:1300-1330]
[src: file: frontend/src/components/workflows/__tests__/WorkflowWizard.test.tsx:523-574]
[src: file: frontend/src/components/workflows/__tests__/WorkflowDetail.steps.test.tsx:444-483]

## Workflow compensation targets

The workflow's `on_failure` Agent steps use the same agent/tier picker and
selection callback contract as its ordinary steps. An explicit connection,
agent or tier change clears the old model and reasoning override while retaining
`max_tokens`; saving without changing the target preserves the entire rollback
configuration. The callback updates only the compensation step, not the main
pipeline. Search, exact connection namespaces, provenance and unavailable tiers
therefore share the existing picker behavior.
[src: file: frontend/src/components/workflows/WorkflowWizard.tsx:262-298]
[src: file: frontend/src/components/workflows/WorkflowWizard.tsx:4469-4473]
[src: file: frontend/src/components/workflows/WorkflowWizard.tsx:4548-4553]
[src: file: frontend/src/components/workflows/__tests__/WorkflowWizard.coverage.test.tsx:829-895]

Five picker controls failed against the previous compensation form; the
unchanged-target control passed. The corrected two wizard suites pass 108 tests.
These are form/callback tests, not actual compensation or provider executions.

## Discussion target persistence

Discussion PATCH has no explicit model-override field. Its backend setters now
clear the old override only when the persisted agent, tier or connection
actually changes. Each setter updates its field and clears the model in the
same SQL statement. Resending an unchanged selection preserves its override;
ordinary title edits and historical message models remain untouched.
[src: file: backend/src/db/discussions.rs:1022-1030]
[src: file: backend/src/db/discussions.rs:1092-1103]
[src: file: backend/src/db/discussions.rs:1119-1132]

An explicit JSON `connection_id: null` is distinct from an absent field.
Clearing a connection is validated against the effective agent before any
requested field is written: an explicit clear is refused for Custom.
Resending the same agent no longer clears an existing connection or invalidates
the summary cache. A lightweight agent lookup avoids loading the transcript
merely to compare targets.
[src: file: backend/src/models/discussions.rs:435-445]
[src: file: backend/src/api/discussions/crud.rs:599-630]
[src: file: backend/src/api/discussions/crud.rs:671-694]
[src: file: backend/src/db/discussions.rs:1012-1020]

Seven isolated Router/SQLite regressions cover changed and unchanged targets,
null versus absent, invalid-target refusal and historical answer preservation.
Six failed on the unchanged backend; the control for ordinary edits passed.
These tests never launch an agent or contact a provider and do not qualify the
separate HTTP fallback or Important-message authority work.
[src: file: backend/tests/discussion_target_model.rs:12-57]
[src: file: backend/tests/discussion_target_model.rs:77-211]

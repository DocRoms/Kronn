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
[src: file: frontend/src/lib/modelCatalogSelection.ts:95-146#catalogModelOptions]
[src: file: frontend/src/lib/modelCatalogSelection.ts:90-93#catalogModelProvenance]
[src: file: frontend/src/components/settings/AgentsSection.tsx:1306-1331#renderSelect]

The settings endpoint replaces the whole model-tier document. Before a save,
the editor rereads it and merges only the selected agent/tier, guarded against
synchronous re-entry. It updates its displayed state after a successful write,
not optimistically. This preserves edits already saved by another card, but is
not a server-side compare-and-swap guarantee against another client writing
between that GET and POST.
[src: file: frontend/src/components/settings/AgentsSection.tsx:200-245#saveTierPreference]

The component regressions cover dynamic IDs, runtime separation, OpenCode,
manual Kiro/Vibe entries, unavailable/cache metadata, empty/error recovery,
explicit clearing, independent Ollama edits, duplicate clicks and write failure.
They mock the API and do not mutate provider accounts or user settings.
[src: file: frontend/src/components/settings/__tests__/AgentsSection.catalog.test.tsx:77-254#Fable]
[src: file: frontend/src/components/settings/__tests__/AgentsSection.catalog.test.tsx:343-355#Kiro]

## Ollama settings use the same catalogue

Ollama's card selects only `agent:ollama` from the saved snapshot; the installed
inventory remains separate input for download/context controls and size hints.
Its three tier controls remain visible offline or without installed models.
Unknown configured IDs and unavailable rows stay visible and disabled; failed
or pending snapshot reads and offline health never turn cached data into live
discovery. Reloading that snapshot or typing a query does not probe Ollama.
[src: file: frontend/src/components/settings/OllamaCard.tsx:457-459#savedTarget]
[src: file: frontend/src/components/settings/OllamaCard.tsx:633-683#pickTierModel]

Tier writes reread the complete settings document, merge only the selected
field, reject synchronous re-entry and replace the displayed value only after
confirmation. This preserves earlier changes made by another editor, not a
cross-client transaction between GET and POST. Initial settings-load errors
are visible and recover through the explicit refresh button. Context saves
and resets also use a synchronous guard, including their follow-up inventory
read; the manual refresh cannot race those writes. Automatic labels no longer
promise an embedded default model: the known configured/catalogue fallback ID
is appended separately.
[src: file: frontend/src/components/settings/OllamaCard.tsx:174-202#refresh]
[src: file: frontend/src/components/settings/OllamaCard.tsx:236-259#pickTierModel]
[src: file: frontend/src/components/settings/OllamaCard.tsx:261-317#saveContextOverride]
[src: file: frontend/src/components/settings/OllamaCard.tsx:637-638#tiersLoadError]
[src: file: frontend/src/components/settings/OllamaCard.tsx:648-652#configuredDefault]
[src: file: frontend/src/components/settings/__tests__/OllamaCard.catalog.test.tsx:105-141#merges]
[src: file: frontend/src/components/settings/__tests__/OllamaCard.catalog.test.tsx:182-194#initial]
[src: file: frontend/src/components/settings/__tests__/OllamaCard.test.tsx:269-288#serializes]
[src: file: frontend/src/components/settings/__tests__/OllamaCard.models.test.ts:73-77#automatic]

These component regressions do not prove the backend discovery/reconciliation
boundary. That remaining work is tracked by KT-634, and the frontend change
must not be treated as completion of the global KT-531 catalogue contract.

## Shared picker identity and provenance

### One-time catalogue bootstrap

Historical model IDs are initial migration data, not normal runtime fallbacks.
On an unbootstrapped database, an old default no longer claims a tier explicitly
configured to another model. Configured Ollama tiers are imported alongside
the CLI tiers. Seed insertion preserves an existing model row and fills only
an unassigned tier in the exact runtime namespace; it does not overwrite a
manual/live row or compete with an unavailable existing assignment.
[src: file: backend/src/core/model_catalog/mod.rs:140-260#migrate_hardcoded_catalog_once]
[src: file: backend/src/core/model_catalog/mod.rs:262-285#seed_configured_tiers]
[src: file: backend/src/db/model_catalog.rs:942-990#insert_migrated_seed_for_target]

Bootstrap records its durable marker in the same database operation. Reopening
the database and invoking bootstrap again leaves rows and timestamps unchanged,
does not restore deleted seeds, and does not import later configuration edits.
This correction does not reset that marker or repair already-bootstrapped
databases. No user database or configuration was changed during qualification.
[src: file: backend/tests/model_catalog_migration.rs:95-141#catalogue_bootstrap_is_durable_and_does_not_restore_removed_seeds]
[src: file: backend/src/core/model_catalog/mod.rs:140-260#mark_migration_done]

Configuration remains unchanged and retains priority at resolution/preflight.
Several explicitly configured tiers can still name the same model even though
one catalogue row has only one tier assignment. The integration tests preserve
that distinction, including an existing historical identity assigned to another
tier and a refusal for the explicitly configured unavailable model.
[src: file: backend/tests/model_catalog_migration.rs:220-271#an_existing_historical_identity_keeps_its_other_tier_and_explicit_configuration]
[src: file: backend/tests/model_catalog_migration.rs:273-332#one_configured_identity_can_still_resolve_multiple_explicit_tiers]
[src: file: backend/src/agents/runner.rs:3388-3460#configured_model_flag]

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
[src: file: frontend/src/lib/constants.ts:60-75#modelForAgentTier]
[src: file: frontend/src/lib/modelCatalogSelection.ts:12-49#resolveCatalogTier]
[src: file: frontend/src/lib/modelCatalogSelection.ts:77-88#catalogTierEntry]
[src: file: frontend/src/components/AgentSwitchPicker.tsx:130-140#resolvedTier]

The shared picker keeps an unavailable configured entry disabled. A failed
snapshot read reports the error and retains previous data as cached, not live.
Saved per-discussion and Quick Prompt overrides are shown for the current
selection; alternative choices resolve their own tiers. The Quick Prompt
caller still explicitly clears its old override when choosing a new agent/tier.
[src: file: frontend/src/lib/modelCatalogSelection.ts:95-146#catalogModelOptions]
[src: file: frontend/src/hooks/useModelCatalogSnapshot.ts:32-93#useModelCatalogSnapshot]
[src: file: frontend/src/lib/modelCatalogSelection.ts:90-93#catalogModelProvenance]
[src: file: frontend/src/components/ChatHeader.tsx:429-430#currentModel]
[src: file: frontend/src/pages/WorkflowsPage.tsx:1606-1637#changeQuickPromptAgent]
[src: file: frontend/src/pages/WorkflowsPage.tsx:3220-3221#currentModel]

KT-627 separately removed the HTTP runner's embedded fallback and qualified
production-library resolution/preflight. The final migration and cross-surface
audit still keeps KT-531 open; a historical test result never qualifies a later
candidate by itself.
[src: file: backend/tests/http_model_resolution.rs:1-4#production]
[src: file: backend/tests/http_model_resolution.rs:91-114#unassigned_http_models_refuse_before_any_provider_request]

### Search and keyboard interaction

The shared agent picker filters agent labels, exact connection identities and
configured/assigned model IDs or aliases. Search ignores case and diacritics
without rewriting the selected identity. Typing never selects a target or
refetches the catalogue. An unknown configured ID remains searchable, and a
known unavailable choice remains disabled; accessible descriptions retain the
model, provenance and unavailable state after filtering.
[src: file: frontend/src/lib/modelCatalogSelection.ts:53-62#matchesCatalogSearch]
[src: file: frontend/src/components/AgentSwitchPicker.tsx:143-149#visibleChoices]
[src: file: frontend/src/components/AgentSwitchPicker.tsx:358-393#tierOptions]

The search field is outside the menu, within a portalled non-modal dialog.
Arrow keys move among enabled choices; Home/End retain their caret behavior in
search and navigate choices when an option has focus. Escape inside the picker
does not propagate to its parent form. Closing with Escape or completing an
asynchronous selection restores trigger focus. Reopening clears the query,
an empty result is explicit, and the popup's height is bounded by the viewport.
[src: file: frontend/src/components/AgentSwitchPicker.tsx:151-176#navigateChoices]
[src: file: frontend/src/components/AgentSwitchPicker.tsx:178-195#updatePopoverPosition]
[src: file: frontend/src/components/AgentSwitchPicker.tsx:230-235#restoreFocusRef]
[src: file: frontend/src/components/AgentSwitchPicker.tsx:299-305#setQuery]
[src: file: frontend/src/components/AgentSwitchPicker.tsx:321-341#createPortal]
[src: file: frontend/src/components/__tests__/AgentSwitchPicker.catalog.test.tsx:168-205#reports]
[src: file: frontend/src/components/__tests__/AgentSwitchPicker.catalog.test.tsx:229-256#closes]
[src: file: frontend/src/components/__tests__/AgentSwitchPicker.accessibility.test.tsx:40-76#repositions]

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
[src: file: frontend/src/lib/modelCatalogSelection.ts:12-49#resolveCatalogTier]
[src: file: frontend/src/hooks/useModelCatalogSnapshot.ts:32-93#useModelCatalogSnapshot]
[src: file: frontend/src/components/MentionTierChoices.tsx:7-15#MentionTierChoices]
[src: file: frontend/src/components/MentionTierChoices.tsx:39-44#lastChecked]
[src: file: frontend/src/components/ChatInput.tsx:510#useModelCatalogSnapshot]
[src: file: frontend/src/components/ChatInput.tsx:734-744#mentionTierResolution]
[src: file: frontend/src/components/NewDiscussionForm.tsx:197#useModelCatalogSnapshot]

Untouched principal insertion preserves its saved model override and does not
write a tier preference. Explicit tier choices resolve their own configuration.
Unavailable tier buttons remain visible and disabled; horizontal keyboard
navigation skips them without wrapping. A disabled tier's pointer event must
not bubble into the new form's implicit default selection. Joined CLI mentions
keep their exact session and stable ordinal, expose no model tier, and do not
change native preferences. New-discussion submissions retain the immutable
connection and explicitly selected tier.
[src: file: frontend/src/lib/mentionTierSelection.ts:6-16#nextMentionTierIndex]
[src: file: frontend/src/components/NewDiscussionForm.tsx:874-880#applyMentionSuggestion]
[src: file: frontend/src/components/NewDiscussionForm.tsx:646-653#initialTargets]
[src: file: frontend/src/components/__tests__/ComposerMentionCatalog.test.tsx:149-157#untouched]
[src: file: frontend/src/components/__tests__/ComposerMentionCatalog.test.tsx:159-170#persists]
[src: file: frontend/src/components/__tests__/ComposerMentionCatalog.test.tsx:216-243#cli_ordinal]

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
[src: file: frontend/src/lib/modelCatalogSelection.ts:53-62#matchesCatalogSearch]
[src: file: frontend/src/components/ChatInput.tsx:745-762#mentionMatchesQuery]
[src: file: frontend/src/components/NewDiscussionForm.tsx:231-234#matchingMentionTargets]
[src: file: frontend/src/lib/mention-autocomplete.ts:7-10#mentionMatchRank]

The unfinished search token accepts Unicode letters and common model-ID
characters (`/`, `.`, `:`). This changes autocomplete only: selection replaces
the original caret range with the canonical trigger. Dispatch parsing is
unchanged, and typing alone never changes preferences or submits an agent.
ArrowDown on an empty result keeps a nonnegative index; when the catalogue
arrives, Enter can select the displayed row rather than indexing `-1` or
throwing. Display and keyboard use the same filtered list.
[src: file: frontend/src/components/__tests__/ComposerMentionCatalog.test.tsx:82-110#keyboard]
[src: file: frontend/src/lib/mention-autocomplete.ts:12-26#findAgentMentionQuery]
[src: file: frontend/src/lib/__tests__/mention-autocomplete.test.ts:20-26#findAgentMentionQuery]

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
[src: file: frontend/src/components/ModelCatalogPicker.tsx:21-71#ModelCatalogPicker]
[src: file: frontend/src/components/workflows/QuickPromptForm.tsx:309-315#ModelCatalogPicker]
[src: file: frontend/src/components/workflows/WorkflowWizard.tsx:4366-4381#ModelCatalogPicker]

Reasoning choices come from the effective model's `reasoning_modes`, not a
fixed low/medium/high list. An existing value absent from that list is retained
and explicitly labelled, not silently rewritten. The QP save path preserves
the edited reasoning value and the existing `max_tokens`; previously it
replaced both with null on every save. Choosing a different agent/tier in the
QP form explicitly clears the old model and reasoning override, while a
catalogue read or failure never changes either value.
[src: file: frontend/src/components/ModelCatalogPicker.tsx:37-42#modes]
[src: file: frontend/src/components/workflows/QuickPromptForm.tsx:174-207#handleSave]
[src: file: frontend/src/components/workflows/QuickPromptForm.tsx:252-258#onTargetSelectionChange]

The regressions exercise the actual form payload, explicit unknown Unicode IDs,
reload failures, namespace switching, disabled entries and per-model reasoning.
They use API fixtures only; these tests are not provider discovery or a browser
layout qualification. The final cross-surface inventory and migration proof
still require the remaining KT-531 audit. Discussion target persistence
is covered separately below.
[src: file: frontend/src/components/workflows/__tests__/QuickPromptForm.catalog.test.tsx:39-87#preserves]
[src: file: frontend/src/components/__tests__/ModelCatalogPicker.test.tsx:25-66#switches]

## Explicit target changes are not catalogue refreshes

Choosing another target or tier in a QP/workflow clears the old explicit model
and model-specific reasoning mode, while preserving unrelated settings such as
`max_tokens`. The payload pins the exact selected connection, or explicitly
clears it for a connectionless agent. A change between two connections of the
same agent family and tier is still a real target change, not a no-op. Merely
opening, editing or rereading the catalogue preserves the saved overrides.
[src: file: frontend/src/lib/agentSelection.ts:3-16#agentSettingsForSelection]
[src: file: frontend/src/components/workflows/WorkflowWizard.tsx:297-326#renderAgentTierPicker]
[src: file: frontend/src/pages/WorkflowsPage.tsx:1153-1185#changeStepAgent]
[src: file: frontend/src/pages/WorkflowsPage.tsx:1606-1637#changeQuickPromptAgent]

The creation wizard, full editor, inline inspector, pipeline and QP card receive
the configured named targets. Their selectors display the saved explicit model
and pass the selected immutable connection instead of carrying an old one via
an object spread. Component tests inspect the actual update payloads and exact
connection callback; these are not live workflow/provider executions.
[src: file: frontend/src/components/workflows/WorkflowDetail.tsx:1344-1386#StepAgentSwitcher]
[src: file: frontend/src/components/workflows/__tests__/WorkflowWizard.test.tsx:780-830#explicit]
[src: file: frontend/src/components/workflows/__tests__/WorkflowDetail.steps.test.tsx:475-517#onChangeStepAgent]

## Workflow compensation targets

The workflow's `on_failure` Agent steps use the same agent/tier picker and
selection callback contract as its ordinary steps. An explicit connection,
agent or tier change clears the old model and reasoning override while retaining
`max_tokens`; saving without changing the target preserves the entire rollback
configuration. The callback updates only the compensation step, not the main
pipeline. Search, exact connection namespaces, provenance and unavailable tiers
therefore share the existing picker behavior.
[src: file: frontend/src/components/workflows/WorkflowWizard.tsx:297-326#renderAgentTierPicker]
[src: file: frontend/src/components/workflows/WorkflowWizard.tsx:4802-4805#updateRb]
[src: file: frontend/src/components/workflows/WorkflowWizard.tsx:4881-4885#renderAgentTierPicker]
[src: file: frontend/src/components/workflows/__tests__/WorkflowWizard.coverage.test.tsx:984-1048#recover]

Five picker controls failed against the previous compensation form; the
unchanged-target control passed. The corrected two wizard suites pass 108 tests.
These are form/callback tests, not actual compensation or provider executions.

## Discussion target persistence

Discussion PATCH has no explicit model-override field. Its backend setters now
clear the old override only when the persisted agent, tier or connection
actually changes. Each setter updates its field and clears the model in the
same SQL statement. Resending an unchanged selection preserves its override;
ordinary title edits and historical message models remain untouched.
[src: file: backend/src/db/discussions.rs:1093-1101#update_discussion_tier]
[src: file: backend/src/db/discussions.rs:1141-1172#update_discussion_agent]
[src: file: backend/src/db/discussions.rs:1187-1202#update_discussion_connection]

An explicit JSON `connection_id: null` is distinct from an absent field.
Clearing a connection is validated against the effective agent before any
requested field is written: an explicit clear is refused for Custom.
Resending the same agent no longer clears an existing connection or invalidates
the summary cache. A lightweight agent lookup avoids loading the transcript
merely to compare targets.
[src: file: backend/src/models/discussions.rs:609-618#connection_id]
[src: file: backend/src/api/discussions/crud.rs:787-817#validate_connection_target]
[src: file: backend/src/api/discussions/crud.rs:859-883#invalidate_summary_cache]
[src: file: backend/src/db/discussions.rs:1082-1091#get_discussion_agent]

Seven isolated Router/SQLite regressions cover changed and unchanged targets,
null versus absent, invalid-target refusal and historical answer preservation.
Six failed on the unchanged backend; the control for ordinary edits passed.
These tests never launch an agent or contact a provider and do not qualify the
separate HTTP fallback or Important-message authority work.
[src: file: backend/tests/discussion_target_model.rs:12-63#fixture]
[src: file: backend/tests/discussion_target_model.rs:98-219#changing_agent_clears_the_previous_explicit_model]

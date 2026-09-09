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
[src: file: frontend/src/lib/modelCatalogSelection.ts:33-70]
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

An explicit configured model wins over a different catalogue tier assignment.
An unknown explicit ID remains visible, never silently replaced by a known
assignment. A named HTTP connection cannot inherit another connection's
agent-family settings; even rows inside its view must match the exact runtime
namespace. HTTP tiers may fall back to their own configured default, unlike
CLI tiers. No embedded model list participates in the display helper anymore.
[src: file: frontend/src/lib/constants.ts:60-75]
[src: file: frontend/src/lib/modelCatalogSelection.ts:15-31]
[src: file: frontend/src/components/AgentSwitchPicker.tsx:113-142]

The shared picker keeps an unavailable configured entry disabled. A failed
snapshot read reports the error and retains previous data as cached, not live.
Saved per-discussion and Quick Prompt overrides are shown for the current
selection; alternative choices resolve their own tiers. The Quick Prompt
caller still explicitly clears its old override when choosing a new agent/tier.
[src: file: frontend/src/components/AgentSwitchPicker.tsx:189-210]
[src: file: frontend/src/components/ChatHeader.tsx:418-428]
[src: file: frontend/src/pages/WorkflowsPage.tsx:1223-1244]
[src: file: frontend/src/pages/WorkflowsPage.tsx:2850-2857]

This checkpoint does not close KT-531: remaining custom selector/display paths
still need the same contract, and the HTTP runner retains a legacy embedded
fallback. Neither a unit-test migration fallback nor historical model metadata
is evidence that the production runtime is free of embedded defaults.

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
layout qualification. Other custom launch selectors and the discussion's
persisted override on target changes still require the remaining KT-531 audit.
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

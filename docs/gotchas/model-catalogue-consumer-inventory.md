# Model catalogue consumer inventory — KT-531

This September 13 audit covers the local 0.13.0 candidate based on `71f29651`.
It is a source/contract inventory, not evidence that every connected provider
has been exercised. See the [qualification ledger](../releases/0.13.0-checklist.md)
for exact executable trees, test results and remaining release gates.

## Migration is not a runtime fallback

Startup calls `migrate_hardcoded_catalog_once`. Its durable marker is checked
again inside the serialized database write; historical seeds and configured
overrides are imported before marking completion. Subsequent starts rebuild
the runtime cache from durable rows, without reseeding deleted models. The
`migrated_default` helper is test-only, not a production fallback.
[src: file: backend/src/main.rs:246]
[src: file: backend/src/core/model_catalog/mod.rs:55-110]
[src: file: backend/src/core/model_catalog/mod.rs:111-219]

Normal resolution prefers explicit configuration and then available durable
tier assignments. HTTP agents may use their explicit/durable default tier;
they do not revive the old embedded model list. Frontend display fallback is
also configuration/caller supplied, rather than a second embedded catalogue.
[src: file: backend/src/agents/runner.rs:2670-2720]
[src: file: frontend/src/lib/constants.ts:62-76]

The production-library migration tests cover file-backed reopen, deletion
without reseeding, unchanged configuration, Ollama overrides, occupied manual
tiers, conflicting historical identities and one explicit model in multiple
tiers. This does not reset existing migration markers or promise to repair
previously bootstrapped user databases.
[src: file: backend/tests/model_catalog_migration.rs:12]
[src: file: backend/tests/model_catalog_migration.rs:41]
[src: file: backend/tests/model_catalog_migration.rs:89]
[src: file: backend/tests/model_catalog_migration.rs:124]
[src: file: backend/tests/model_catalog_migration.rs:161]
[src: file: backend/tests/model_catalog_migration.rs:210]

## Shared consumers

Runtime identity is exact (`agent:*` or `http:<connection-id>`), never a display
label. Shared option construction preserves unknown configured IDs visibly,
disables unavailable choices, and displays provenance/check time/diagnostics.
An expired or failed live snapshot renders as cached without changing the
stored model's availability or operator metadata. Opening a mention/picker
reads a snapshot once; typing filters locally, without provider discovery.
[src: file: frontend/src/lib/modelCatalogSelection.ts:11-113]
[src: file: frontend/src/hooks/useModelCatalogSnapshot.ts:6-27]

| Surface | Shared path and source |
|---|---|
| Discussion creation, header and project default | `AgentSwitchPicker`. [src: file: frontend/src/components/NewDiscussionForm.tsx:1086] [src: file: frontend/src/components/ChatHeader.tsx:403] [src: file: frontend/src/components/ProjectCard.tsx:1341] |
| Chat and creation mentions | Shared catalogue resolver/search and `MentionTierChoices`; keyboard traversal skips unavailable tiers while preserving exact joined-CLI identity. [src: file: frontend/src/components/ChatInput.tsx:668] [src: file: frontend/src/components/NewDiscussionForm.tsx:175] [src: file: frontend/src/lib/mentionTierSelection.ts:6] |
| QP and workflow authoring, including compensation and review | `AgentSwitchPicker` plus `ModelCatalogPicker`/`SearchableSelect` where a model or reasoning override is editable. [src: file: frontend/src/components/workflows/QuickPromptForm.tsx:243] [src: file: frontend/src/components/workflows/QuickPromptForm.tsx:309] [src: file: frontend/src/components/workflows/WorkflowWizard.tsx:263] [src: file: frontend/src/components/workflows/WorkflowWizard.tsx:4131] [src: file: frontend/src/components/workflows/WorkflowWizard.tsx:4195] [src: file: frontend/src/components/workflows/WorkflowDetail.tsx:1311] [src: file: frontend/src/pages/WorkflowsPage.tsx:2850] [src: file: frontend/src/pages/WorkflowsPage.tsx:3203] |
| Planning launch/reassignment | The same agent and model controls, without clearing unrelated execution policy/history. [src: file: frontend/src/components/TaskLaunchDialog.tsx:215] [src: file: frontend/src/components/DiscussionPlanPanel.tsx:1104] |
| Comparison rerun/reviewer | Shared target picker for new selections; historical identity stays recorded/attested/unknown rather than being replaced by current settings. [src: file: frontend/src/components/BatchCompareDetailsPanel.tsx:343] [src: file: frontend/src/components/BatchCompareDetailsPanel.tsx:412] See [comparison identity](compare-model-identity.md). |
| Agent and Ollama settings | Shared `catalogModelOptions`, exact runtime target and searchable model choices. [src: file: frontend/src/components/settings/AgentsSection.tsx:1218] [src: file: frontend/src/components/settings/OllamaCard.tsx:481] See [tier settings](agent-tier-catalogue-settings.md). |
| HTTP connection draft | Searchable choices come from that draft's test result; missing/untested saved IDs remain visible but unverified. A changed endpoint cannot borrow another saved target's verified catalogue. [src: file: frontend/src/components/settings/ExternalApiSection.tsx:342] See [HTTP probe isolation](http-probe-catalogue-isolation.md). |
| Manual catalogue management | Searchable runtime target, model/ID/source filtering and shared derived provenance. Failed refresh/reload retains identities as cached; manual provenance is preserved. [src: file: frontend/src/components/settings/ModelCatalogSection.tsx:68] [src: file: frontend/src/components/settings/ModelCatalogSection.tsx:111] |

`ModelCatalogPicker` additionally preserves an unrecognized saved reasoning
mode and labels explicitly typed model IDs as outside the catalogue. An empty
override means the configured/durable default, not an invented model.
[src: file: frontend/src/components/ModelCatalogPicker.tsx:23-71]

The management table's create/update/delete/refresh actions share a synchronous
guard through snapshot reload; failure releases it for an explicit retry.
Four provenance/refresh regression cases and four batched-double-click cases
cover this final table correction, including preserved availability and manual
neighbours. A subsequent assertion also exposed the delete button's missing
busy state, now aligned with the other mutation controls.
[src: file: frontend/src/components/settings/ModelCatalogSection.tsx:58]
[src: file: frontend/src/components/settings/ModelCatalogSection.tsx:198]
[src: file: frontend/src/components/settings/__tests__/ModelCatalogSection.test.tsx:101]

## Deliberate boundaries

Ollama download suggestions are installation recommendations, not runtime tier
fallbacks. The OpenRouter GLM preset selects only a model actually returned by
its test and preserves an existing choice; it is not an unconditional runtime
fallback. Neither is removed by this inventory.
[src: file: frontend/src/components/settings/OllamaCard.tsx:417]
[src: file: frontend/src/components/settings/ExternalApiSection.tsx:583]

This closes the catalogue migration/consumer audit only after its final gates
pass and the exact change is integrated. It does not qualify KT-619/635,
whole-app browser coverage, a real provider canary, or release publication.

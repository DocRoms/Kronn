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
[src: file: backend/src/main.rs:285-289#migrate_hardcoded_catalog_once]
[src: file: backend/src/core/model_catalog/mod.rs:107-110#migrated_default]
[src: file: backend/src/core/model_catalog/mod.rs:112-138#refresh_runtime_cache]
[src: file: backend/src/core/model_catalog/mod.rs:140-260#migrate_hardcoded_catalog_once]

Normal resolution prefers explicit configuration and then available durable
tier assignments. HTTP agents may use their explicit/durable default tier;
they do not revive the old embedded model list. Frontend display fallback is
also configuration/caller supplied, rather than a second embedded catalogue.
[src: file: backend/src/agents/runner.rs:3388-3439#configured_model_flag]
[src: file: backend/src/agents/runner.rs:3441-3460#resolve_model_flag]
[src: file: frontend/src/lib/constants.ts:60-75#modelForAgentTier]

The production-library migration tests cover file-backed reopen, deletion
without reseeding, unchanged configuration, Ollama overrides, occupied manual
tiers, conflicting historical identities and one explicit model in multiple
tiers. This does not reset existing migration markers or promise to repair
previously bootstrapped user databases.
[src: file: backend/tests/model_catalog_migration.rs:66-93#ollama_bootstrap_preserves_the_configured_default_instead_of_its_old_seed]
[src: file: backend/tests/model_catalog_migration.rs:95-141#catalogue_bootstrap_is_durable_and_does_not_restore_removed_seeds]
[src: file: backend/tests/model_catalog_migration.rs:143-176#migrated_operator_tier_replaces_the_historical_assignment]
[src: file: backend/tests/model_catalog_migration.rs:178-218#historical_bootstrap_does_not_compete_with_an_existing_manual_tier]
[src: file: backend/tests/model_catalog_migration.rs:220-271#an_existing_historical_identity_keeps_its_other_tier_and_explicit_configuration]
[src: file: backend/tests/model_catalog_migration.rs:273-332#one_configured_identity_can_still_resolve_multiple_explicit_tiers]

## Shared consumers

Runtime identity is exact (`agent:*` or `http:<connection-id>`), never a display
label. Shared option construction preserves unknown configured IDs visibly,
disables unavailable choices, and displays provenance/check time/diagnostics.
An expired or failed live snapshot renders as cached without changing the
stored model's availability or operator metadata. Opening a mention/picker
reads a snapshot once; typing filters locally, without provider discovery.
[src: file: frontend/src/lib/modelCatalogSelection.ts:64-75#AGENT_RUNTIME_TARGETS]
[src: file: frontend/src/lib/modelCatalogSelection.ts:90-93#catalogModelProvenance]
[src: file: frontend/src/lib/modelCatalogSelection.ts:95-146#catalogModelOptions]
[src: file: frontend/src/hooks/useModelCatalogSnapshot.ts:32-93#useModelCatalogSnapshot]

| Surface | Shared path and source |
|---|---|
| Discussion creation and header | `AgentSwitchPicker`. [src: file: frontend/src/components/NewDiscussionForm.tsx:1234#AgentSwitchPicker] [src: file: frontend/src/components/ChatHeader.tsx:412#AgentSwitchPicker] The project audit selector is not on this path: its tier buttons show the model configured in settings (or on the named connection), read without the catalogue. [src: file: frontend/src/components/AuditTimeline.tsx:446-451#tierModel] |
| Chat and creation mentions | Shared catalogue resolver/search and `MentionTierChoices`; keyboard traversal skips unavailable tiers while preserving exact joined-CLI identity. [src: file: frontend/src/components/ChatInput.tsx:734-744#mentionTierResolution] [src: file: frontend/src/components/NewDiscussionForm.tsx:198-199#targetTierResolution] [src: file: frontend/src/lib/mentionTierSelection.ts:6-16#nextMentionTierIndex] |
| QP and workflow authoring, including compensation and review | `AgentSwitchPicker` plus `ModelCatalogPicker`/`SearchableSelect` where a model or reasoning override is editable. [src: file: frontend/src/components/workflows/QuickPromptForm.tsx:244#AgentSwitchPicker] [src: file: frontend/src/components/workflows/QuickPromptForm.tsx:310#ModelCatalogPicker] [src: file: frontend/src/components/workflows/WorkflowWizard.tsx:298#AgentSwitchPicker] [src: file: frontend/src/components/workflows/WorkflowWizard.tsx:4366#ModelCatalogPicker] [src: file: frontend/src/components/workflows/WorkflowWizard.tsx:4430#AgentSwitchPicker] [src: file: frontend/src/components/workflows/WorkflowDetail.tsx:1365#AgentSwitchPicker] [src: file: frontend/src/pages/WorkflowsPage.tsx:3221#AgentSwitchPicker] [src: file: frontend/src/components/AgentCompareSelector.tsx:85#AgentSwitchPicker] |
| Planning launch/reassignment | The same agent and model controls, without clearing unrelated execution policy/history. [src: file: frontend/src/components/TaskLaunchDialog.tsx:214-231#AgentSwitchPicker] [src: file: frontend/src/components/DiscussionPlanPanel.tsx:1142-1159#AgentSwitchPicker] |
| Comparison rerun/reviewer | Shared target picker for new selections; historical identity stays recorded/attested/unknown rather than being replaced by current settings. [src: file: frontend/src/components/BatchCompareDetailsPanel.tsx:343#AgentSwitchPicker] [src: file: frontend/src/components/BatchCompareDetailsPanel.tsx:412#AgentSwitchPicker] See [comparison identity](compare-model-identity.md). |
| Agent and Ollama settings | Shared `catalogModelOptions`, exact runtime target and searchable model choices. [src: file: frontend/src/components/settings/AgentsSection.tsx:1309#catalogModelOptions] [src: file: frontend/src/components/settings/OllamaCard.tsx:653#catalogModelOptions] See [tier settings](agent-tier-catalogue-settings.md). |
| HTTP connection draft | Searchable choices come from that draft's test result; missing/untested saved IDs remain visible but unverified. A changed endpoint cannot borrow another saved target's verified catalogue. [src: file: frontend/src/components/settings/ExternalApiSection.tsx:229-237#catalogModels] [src: file: frontend/src/components/settings/ExternalApiSection.tsx:403-413#catalogModels] [src: file: frontend/src/components/settings/ExternalApiSection.tsx:650-656#changeConnection] See [HTTP probe isolation](http-probe-catalogue-isolation.md). |
| Manual catalogue management | Searchable runtime target, model/ID/source filtering and shared derived provenance. Failed refresh/reload retains identities as cached; manual provenance is preserved. [src: file: frontend/src/components/settings/ModelCatalogSection.tsx:104-114#visibleRows] [src: file: frontend/src/components/settings/ModelCatalogSection.tsx:272-289#SearchableSelect] [src: file: frontend/src/components/settings/ModelCatalogSection.tsx:92-102#rows] |

`ModelCatalogPicker` additionally preserves an unrecognized saved reasoning
mode and labels explicitly typed model IDs as outside the catalogue. An empty
override means the configured/durable default, not an invented model.
[src: file: frontend/src/components/ModelCatalogPicker.tsx:21-71#ModelCatalogPicker]

The management table's create/update/delete/refresh actions share a synchronous
guard through snapshot reload; failure releases it for an explicit retry.
Four provenance/refresh regression cases and four batched-double-click cases
cover this final table correction, including preserved availability and manual
neighbours. A subsequent assertion also exposed the delete button's missing
busy state, now aligned with the other mutation controls.
[src: file: frontend/src/components/settings/ModelCatalogSection.tsx:65#mutationPending]
[src: file: frontend/src/components/settings/ModelCatalogSection.tsx:182-211#save]
[src: file: frontend/src/components/settings/ModelCatalogSection.tsx:212-229#remove]
[src: file: frontend/src/components/settings/ModelCatalogSection.tsx:366-375#mutationPending]
[src: file: frontend/src/components/settings/ModelCatalogSection.tsx:472#remove]
[src: file: frontend/src/components/settings/__tests__/ModelCatalogSection.test.tsx:153-186#serializes]
[src: file: frontend/src/components/settings/__tests__/ModelCatalogSection.test.tsx:188-222#cached]

## Deliberate boundaries

Ollama download suggestions are installation recommendations, not runtime tier
fallbacks. The OpenRouter GLM preset selects only a model actually returned by
its test and preserves an existing choice; it is not an unconditional runtime
fallback. Neither is removed by this inventory.
[src: file: frontend/src/components/settings/OllamaCard.tsx:568-598#suggestions]
[src: file: frontend/src/components/settings/ExternalApiSection.tsx:682-691#open_router]

This closes the catalogue migration/consumer audit only after its final gates
pass and the exact change is integrated. It does not qualify KT-619/635,
whole-app browser coverage, a real provider canary, or release publication.

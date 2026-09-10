# Catalogue and reviewer selection

The workflow review picker keeps `reviewer_agent` and its nullable
`reviewer_tier` as the existing review configuration. Enabling review leaves
the tier `null`; the tier changes only when the operator selects a picker
entry. [src: file: frontend/src/components/workflows/WorkflowWizard.tsx:4175-4217]

`AgentSwitchPicker` accepts a nullable current tier so an existing saved review
can be displayed without forcing a tier selection. Its callbacks remain the
only path that reports a selection to its caller. [src: file: frontend/src/components/AgentSwitchPicker.tsx:28-39]

The manual catalogue form keys a target by `runtime_target_id` and updates its
matching `agent_type` only in the searchable selector callback. During edit,
the picker is disabled and a saved target absent from the current snapshot is
kept as an option, preserving its runtime namespace. [src: file: frontend/src/components/settings/ModelCatalogSection.tsx:165-180] [src: file: frontend/src/components/settings/ModelCatalogSection.tsx:242-258]

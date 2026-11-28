# Step shadows in the overview and the editor (KT-1696)

The shadows of an Agent step and the merged result they feed are shown and
edited as sub-rows under the step's agent, in the workflow overview
(`frontend/src/components/workflows/WorkflowDetail.tsx`) and in the editor
(`frontend/src/components/workflows/WorkflowWizard.tsx`). One component serves
both: `frontend/src/components/workflows/StepShadowRows.tsx`.

## How a change is written

Every edit goes through `frontend/src/lib/shadowMerge.ts` and returns the new
shadows with the merge to write. The merge is **always an explicit
`shadow_merge.shadows` list**:

- `enabled` is true iff the list is not empty;
- an empty list is never written with `enabled = true` (the backend reads that
  as « every shadow »);
- with no shadow merged the merge is switched off and keeps its other settings
  (`agent`, `timeout_secs`);
- turning Merge on for a paused shadow also resumes it, and a shadow that merges
  cannot be paused (`canPauseShadow`);
- a rename is followed by the list, a removal drops out of it, and removing every
  shadow removes the merge.

A legacy merge (`enabled`, empty list = all) is read as all the shadows and
becomes explicit on its first edit.

## Saving from the overview

`WorkflowsPage.changeStepShadows` shows the change first, sends the whole step
list with `workflows.update`, and on failure puts the step back and toasts the
reason (`wf.shadow.rows.saveFailed`), like the main agent picker. Without
`onChangeStepShadows` the rows are read-only (sub-workflow overview, tests).

## Where the pieces are

| Piece | File |
|---|---|
| Sub-rows, toggles, summary line, `?` popover | `frontend/src/components/workflows/StepShadowRows.tsx` |
| Agent/model menu (the shared picker's « exact model » mode, KT-1885), transport badge | `exactModel` and `TransportBadge` in `frontend/src/components/AgentSwitchPicker.tsx` |
| Branch map lines (`⑂`, `⊕`, footer) | `frontend/src/components/workflows/StepBranchMap.tsx` |
| Header button label | `ShadowStatusButton` in `frontend/src/components/workflows/StepShadowsEditor.tsx` |
| Runs list tag (`⑂ N · ⊕ ok / partielle`) | `RunShadowTag` in `frontend/src/components/workflows/WorkflowDetail.tsx` |

The detailed settings (prompt, versions, projects, limits, the merge's timeout)
stay in `StepShadowsEditor`, under « Advanced ».

## Gotchas

- The runs list tag reads `workflows.stepShadows` per run (one request per
  visible run), only for a workflow that carries shadows today.
- `StepShadowRows.css` holds the styles of the rows, the menu, the branch-map
  lines and the runs tag; violet is a shadow, orange its merge (`--kr-purple`,
  `--kr-warning-amber`).

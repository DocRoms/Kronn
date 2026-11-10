# Heavy step outputs (KT-1525)

A workflow step whose output grows unnoticed slows every run, the run storage
and the Page that receives the data (a collect step once left 1.1 MB per hourly
run: full Jira issues where the Page used a few fields). Kronn records sizes and
warns; it never refuses a run.

## What is recorded

- `StepResult.output_bytes`: UTF-8 bytes of the step's `output`, stamped by
  `apply_step_snapshot` when the step completes. Absent on a run recorded before
  KT-1525; the run view then measures the stored output.
  `[src: file: backend/src/workflows/runner.rs:4975]`
- A CollectApiData step adds `output_bytes` per source to its envelope
  (`data.meta.sources[]`), 0 for a failed source, so the heavy source is
  obvious (Quick API and Quick Exec sources alike).
  `[src: file: backend/src/workflows/collect_api_data_step.rs:586-640]`
- `workflow_runs.heaviest_output_bytes` / `heaviest_output_step` (migration 290):
  the run's heaviest step output and its step's name, written on insert and on
  every progress write, and read by the workflow list from the last-run summary.
  Both summary indexes (`idx_workflow_runs_summary`,
  `idx_workflow_runs_project_summary`) include them so the list still never reads a
  run's payload. A run recorded before migration 290 reports 0 and no step.
  `[src: file: backend/src/db/sql/290_workflow_runs_heaviest_output.sql:1-15]`

## The threshold

`WorkersConfig.heavy_output_kb` (Settings > Workers, default 256, 1 to 102 400):
a step output strictly above it is « heavy ». The UI grades; the backend only
records sizes. `[src: file: backend/src/models/setup.rs:129-134]`

## Where the warning shows

- Run view: a « Heavy output » chip on the step row, and in the expanded step the
  step name, its size, the fix (select fields, limit, paginate) and, for a
  CollectApiData step, each source's payload (heaviest first). Every expanded
  step shows its output size.
- Workflow list: the card of a workflow whose last run is heavy names the step.
- Pages: see « HTML weight » in [live-pages](../architecture/live-pages.md); the
  dataset weight is graded there.

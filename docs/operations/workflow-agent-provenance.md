# Workflow agent provenance

New Agent step results carry an optional `agent_provenance` object with an
ordered list of launches and the one-based `selected_attempt` whose output was
retained. Initial execution, repair, escalation, debate review and debate author
are separate roles. HTTP tool turns and structured-format negotiation remain
inside the same Agent launch. Failed launches remain visible; a failed step
without retained agent output has no selected attempt.

Each attempt captures its agent, tier, saved connection id, explicit model
override, model resolved at launch, structured model observations, format
fallback, timestamp, duration and launch outcome. Endpoints, credentials,
prompts and tool payloads are not part of this object. Successful execution
does not imply a valid schema or selection: an invalid repair can complete
successfully while the initial output remains selected.

`requested_model` is the explicit override. `resolved_model` is the launch
selection after tier/connection resolution. Neither proves what a proxy served.
`observed_models` contains distinct identifiers from HTTP `model` fields or
Claude assistant/message-start metadata. Missing observations stay empty;
generated prose and CLI initialization configuration do not count as evidence.
Observation identifiers are bounded to 256 bytes and 16 distinct values per
launch. Native ACP can report `model_applied: false` when no matching session
option exists and it retains its own default. That default stays unknown
unless the runtime reports it.

When provenance exists, the compact `step_agent` and `step_model` are derived
from the selected attempt, never overwritten with the original step's agent
after repair or escalation. Observed model identifiers take precedence over
the resolved request; the tier suffix remains part of the compact label.
When no output was retained, these compact fields identify the last attempted
launch so a transport failure remains diagnosable; `selected_attempt` stays
null. An empty attempt list leaves both compact fields unknown.
An invalid repair or rejected debate output preserves the earlier selection.
Historical rows without provenance keep their stored metadata and remain
without an attempt history; changing configuration does not reconstruct it.

This records execution metadata on the completed step row. It is not an
independent transaction journal of every in-flight provider request; abrupt
process termination before the step result is saved may leave no provenance.
The run detail shows each attempt and marks the retained one.

## In templates and Page publication

Later steps read the provenance of an earlier Agent step under
`steps.<name>.provenance` (and `previous_step.provenance`), the same way they
read `steps.<name>.data`:

| Path | Value |
|---|---|
| `.agent` | agent of the attempt, e.g. `LiteLlm` |
| `.model` | observed model, else the resolved one; `null` when a native ACP runtime kept its unknown default |
| `.connection_id` | saved connection, or `null` |
| `.role` | `Initial`, `Repair`, `Escalation`, `Review` or `Author` |
| `.retained` | `true` for the attempt whose output was kept; `false` when no output was kept and this is the last launch |
| `.format_fallback` | whether that attempt ran without constrained output |
| `.attempts` | number of launches recorded for the step |

`PublishPageData` can write the whole object with
`value_from: "steps.advise.provenance"`, so a Page attributes its analysis to
the agent that produced it instead of hard-coding a provider in its HTML. A
step with no recorded provenance (older runs, non-Agent steps) exposes nothing:
`render_strict` fails on these paths rather than inventing an author from the
current configuration.

[src: file: backend/src/models/workflows.rs]
[src: file: backend/src/workflows/steps.rs]
[src: file: backend/src/workflows/runner.rs]
[src: file: backend/src/agents/provenance.rs]
[src: file: backend/src/agents/runner.rs]
[src: file: backend/src/workflows/template.rs]

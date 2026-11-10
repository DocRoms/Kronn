# Test a new version without risk: shadow runs

You changed a skill or a Quick Prompt and you think the new version is better.
A shadow run lets you check that on real work before your normal runs use it:
each normal run of the workflow also starts a shadow run, with the same inputs
and the version you are testing, and every step that acts outside Kronn is
skipped in it. The run list then compares the two runs, step by step. When the
new version wins, one click hands it to the normal runs.

## What a shadow run is

A shadow run is a copy of a normal run: same workflow, same trigger context
and launch variables, its own id, and a link to the normal run it shadows.
[src: file: backend/src/workflows/shadow.rs:59-89#shadow_of]
It starts in the background once the normal run is pinned, and never blocks or
fails it: a shadow that cannot start is only logged.
[src: file: backend/src/workflows/shadow.rs:91-116#spawn_if_wanted]

A normal run gets a shadow when the workflow's shadow is on, on its first
execution only (a resumed run does not start another one), and when the run's
project is one the shadow covers; an empty project list covers every run.
[src: file: backend/src/workflows/shadow.rs:47-58#wants_shadow]

Two things differ in the shadow run:

- **the versions it loads**: each skill named in the shadow is swapped by its
  base id, and each Quick Prompt step loads the chosen version's template and
  bindings;
  [src: file: backend/src/workflows/shadow.rs:136-239#apply_versions]
- **its worktree**: the normal run may hold the project's checkout, so the
  shadow runs in its own worktree unless the workflow never writes the
  checkout.
  [src: file: backend/src/workflows/shadow.rs:118-134#isolate]

Steps and templates can tell the two apart: `{{run.variant}}` is `normal` or
`shadow`. Sub-workflows started by a shadow run are shadow runs too.

## Pin the versions your normal runs load

A comparison means something only if the normal run loads a known version.

- **Skills.** On an Agent step, each custom skill loads its latest version, or
  a pinned one written `custom-x@N`. Only custom skills have versions; built-in
  and repository skills have none.
  [src: file: backend/src/core/skill_versions.rs:149-168#split_pin]
- **Quick Prompts.** An Agent step linked to a Quick Prompt has a version
  picker next to it (`quick_prompt_version`); unset, the step follows the
  latest version.
  [src: file: backend/src/models/workflows.rs:712-715#quick_prompt_version]

## Turn on the shadow of a workflow

In the workflow editor, open **Shadow runs**:

1. tick **Start a shadow run with each normal run**;
2. keep every project, or pick the projects whose runs get a shadow;
3. under **Versions compared**, choose the version the shadow run tests for
   each skill and Quick Prompt the steps load.

This is stored as the workflow's `shadow`: `enabled`, `project_ids`, `skills`
(a skill id → its replacement, such as `custom-x@3`) and
`quick_prompt_versions` (a Quick Prompt id → a version number).
[src: file: backend/src/models/workflows.rs:170-190#WorkflowShadow]
Only a person sets it: a shadow starts extra runs, so an agent's save that
sets or changes it is refused.
[src: file: backend/src/api/workflows.rs:2750-2752#AGENT_SHADOW_REFUSAL]

A shadow run costs as much as a normal one: its agent steps are paid twice.

## What the shadow run skips

Mark every step that posts, sends or writes outside Kronn (a message, a
ticket, an API call) with **Acts outside Kronn**, under the step's advanced
settings. That is the step's `external_effect` flag.
[src: file: backend/src/models/workflows.rs:628-631#external_effect]

The shadow run skips those steps and every Gate, since a trial run must never
wait for a person.
[src: file: backend/src/workflows/runner.rs:2507-2513#skip_in_shadow]
A skipped step records a success with `[SIGNAL: SKIPPED_SHADOW]` and the run
goes on with the next step.
[src: file: backend/src/workflows/shadow.rs:262-294#skipped]

Two cautions, both shown in the Shadow runs card:

- a step not marked **Acts outside Kronn** runs in the shadow too, including
  what it publishes or sends;
- both runs start at the same time, so a step writing to a fixed path outside
  the run's worktree writes to the same place twice: add `{{run.variant}}` to
  that path.

## Read the comparison

In the run list, a run that has a shadow shows three views: **Normal run**,
**Comparison** and **Shadow run**. The comparison goes step by step: a verdict
per step (identical, different, skipped in shadow, missing on one side), each
side's status, duration and tokens, and a line diff where the outputs differ.
**Differences only** hides the identical steps. While the shadow run is still
going, the comparison says it is partial.

## Promote the better version

A Page shows, in its **Quick Prompts and skills** panel, every Quick Prompt and
skill used by the workflows behind it, with the version each lane loads: the
normal run's and the one the shadow tests.
[src: file: backend/src/db/live_pages.rs:2051-2197#list_live_page_resources]
**Promote** hands the tested version to the normal runs in one click:

- a skill: every step of that workflow that loads it now loads the tested
  version, pinned;
- a Quick Prompt: the workflow's steps that load it pin the tested version; the
  Quick Prompt itself is unchanged, so other workflows keep theirs.

Either way the shadow stops testing it. Promotion is a person's decision too:
the route has no agent access.
[src: file: backend/src/workflows/shadow.rs:307-361#promote]
[src: file: backend/src/api/workflows.rs:2727-2748#promote_shadow]

## Current limits

- **One shadow lane.** A workflow has one shadow, which changes only skill and
  Quick Prompt versions: the agent, the model and the prompt text stay the
  normal run's.
- Several lanes per run, each with its own agent, model, prompt and skills,
  are planned in KT-1459.

See also: [the Kronn way](the-kronn-way.md) and the
[glossary](../glossary.md) entries *Shadow run* and *Page resources*.

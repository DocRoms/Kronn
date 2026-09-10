# Orchestration catalogue selection

Task launch and execution reassignment use `AgentSwitchPicker` for an explicit
agent-target choice and `ModelCatalogPicker` for the saved model catalogue.
The model override is cleared only by a successful explicit target change;
opening, searching, or a failed catalogue read leaves the stored value alone.
An override absent from the saved catalogue remains visible as unverified so it
can be preserved rather than silently replaced. [src: file: frontend/src/components/TaskLaunchDialog.tsx:139-145]
[src: file: frontend/src/components/DiscussionPlanPanel.tsx:387-392]

The launch dialog must let picker-owned Escape handling finish first: the model
picker prevents the event, while the agent picker’s portal is recognised before
the dialog’s window-level close handler runs. [src: file: frontend/src/components/TaskLaunchDialog.tsx:96-104]

The regression also unmounts the launch dialog and checks that focus actually
returns to its original trigger; the test title alone is not proof of focus
restoration. [src: file: frontend/src/components/__tests__/TaskLaunchDialog.test.tsx:154-171]

## Principal qualification

The first worker delivery passed 28 focused tests but still let model-picker
Escape close the dialog and nested labels around the shared components.
The second delivery `f26965f8` recorded two failing regressions before fixing
those defects, and the principal independently replayed its 35 tests.
Coverage includes unavailable catalogue entries, failed reads without clearing
historical overrides, locked campaign choices, explicit target-change reset,
unchanged reassignment payloads, portal Escape and launch idempotence.
[src: commit: f26965f8a929d273b1356d2eaa953c8c9d5ab4c5]

The human-approved [principal recovery](compare-model-identity.md#principal-integration-and-validation-working-directory)
preserved the cancelled worker execution, commits and worktree. The principal
combined that delivery with `30a232a6` and added the focus assertion above.
Frontend tree `f8a2df947aa4ba9d8899caf8e121c1ee49e106bc` passed 35 focused
tests (4.91 seconds), then the full unfiltered suite from the `frontend`
working directory: 313 files / 4,071 tests, 55.00 seconds (run 48050).
Native and legacy TypeScript, Oxlint, ESLint (0 errors / 63 existing warnings),
i18n (4,481 keys per locale; 708 static unused-key warnings), and Vite
(1.07 seconds) passed. No threshold was relaxed or locale key removed.
Backend tree `ab1476a042ea819b0140791a86467937570c0c6e` is unchanged.
No browser, provider, coverage or successful automatic-execution claim is made.
[src: user: 2026-09-10: kt628-kt630-validation-cwd-recovery]

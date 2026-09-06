# OpenCode execution-surface regression evidence

KT-614 keeps this evidence at the frontend boundary. The regressions exercise
the usable-agent inputs supplied to the real UI consumers and assert the native
`OpenCode` value in their outgoing payloads; they do not invoke a provider.

| Surface | Regression evidence | Contract covered |
| --- | --- | --- |
| Discussion creation | `frontend/src/components/__tests__/NewDiscussionForm.test.tsx` | A usable OpenCode detection can be chosen in the discussion picker; the create callback receives `agent: 'OpenCode'`. [src: file: frontend/src/components/__tests__/NewDiscussionForm.test.tsx:404-435] |
| Quick Prompt Compare | `frontend/src/pages/__tests__/WorkflowsPage.qp-launch.test.tsx` | An installed OpenCode target is selectable in Compare and is sent as `{ agent: 'OpenCode', tier: 'default' }` alongside the other selected target. [src: file: frontend/src/pages/__tests__/WorkflowsPage.qp-launch.test.tsx:581-607] |
| Workflow step | `frontend/src/components/workflows/__tests__/WorkflowWizard.test.tsx` | The workflow-step picker accepts OpenCode and the create payload retains it on the step. [src: file: frontend/src/components/workflows/__tests__/WorkflowWizard.test.tsx:490-504] |
| Orchestration worker | `frontend/src/components/__tests__/TaskLaunchDialog.test.tsx` | A usable OpenCode detection appears in the worker select; campaign creation and launch both receive it as `worker.target.agent_type`. [src: file: frontend/src/components/__tests__/TaskLaunchDialog.test.tsx:90-109] |
| Compare judge | `frontend/src/components/__tests__/BatchComparePanel.test.tsx` | The judge picker accepts OpenCode and the judge-start payload keeps `{ agent: 'OpenCode', tier: 'default', connection_id: null }`. [src: file: frontend/src/components/__tests__/BatchComparePanel.test.tsx:354-397] |

QA and QE are not execution-agent options in the selector contracts covered by
this task; those controls accept `AgentType` targets. [src: file: frontend/src/components/AgentSwitchPicker.tsx:30-38]
This note therefore makes no claim that a QA/QE selector has been tested.

Limit: these are component tests with mocked frontend API boundaries. They
prove selection and request identity but intentionally do not cover provider
execution, browser E2E, backend dispatch, authentication, or catalog discovery.

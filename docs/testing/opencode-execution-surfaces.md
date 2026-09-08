# OpenCode execution-surface regression evidence

KT-614 keeps this evidence at the frontend boundary. The regressions exercise
the usable-agent inputs supplied to the real UI consumers and assert the native
`OpenCode` value in their outgoing payloads; they do not invoke a provider.

| Surface | Regression evidence | Contract covered |
| --- | --- | --- |
| Discussion creation | `frontend/src/components/__tests__/NewDiscussionForm.test.tsx` | A usable OpenCode detection can be chosen in the discussion picker; the create callback receives `agent: 'OpenCode'`. [src: file: frontend/src/components/__tests__/NewDiscussionForm.test.tsx:404-435] |
| Quick Prompt Compare | `frontend/src/pages/__tests__/WorkflowsPage.qp-launch.test.tsx` | An installed OpenCode target is selectable in Compare and is sent as `{ agent: 'OpenCode', tier: 'default' }` alongside the other selected target. [src: file: frontend/src/pages/__tests__/WorkflowsPage.qp-launch.test.tsx:581-607] |
| Workflow step | `frontend/src/components/workflows/__tests__/WorkflowWizard.test.tsx` | The workflow-step picker accepts OpenCode and the create payload retains it on the step. [src: file: frontend/src/components/workflows/__tests__/WorkflowWizard.test.tsx:490-504] |
| Orchestration worker | `frontend/src/components/__tests__/TaskLaunchDialog.test.tsx` | A usable OpenCode detection appears in the worker select; campaign creation and launch both receive the native target `{ kind: 'agent', agent_type: 'OpenCode', cli_session_id: null }`. [src: file: frontend/src/components/__tests__/TaskLaunchDialog.test.tsx:90-117] |
| Compare judge | `frontend/src/components/__tests__/BatchComparePanel.test.tsx` | The judge picker accepts OpenCode and the judge-start payload keeps `{ agent: 'OpenCode', tier: 'default', connection_id: null }`. [src: file: frontend/src/components/__tests__/BatchComparePanel.test.tsx:354-397] |

Quick API and Quick Exec have no agent selector for their own execution. The
Quick API form persists API plugin/config/endpoint fields and variables, while
its create request has no agent field. [src: file: frontend/src/components/workflows/QuickApiForm.tsx:211-237] [src: file: frontend/src/types/generated.ts:1356] Its direct-run handler sends only
`{ variables }` to `quickApis.runQa`. [src: file: frontend/src/pages/WorkflowsPage.tsx:1297-1317] [src: file: frontend/src/types/generated.ts:5221-5231]
The Quick Exec form instead persists a command, arguments, timeout, output
format, and variables; its create request likewise has no agent field. [src: file: frontend/src/components/workflows/QuickExecForm.tsx:46-66] [src: file: frontend/src/types/generated.ts:1358] Its direct-run handler sends only
`{ variables }` to `quickExecs.run`. [src: file: frontend/src/pages/WorkflowsPage.tsx:1276-1291] [src: file: frontend/src/types/generated.ts:5247]
Therefore this task makes no QA/QE OpenCode-selection assertion; that absence
comes from their UI/request/executor contracts, rather than from the `AgentType`
enum.

Limit: these are component tests with mocked frontend API boundaries. They
prove selection and request identity but intentionally do not cover provider
execution, browser E2E, backend dispatch, authentication, or catalog discovery.

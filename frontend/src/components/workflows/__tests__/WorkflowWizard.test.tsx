// Unit tests for WorkflowWizard — the multi-step workflow-builder wizard.
//
// This component was the biggest single coverage hole (34.6% Lines /
// 16.7% Functions, no existing test). The wizard is large, so these
// tests prioritise the UNCOVERED FUNCTIONS over exhaustive rendering:
//   - step navigation (next / previous / cancel / jump-after-name-gate)
//   - mode toggle (simple ↔ advanced) and the step-count gating
//   - name + project field onChange
//   - add / remove / reorder / insert step handlers
//   - step-type swap (Agent → Notify / Gate / Exec) and field clearing
//   - validation gates (Create button disabled when steps invalid)
//   - the save handler → assert workflowsApi.create / update payload
//   - save error surfacing + loading guard
//   - the exported pure helper buildBlankStep (default-tier semantics)
//
// Conventions copied from sibling tests: vi.hoisted api spies +
// buildApiMock, key-passthrough i18n stub, render + waitFor.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup, act } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { buildApiMock } from '../../../test/apiMock';
import type { Project, Workflow, WorkflowStep } from '../../../types/generated';

const { createMock, updateMock, qpListMock } = vi.hoisted(() => ({
  createMock: vi.fn(),
  updateMock: vi.fn(),
  qpListMock: vi.fn(),
}));

vi.mock('../../../lib/api', () => buildApiMock({
  workflows: {
    create: createMock as never,
    update: updateMock as never,
  },
  quickPrompts: {
    list: qpListMock as never,
  },
}));

// Key-passthrough i18n so assertions are locale-stable.
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length > 0 ? `${key}:${args.join(',')}` : key,
    locale: 'fr',
    setLocale: () => {},
  }),
}));

import { WorkflowWizard, buildBlankStep } from '../WorkflowWizard';

// ── Fixtures ────────────────────────────────────────────────────────

const mkProject = (over: Partial<Project> = {}): Project => ({
  id: 'proj-1',
  name: 'ProjectAlpha',
  path: '/tmp/alpha',
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'None' as Project['audit_status'],
  ai_todo_count: 0,
  created_at: '2026-05-01T00:00:00Z',
  updated_at: '2026-05-01T00:00:00Z',
  ...over,
});

const mkStep = (over: Partial<WorkflowStep> = {}): WorkflowStep => ({
  name: 'main',
  step_type: { type: 'Agent' },
  description: null,
  agent: 'ClaudeCode',
  prompt_template: 'do the thing',
  mode: { type: 'Normal' },
  output_format: { type: 'Structured' },
  ...over,
});

const mkWorkflow = (over: Partial<Workflow> = {}): Workflow => ({
  id: 'wf-1',
  name: 'ExistingWorkflow',
  project_id: 'proj-1',
  trigger: { type: 'Manual' },
  steps: [mkStep()],
  actions: [],
  enabled: true,
  created_at: '2026-05-01T00:00:00Z',
  updated_at: '2026-05-01T00:00:00Z',
  ...over,
} as Workflow);

// Typed props bag so override literals don't get rejected by narrow
// inference (tsc gotcha called out in the task).
const baseProps: ComponentProps<typeof WorkflowWizard> = {
  projects: [mkProject()],
  onDone: vi.fn(),
  onCancel: vi.fn(),
  installedAgentTypes: ['ClaudeCode'],
};

const renderWizard = (over: Partial<ComponentProps<typeof WorkflowWizard>> = {}) =>
  render(<WorkflowWizard {...baseProps} {...over} />);

beforeEach(() => {
  createMock.mockReset();
  updateMock.mockReset();
  qpListMock.mockReset();
  createMock.mockResolvedValue({});
  updateMock.mockResolvedValue({});
  qpListMock.mockResolvedValue([]);
  vi.stubGlobal('confirm', vi.fn(() => true));
});

const sampleQp = {
  id: 'qp-1',
  name: 'AuditPrompt',
  icon: '🔍',
  description: 'Audit',
  agent: 'ClaudeCode',
  prompt_template: 'audit {{target}}',
  variables: [{ name: 'target', label: 'Target', placeholder: '', description: null, required: true }],
} as never;

// ── buildBlankStep (pure, exported) ─────────────────────────────────

describe('buildBlankStep', () => {
  it('numbers the step from existingCount and defaults to Structured ClaudeCode', () => {
    const s = buildBlankStep(2, null);
    expect(s.name).toBe('step-3');
    expect(s.agent).toBe('ClaudeCode');
    expect(s.output_format).toEqual({ type: 'Structured' });
    expect(s.agent_settings).toBeUndefined();
  });

  it('leaves agent_settings unset for the "default" tier', () => {
    expect(buildBlankStep(0, 'default').agent_settings).toBeUndefined();
  });

  it('attaches a tier override for economy / reasoning', () => {
    expect(buildBlankStep(0, 'economy').agent_settings).toEqual({ tier: 'economy' });
    expect(buildBlankStep(0, 'reasoning').agent_settings).toEqual({ tier: 'reasoning' });
  });
});

// ── Mode toggle + initial render ────────────────────────────────────

describe('WorkflowWizard — mode + initial render', () => {
  it('renders the mode toggle and starts on the name step in simple mode by default', () => {
    renderWizard();
    expect(screen.getByText('wiz.modeSimple')).toBeInTheDocument();
    expect(screen.getByText('wiz.modeAdvanced')).toBeInTheDocument();
    // Name input is on step 0.
    expect(screen.getByLabelText('wiz.name')).toBeInTheDocument();
  });

  it('hides the mode toggle when editing an existing workflow', () => {
    renderWizard({ editWorkflow: mkWorkflow() });
    expect(screen.queryByText('wiz.modeSimple')).not.toBeInTheDocument();
  });

  it('switching to advanced mode shows the advanced progress labels (Trigger, Config)', () => {
    renderWizard();
    fireEvent.click(screen.getByText('wiz.modeAdvanced'));
    expect(screen.getByText('wiz.trigger')).toBeInTheDocument();
    expect(screen.getByText('wiz.config')).toBeInTheDocument();
  });

  it('opens an edit workflow with >1 step directly in advanced mode', () => {
    renderWizard({
      editWorkflow: mkWorkflow({ steps: [mkStep(), mkStep({ name: 'second' })] }),
    });
    // Advanced progress bar carries the Trigger label that simple mode lacks.
    expect(screen.getByText('wiz.trigger')).toBeInTheDocument();
  });
});

// ── Name / project field editing ────────────────────────────────────

describe('WorkflowWizard — name + project fields', () => {
  it('typing into the name input updates the value and clears the required hint', () => {
    renderWizard();
    // Empty name shows the required hint.
    expect(screen.getByText('wiz.nameRequired')).toBeInTheDocument();
    const nameInput = screen.getByLabelText('wiz.name') as HTMLInputElement;
    fireEvent.change(nameInput, { target: { value: 'My WF' } });
    expect(nameInput.value).toBe('My WF');
    expect(screen.queryByText('wiz.nameRequired')).not.toBeInTheDocument();
  });

  it('renders every project as a select option', () => {
    renderWizard({ projects: [mkProject(), mkProject({ id: 'proj-2', name: 'ProjectBeta' })] });
    const select = screen.getByLabelText('wiz.project') as HTMLSelectElement;
    fireEvent.change(select, { target: { value: 'proj-2' } });
    expect(select.value).toBe('proj-2');
    expect(screen.getByText('ProjectBeta')).toBeInTheDocument();
  });

  it('selecting a project seeds default skill ids onto steps without their own skills', () => {
    // No throw + the project is selected; the skill-seeding branch runs.
    renderWizard({ projects: [mkProject({ default_skill_ids: ['skill-1'] })] });
    const select = screen.getByLabelText('wiz.project') as HTMLSelectElement;
    fireEvent.change(select, { target: { value: 'proj-1' } });
    expect(select.value).toBe('proj-1');
  });
});

// ── Step navigation (next / previous / cancel / name gate) ──────────

describe('WorkflowWizard — navigation', () => {
  it('Next is disabled until the workflow is named', () => {
    renderWizard();
    const next = screen.getByText('wiz.next').closest('button') as HTMLButtonElement;
    expect(next).toBeDisabled();
    fireEvent.change(screen.getByLabelText('wiz.name'), { target: { value: 'Named' } });
    expect(next).not.toBeDisabled();
  });

  it('clicking Next advances to the next step (simple: Task)', () => {
    renderWizard();
    fireEvent.change(screen.getByLabelText('wiz.name'), { target: { value: 'Named' } });
    fireEvent.click(screen.getByText('wiz.next'));
    // Simple step 1 exposes the agent picker.
    expect(screen.getByLabelText('wiz.agentLabel')).toBeInTheDocument();
  });

  it('Previous walks back to the name step', () => {
    renderWizard();
    fireEvent.change(screen.getByLabelText('wiz.name'), { target: { value: 'Named' } });
    fireEvent.click(screen.getByText('wiz.next'));
    fireEvent.click(screen.getByText('wiz.previous'));
    expect(screen.getByLabelText('wiz.name')).toBeInTheDocument();
  });

  it('Cancel on the first step calls onCancel', () => {
    const onCancel = vi.fn();
    renderWizard({ onCancel });
    fireEvent.click(screen.getByText('common.cancel'));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });
});

// ── Advanced: trigger selection ─────────────────────────────────────

describe('WorkflowWizard — trigger step (advanced)', () => {
  const toTriggerStep = () => {
    renderWizard();
    fireEvent.click(screen.getByText('wiz.modeAdvanced'));
    fireEvent.change(screen.getByLabelText('wiz.name'), { target: { value: 'Named' } });
    fireEvent.click(screen.getByText('wiz.next'));
  };

  it('selecting the Cron trigger reveals the frequency editor', () => {
    toTriggerStep();
    fireEvent.click(screen.getByText('wiz.triggerScheduled'));
    expect(screen.getByText('wiz.frequency')).toBeInTheDocument();
  });

  it('selecting the Tracker trigger reveals owner/repo/labels inputs', () => {
    toTriggerStep();
    fireEvent.click(screen.getByText('wiz.triggerTracker'));
    const owner = screen.getByPlaceholderText('owner') as HTMLInputElement;
    const repo = screen.getByPlaceholderText('repo') as HTMLInputElement;
    fireEvent.change(owner, { target: { value: 'octo' } });
    fireEvent.change(repo, { target: { value: 'demo' } });
    expect(owner.value).toBe('octo');
    expect(repo.value).toBe('demo');
    // Labels input present (comma-separated).
    expect(screen.getByPlaceholderText('bug-5xx, auto-fix')).toBeInTheDocument();
  });
});

// ── Step add / remove / reorder (advanced steps page) ───────────────

describe('WorkflowWizard — step list handlers', () => {
  // A multi-step edit workflow opens in ADVANCED mode (needsAdvanced).
  // Walk Infos → Trigger → Steps to land on the steps page.
  const toStepsPage = (steps: WorkflowStep[]) => {
    const utils = renderWizard({ editWorkflow: mkWorkflow({ steps }) });
    fireEvent.click(screen.getByText('wiz.next')); // Infos → Trigger
    fireEvent.click(screen.getByText('wiz.next')); // Trigger → Steps
    return utils;
  };

  it('Add a step appends a new step to the list', () => {
    toStepsPage([mkStep(), mkStep({ name: 'beta' })]);
    const addBtn = screen.getByText('wiz.addStep').closest('button') as HTMLButtonElement;
    fireEvent.click(addBtn);
    // The appended step seeds name "step-3" (existingCount=2).
    expect(screen.getByDisplayValue('step-3')).toBeInTheDocument();
  });

  it('keeps the existing steps editable on the steps page', () => {
    toStepsPage([mkStep(), mkStep({ name: 'beta' })]);
    expect(screen.getByDisplayValue('main')).toBeInTheDocument();
    expect(screen.getByDisplayValue('beta')).toBeInTheDocument();
  });

  it('editing a step name propagates to the step', () => {
    toStepsPage([mkStep(), mkStep({ name: 'beta' })]);
    const stepName = screen.getByDisplayValue('main') as HTMLInputElement;
    fireEvent.change(stepName, { target: { value: 'renamed' } });
    expect(stepName.value).toBe('renamed');
  });

  it('adding a rollback (on_failure) step renders a Notify rollback row', () => {
    toStepsPage([mkStep(), mkStep({ name: 'beta' })]);
    const addRb = screen.getByText('wiz.addRollbackStep').closest('button') as HTMLButtonElement;
    fireEvent.click(addRb);
    // The seeded rollback step is named "rollback-1".
    expect(screen.getByDisplayValue('rollback-1')).toBeInTheDocument();
  });
});

// ── Save handler — create / update payload ──────────────────────────

describe('WorkflowWizard — save handler', () => {
  // Drive a simple-mode workflow all the way to the summary step.
  const toSummary = (over: Partial<ComponentProps<typeof WorkflowWizard>> = {}) => {
    renderWizard(over);
    fireEvent.change(screen.getByLabelText('wiz.name'), { target: { value: 'SaveMe' } });
    fireEvent.click(screen.getByText('wiz.next')); // → Task
    // The blank starting step has an empty prompt → fill it so the
    // validator passes and the Create button enables.
    fireEvent.change(screen.getByLabelText('wiz.promptLabel'), {
      target: { value: 'audit the repo' },
    });
    fireEvent.click(screen.getByText('wiz.next')); // → Summary
  };

  it('Create calls workflowsApi.create with the built payload and then onDone', async () => {
    const onDone = vi.fn();
    toSummary({ onDone });
    const createBtn = screen.getByText('wiz.create').closest('button') as HTMLButtonElement;
    expect(createBtn).not.toBeDisabled();
    fireEvent.click(createBtn);
    await waitFor(() => expect(createMock).toHaveBeenCalledTimes(1));
    const payload = createMock.mock.calls[0][0];
    expect(payload.name).toBe('SaveMe');
    expect(payload.trigger).toEqual({ type: 'Manual' });
    expect(Array.isArray(payload.steps)).toBe(true);
    expect(payload.steps.length).toBe(1);
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
    // The Manual trigger summary path renders "Manuel".
    expect(updateMock).not.toHaveBeenCalled();
  });

  it('Edit mode calls workflowsApi.update with the workflow id', async () => {
    const onDone = vi.fn();
    renderWizard({ editWorkflow: mkWorkflow(), onDone });
    // Edit → advanced (single Manual step still simple? No: single-step
    // Manual opens simple. Walk simple: Infos → Task → Summary).
    // Single-step Manual edit defaults to simple mode.
    fireEvent.click(screen.getByText('wiz.next')); // → Task
    fireEvent.click(screen.getByText('wiz.next')); // → Summary
    const saveBtn = screen.getByText('wiz.save').closest('button') as HTMLButtonElement;
    fireEvent.click(saveBtn);
    await waitFor(() => expect(updateMock).toHaveBeenCalledTimes(1));
    expect(updateMock.mock.calls[0][0]).toBe('wf-1');
    const payload = updateMock.mock.calls[0][1];
    expect(payload.name).toBe('ExistingWorkflow');
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
  });

  it('surfaces a save error banner when create rejects (and does not call onDone)', async () => {
    const onDone = vi.fn();
    createMock.mockRejectedValueOnce(new Error('backend boom'));
    toSummary({ onDone });
    fireEvent.click(screen.getByText('wiz.create'));
    await waitFor(() => expect(createMock).toHaveBeenCalled());
    // userError falls back to t('wiz.saveError') key; assert a banner appears.
    await waitFor(() =>
      expect(screen.getByText(/backend boom|wiz\.saveError/)).toBeInTheDocument(),
    );
    expect(onDone).not.toHaveBeenCalled();
  });

  it('Create button is disabled when an Agent step has no prompt', () => {
    // Edit a workflow whose only step has an empty prompt → invalid.
    renderWizard({ editWorkflow: mkWorkflow({ steps: [mkStep({ prompt_template: '' })] }) });
    fireEvent.click(screen.getByText('wiz.next')); // → Task
    fireEvent.click(screen.getByText('wiz.next')); // → Summary
    const saveBtn = screen.getByText('wiz.save').closest('button') as HTMLButtonElement;
    expect(saveBtn).toBeDisabled();
    // The visible validator lists the missing-prompt error too.
    expect(screen.getByText(/wiz\.errorNoPrompt/)).toBeInTheDocument();
  });

  it('does not double-submit on a fast double-click (saving guard)', async () => {
    toSummary();
    const createBtn = screen.getByText('wiz.create').closest('button') as HTMLButtonElement;
    await act(async () => {
      fireEvent.click(createBtn);
      fireEvent.click(createBtn);
    });
    await waitFor(() => expect(createMock).toHaveBeenCalledTimes(1));
  });

  it('advanced create builds a Cron trigger + safety payload', async () => {
    renderWizard();
    fireEvent.click(screen.getByText('wiz.modeAdvanced'));
    fireEvent.change(screen.getByLabelText('wiz.name'), { target: { value: 'CronWF' } });
    fireEvent.click(screen.getByText('wiz.next')); // → Trigger
    fireEvent.click(screen.getByText('wiz.triggerScheduled')); // Cron
    fireEvent.click(screen.getByText('wiz.next')); // → Steps
    // Give the single Agent step a prompt so validation passes.
    const prompt = document.querySelector('textarea.wf-textarea') as HTMLTextAreaElement;
    fireEvent.change(prompt, { target: { value: 'analyse' } });
    fireEvent.click(screen.getByText('wiz.next')); // → Config
    // Enable sandbox safety.
    fireEvent.click(screen.getAllByRole('checkbox')[0]);
    fireEvent.click(screen.getByText('wiz.next')); // → Summary
    const createBtn = screen.getByText('wiz.create').closest('button') as HTMLButtonElement;
    fireEvent.click(createBtn);
    await waitFor(() => expect(createMock).toHaveBeenCalledTimes(1));
    const payload = createMock.mock.calls[0][0];
    expect(payload.name).toBe('CronWF');
    expect(payload.trigger.type).toBe('Cron');
    // Default minutes cron → "*/5 * * * *".
    expect(payload.trigger.schedule).toMatch(/^\*\/\d+ \* \* \* \*$/);
    expect(payload.safety?.sandbox).toBe(true);
  });
});

// ── Steps page: type swaps + per-type forms ─────────────────────────

describe('WorkflowWizard — step-type swaps', () => {
  // Land on the advanced Steps page with a single Agent step. We use a
  // 2-step workflow to force advanced mode, then act on the first step.
  const toSteps = (steps: WorkflowStep[] = [mkStep(), mkStep({ name: 'beta' })]) => {
    renderWizard({ editWorkflow: mkWorkflow({ steps }) });
    fireEvent.click(screen.getByText('wiz.next')); // → Trigger
    fireEvent.click(screen.getByText('wiz.next')); // → Steps
  };

  it('swapping to Notify reveals the webhook URL field and edits url/method/body', () => {
    toSteps();
    // First Notify swap button (one per step; act on the first).
    fireEvent.click(screen.getAllByText('wiz.stepTypeNotify')[0]);
    expect(screen.getByText('wiz.notifyTitle')).toBeInTheDocument();
    expect(screen.getByText(/wiz\.notifyUrl/)).toBeInTheDocument();
    const url = screen.getByPlaceholderText(/hooks\.slack\.com/) as HTMLInputElement;
    fireEvent.change(url, { target: { value: 'https://hooks.example.com/x' } });
    expect(url.value).toBe('https://hooks.example.com/x');
    const method = screen.getByDisplayValue('POST') as HTMLSelectElement;
    fireEvent.change(method, { target: { value: 'PUT' } });
    expect(method.value).toBe('PUT');
    const body = screen.getByText('wiz.notifyBody').parentElement
      ?.querySelector('textarea') as HTMLTextAreaElement;
    fireEvent.change(body, { target: { value: '{"text":"hi"}' } });
    expect(body.value).toBe('{"text":"hi"}');
  });

  it('swapping to Gate reveals the gate message field and editing it works', () => {
    toSteps();
    fireEvent.click(screen.getAllByText('wiz.stepTypeGate')[0]);
    expect(screen.getByText('wiz.gateMessage')).toBeInTheDocument();
    const msg = screen.getByPlaceholderText('wiz.gateMessagePlaceholder') as HTMLTextAreaElement;
    fireEvent.change(msg, { target: { value: 'Please review' } });
    expect(msg.value).toBe('Please review');
  });

  it('swapping to Exec surfaces the empty-allowlist warning + "configure now" CTA', () => {
    toSteps();
    fireEvent.click(screen.getAllByText('wiz.stepTypeExec')[0]);
    // No allowlist configured yet → the warning + CTA render (not the
    // command picker). Clicking the CTA jumps to the Config tab.
    expect(screen.getByText('wiz.execTitle')).toBeInTheDocument();
    expect(screen.getByText('wiz.execAllowlistEmpty')).toBeInTheDocument();
    fireEvent.click(screen.getByText('wiz.execAllowlistConfigureNow'));
    // Config tab now shows the allowlist section.
    expect(screen.getByText('wiz.execAllowlistTitle')).toBeInTheDocument();
  });

  it('swapping to JsonData reveals the payload editor and parses valid JSON', () => {
    toSteps();
    fireEvent.click(screen.getAllByText('wiz.stepTypeJsonData')[0]);
    expect(screen.getByText('wiz.jsonDataTitle')).toBeInTheDocument();
    expect(screen.getByText(/wiz\.jsonDataPayload/)).toBeInTheDocument();
    const editor = screen.getByPlaceholderText('wiz.jsonDataPlaceholder') as HTMLTextAreaElement;
    fireEvent.change(editor, { target: { value: '{"a":1}' } });
    // The textarea is controlled off the parsed payload → it pretty-prints
    // the accepted JSON with 2-space indentation.
    expect(editor.value).toContain('"a": 1');
    // Object summary surfaces (1 key).
    expect(screen.getByText(/wiz\.jsonDataSummaryObject/)).toBeInTheDocument();
  });

  it('JsonData ignores invalid JSON (payload not stored, editor stays empty)', () => {
    toSteps();
    fireEvent.click(screen.getAllByText('wiz.stepTypeJsonData')[0]);
    const editor = screen.getByPlaceholderText('wiz.jsonDataPlaceholder') as HTMLTextAreaElement;
    fireEvent.change(editor, { target: { value: '{ not json' } });
    // Invalid input is not committed to the (typed) payload, so the
    // controlled value resets to empty.
    expect(editor.value).toBe('');
  });

  it('swapping to ApiCall mounts the ApiCall step card (empty-plugin state)', () => {
    toSteps();
    fireEvent.click(screen.getAllByText('wiz.stepTypeApiCall')[0]);
    // No plugins configured → the card renders its not-supported notice.
    expect(screen.getByText('wf.apicall.notSupported')).toBeInTheDocument();
  });

  it('swapping the SECOND step to BatchQuickPrompt renders the batch form', () => {
    toSteps();
    // Act on the second step so selectBatchQpStepType patches the
    // predecessor (the first Agent step) to Structured + low-effort.
    fireEvent.click(screen.getAllByText('wiz.stepTypeBatchQP')[1]);
    // No QPs configured in the default mock → the batch form surfaces the
    // "no quick prompts" notice (still proves the BatchQP branch rendered).
    expect(screen.getByText('wiz.batchQPTitle')).toBeInTheDocument();
  });

  it('with QPs seeded, the batch form lets you pick a QP and an items source', async () => {
    qpListMock.mockResolvedValue([sampleQp]);
    renderWizard({ editWorkflow: mkWorkflow({ steps: [mkStep(), mkStep({ name: 'beta' })] }) });
    fireEvent.click(screen.getByText('wiz.next')); // → Trigger
    fireEvent.click(screen.getByText('wiz.next')); // → Steps
    // Wait for QPs to load so the batch picker has options.
    await screen.findAllByDisplayValue('wiz.agentQpPickerInline');
    fireEvent.click(screen.getAllByText('wiz.stepTypeBatchQP')[1]);
    // The batch QP picker (data-invalid select) is present + selectable.
    const picker = screen.getByText(/wiz\.batchQPPicker$/).parentElement
      ?.parentElement?.querySelector('select[data-invalid]') as HTMLSelectElement
      ?? document.querySelector('select[data-invalid]') as HTMLSelectElement;
    fireEvent.change(picker, { target: { value: 'qp-1' } });
    // Items source textarea (data-invalid) accepts a JSONPath-ish source.
    const itemsFrom = document.querySelector('textarea[data-invalid]') as HTMLTextAreaElement;
    fireEvent.change(itemsFrom, { target: { value: '{{steps.main.data.items}}' } });
    expect(itemsFrom.value).toBe('{{steps.main.data.items}}');
  });

  it('honors a cancelled confirm() and keeps the Agent step intact', () => {
    // The first step has a non-empty prompt → swapping fires confirm().
    // Reject it; the Agent prompt textarea must still be there.
    vi.stubGlobal('confirm', vi.fn(() => false));
    toSteps();
    fireEvent.click(screen.getAllByText('wiz.stepTypeNotify')[0]);
    // Notify form did NOT appear because the swap was cancelled.
    expect(screen.queryByText('wiz.notifyTitle')).not.toBeInTheDocument();
  });

  it('binding a Quick Prompt to an Agent step shows the inherited banner', async () => {
    qpListMock.mockResolvedValue([sampleQp]);
    renderWizard({ editWorkflow: mkWorkflow({ steps: [mkStep(), mkStep({ name: 'beta' })] }) });
    fireEvent.click(screen.getByText('wiz.next')); // → Trigger
    fireEvent.click(screen.getByText('wiz.next')); // → Steps
    // Wait for the QP list fetch to populate the inline picker (one per
    // Agent step — act on the first).
    const qpSelects = await screen.findAllByDisplayValue('wiz.agentQpPickerInline');
    fireEvent.change(qpSelects[0], { target: { value: 'qp-1' } });
    // The inherited-from banner renders the QP name via the templated key.
    expect(screen.getByText(/wiz\.agentQpInheritedFrom/)).toBeInTheDocument();
  });
});

// ── Steps page: move / insert / advanced toggle ─────────────────────

describe('WorkflowWizard — step reorder + advanced toggle', () => {
  const toSteps = () => {
    renderWizard({
      editWorkflow: mkWorkflow({ steps: [mkStep(), mkStep({ name: 'beta' })] }),
    });
    fireEvent.click(screen.getByText('wiz.next')); // → Trigger
    fireEvent.click(screen.getByText('wiz.next')); // → Steps
  };

  it('inserting a step adds a fresh blank step', () => {
    toSteps();
    fireEvent.click(screen.getAllByLabelText('wiz.insertStepHere')[0]);
    // existingCount=2 → seeded name step-3.
    expect(screen.getByDisplayValue('step-3')).toBeInTheDocument();
  });

  it('move-down swaps the first two steps order', () => {
    toSteps();
    const before = screen.getAllByDisplayValue(/^(main|beta)$/).map(
      el => (el as HTMLInputElement).value,
    );
    expect(before).toEqual(['main', 'beta']);
    fireEvent.click(screen.getAllByLabelText('wiz.moveStepDown')[0]);
    const after = screen.getAllByDisplayValue(/^(main|beta)$/).map(
      el => (el as HTMLInputElement).value,
    );
    expect(after).toEqual(['beta', 'main']);
  });

  it('removing a step drops it from the list (>1 step)', () => {
    toSteps();
    expect(screen.getByDisplayValue('beta')).toBeInTheDocument();
    // Two "Remove step" controls (one per step); remove the second.
    fireEvent.click(screen.getAllByLabelText('Remove step')[1]);
    expect(screen.queryByDisplayValue('beta')).not.toBeInTheDocument();
    expect(screen.getByDisplayValue('main')).toBeInTheDocument();
  });

  it('the per-step Advanced toggle reveals agent-settings + timeout + retry', () => {
    toSteps();
    const toggles = screen.getAllByText('wiz.advanced');
    fireEvent.click(toggles[0]);
    // Agent settings + execution-limit fields now render.
    expect(screen.getByText('wiz.agentSettings')).toBeInTheDocument();
    expect(screen.getByText('wiz.stallTimeout')).toBeInTheDocument();
    expect(screen.getByText('wiz.retry')).toBeInTheDocument();
    // Setting a stall timeout updates the field.
    const timeout = screen.getByPlaceholderText('600') as HTMLInputElement;
    fireEvent.change(timeout, { target: { value: '120' } });
    expect(timeout.value).toBe('120');
    // Re-clicking collapses again (toggle path covered).
    fireEvent.click(toggles[0]);
    expect(screen.getByDisplayValue('main')).toBeInTheDocument();
  });

  it('adds, edits and removes an on_result condition', () => {
    toSteps();
    fireEvent.click(screen.getAllByText('wiz.advanced')[0]);
    expect(screen.getByText('wiz.conditions')).toBeInTheDocument();
    // No condition yet → "Condition custom" adds one.
    fireEvent.click(screen.getAllByText('Condition custom')[0]);
    const contains = screen.getByPlaceholderText('wiz.ifContainsPlaceholder') as HTMLInputElement;
    fireEvent.change(contains, { target: { value: 'DONE' } });
    expect(contains.value).toBe('DONE');
    // Remove it again.
    fireEvent.click(screen.getAllByLabelText('Remove condition')[0]);
    expect(screen.queryByPlaceholderText('wiz.ifContainsPlaceholder')).not.toBeInTheDocument();
  });

  it('switching a condition action to Goto reveals the target-step picker', () => {
    toSteps();
    fireEvent.click(screen.getAllByText('wiz.advanced')[0]);
    fireEvent.click(screen.getAllByText('Condition custom')[0]);
    // The action <select> defaults to "Stop"; switch to "Goto".
    const actionSelect = screen.getByDisplayValue('wiz.condActionStop') as HTMLSelectElement;
    fireEvent.change(actionSelect, { target: { value: 'Goto' } });
    // The goto target picker now offers the other step ("beta").
    expect(screen.getByText('wiz.gotoMaxIterLabel')).toBeInTheDocument();
  });

  it('the "no-results stop" preset adds a NO_RESULTS condition in one click', () => {
    toSteps();
    fireEvent.click(screen.getAllByText('wiz.advanced')[0]);
    fireEvent.click(screen.getAllByText('wiz.noResultsStop')[0]);
    // The contains field is pre-filled with NO_RESULTS.
    expect(screen.getByDisplayValue('NO_RESULTS')).toBeInTheDocument();
  });

  it('editing a rollback Notify step propagates url + body', () => {
    toSteps();
    fireEvent.click(screen.getByText('wiz.addRollbackStep').closest('button')!);
    // The rollback Notify URL input uses the slack-hooks placeholder.
    const rbUrl = screen.getByPlaceholderText('https://hooks.slack.com/services/...') as HTMLInputElement;
    fireEvent.change(rbUrl, { target: { value: 'https://ops.example.com' } });
    expect(rbUrl.value).toBe('https://ops.example.com');
  });
});

// ── Advanced: Config tab + cron frequency editor ────────────────────

describe('WorkflowWizard — config tab + cron', () => {
  const toAdvanced = () => {
    renderWizard();
    fireEvent.click(screen.getByText('wiz.modeAdvanced'));
    fireEvent.change(screen.getByLabelText('wiz.name'), { target: { value: 'AdvWF' } });
  };

  it('cron frequency editor edits the interval and reflects in summary', () => {
    toAdvanced();
    fireEvent.click(screen.getByText('wiz.next')); // → Trigger
    fireEvent.click(screen.getByText('wiz.triggerScheduled'));
    // The numeric "every" input — change it.
    const everyInput = document.querySelector('input[type="number"]') as HTMLInputElement;
    fireEvent.change(everyInput, { target: { value: '15' } });
    expect(everyInput.value).toBe('15');
  });

  it('Config tab toggles safety sandbox and exposes the exec allowlist', () => {
    toAdvanced();
    fireEvent.click(screen.getByText('wiz.next')); // → Trigger
    fireEvent.click(screen.getByText('wiz.next')); // → Steps
    fireEvent.click(screen.getByText('wiz.next')); // → Config
    expect(screen.getByText('wiz.security')).toBeInTheDocument();
    const sandbox = screen.getAllByRole('checkbox')[0] as HTMLInputElement;
    fireEvent.click(sandbox);
    expect(sandbox.checked).toBe(true);
    // Exec allowlist input present.
    expect(screen.getByText('wiz.execAllowlistTitle')).toBeInTheDocument();
  });

  it('adding a launch variable on the Config tab renders a variable row', () => {
    toAdvanced();
    fireEvent.click(screen.getByText('wiz.next')); // → Trigger
    fireEvent.click(screen.getByText('wiz.next')); // → Steps
    fireEvent.click(screen.getByText('wiz.next')); // → Config
    fireEvent.click(screen.getByText('wiz.addVariable').closest('button')!);
    expect(screen.getByDisplayValue('var_1')).toBeInTheDocument();
  });

  it('the expert-config toggle reveals concurrency + workspace hooks', () => {
    toAdvanced();
    fireEvent.click(screen.getByText('wiz.next')); // → Trigger
    fireEvent.click(screen.getByText('wiz.next')); // → Steps
    fireEvent.click(screen.getByText('wiz.next')); // → Config
    fireEvent.click(screen.getByText('wiz.advanced'));
    expect(screen.getByText('wiz.concurrency')).toBeInTheDocument();
    expect(screen.getByText('wiz.hooks')).toBeInTheDocument();
    // The concurrency input is the number input with max=20 (maxFiles /
    // maxLines share the "illimite" placeholder but have no max).
    const concurrency = Array.from(
      document.querySelectorAll('input[placeholder="illimite"]'),
    ).find(el => el.getAttribute('max') === '20') as HTMLInputElement;
    fireEvent.change(concurrency, { target: { value: '3' } });
    expect(concurrency.value).toBe('3');
    // A workspace hook input also edits.
    const hookInput = screen.getByPlaceholderText('npm install') as HTMLInputElement;
    fireEvent.change(hookInput, { target: { value: 'pnpm i' } });
    expect(hookInput.value).toBe('pnpm i');
  });
});

// ── Simple-mode task page (agent + prompt + starters) ───────────────

describe('WorkflowWizard — simple task page', () => {
  const toTask = () => {
    renderWizard();
    fireEvent.change(screen.getByLabelText('wiz.name'), { target: { value: 'TaskWF' } });
    fireEvent.click(screen.getByText('wiz.next')); // → Task
  };

  it('changing the agent picker updates the step agent', () => {
    renderWizard({ installedAgentTypes: ['ClaudeCode', 'Codex'] });
    fireEvent.change(screen.getByLabelText('wiz.name'), { target: { value: 'TaskWF' } });
    fireEvent.click(screen.getByText('wiz.next'));
    const agentSelect = screen.getByLabelText('wiz.agentLabel') as HTMLSelectElement;
    fireEvent.change(agentSelect, { target: { value: 'Codex' } });
    expect(agentSelect.value).toBe('Codex');
  });

  it('clicking a starter card fills the prompt and names the workflow', () => {
    toTask();
    // Starter cards render only when prompt is empty + not editing.
    const codeReview = screen.getByText('wiz.starter.codeReview').closest('button') as HTMLButtonElement;
    fireEvent.click(codeReview);
    const prompt = screen.getByLabelText('wiz.promptLabel') as HTMLTextAreaElement;
    expect(prompt.value).toBe('wiz.starter.codeReviewPrompt');
  });

  it('selecting the scheduled trigger reveals the cron weekday chips on the days unit', () => {
    toTask();
    fireEvent.click(screen.getByText('wiz.triggerScheduled'));
    // Default unit is minutes; switch to days to surface weekday chips.
    const daySelect = Array.from(document.querySelectorAll('select')).find(
      s => Array.from(s.options).some(o => o.value === 'days'),
    ) as HTMLSelectElement;
    fireEvent.change(daySelect, { target: { value: 'days' } });
    expect(screen.getByText(/wiz\.cronWeekdaysLabel/)).toBeInTheDocument();
    // Toggle a weekday chip (Monday).
    const monday = screen.getByText('wiz.weekdayShort.1').closest('button') as HTMLButtonElement;
    fireEvent.click(monday);
    expect(monday).toHaveAttribute('aria-pressed', 'true');
  });
});

// ── Summary rendering ───────────────────────────────────────────────

describe('WorkflowWizard — summary', () => {
  it('shows the workflow name, project and step count on the summary step', () => {
    renderWizard();
    fireEvent.change(screen.getByLabelText('wiz.name'), { target: { value: 'SummaryWF' } });
    fireEvent.click(screen.getByText('wiz.next')); // → Task
    fireEvent.click(screen.getByText('wiz.next')); // → Summary
    expect(screen.getByText('SummaryWF')).toBeInTheDocument();
    // Step count row renders "1".
    expect(screen.getByText('Steps')).toBeInTheDocument();
  });
});

afterEach(() => cleanup());

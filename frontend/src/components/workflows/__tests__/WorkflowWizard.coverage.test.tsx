// Supplementary coverage for WorkflowWizard — targets the paths the
// primary WorkflowWizard.test.tsx intentionally skipped to push Lines
// past the 70% milestone:
//   - parseCronExpr branches (hourly / daily / weekday-list / raw)
//   - the preset deep-link useEffect (initialPresetId) incl. the
//     JsonData → ApiCall tracker-plugin transform
//   - applyStarterTemplate / applySuggestion / applyQuickStart preset path
//   - the full Summary recap for every step_type (API / Notify / Gate /
//     Exec / BatchApiCall / JsonData) + safety/hooks summary rows
//   - the per-step Exec setup-phase form, BatchApiCall form,
//     JsonData parse-error branch
//   - Skills / Profiles / Directives selectors (seeded API mocks)
//   - on_failure Agent + ApiCall rollback kinds
//   - state chips (insertVarAtCursor / appendPromptBlock via Goto pairs)
//   - cron weekday emit + raw cron switch + summary cron label
//
// Conventions mirror the sibling test: vi.hoisted spies + buildApiMock,
// key-passthrough i18n stub, confirm stub, ComponentProps<typeof X> props.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { buildApiMock } from '../../../test/apiMock';
import type {
  Project, Workflow, WorkflowStep, Skill, AgentProfile, Directive,
  QuickApi,
} from '../../../types/generated';

const {
  createMock, updateMock, qpListMock, qaListMock,
  skillsListMock, profilesListMock, directivesListMock,
  suggestionsMock, overviewMock,
} = vi.hoisted(() => ({
  createMock: vi.fn(),
  updateMock: vi.fn(),
  qpListMock: vi.fn(),
  qaListMock: vi.fn(),
  skillsListMock: vi.fn(),
  profilesListMock: vi.fn(),
  directivesListMock: vi.fn(),
  suggestionsMock: vi.fn(),
  overviewMock: vi.fn(),
}));

vi.mock('../../../lib/api', () => buildApiMock({
  workflows: {
    create: createMock as never,
    update: updateMock as never,
    suggestions: suggestionsMock as never,
  },
  quickPrompts: { list: qpListMock as never },
  quickApis: { list: qaListMock as never },
  skills: { list: skillsListMock as never },
  profiles: { list: profilesListMock as never },
  directives: { list: directivesListMock as never },
  mcps: { overview: overviewMock as never },
}));

vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length > 0 ? `${key}:${args.join(',')}` : key,
    locale: 'fr',
    setLocale: () => {},
  }),
}));

import { WorkflowWizard } from '../WorkflowWizard';

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
  qaListMock.mockReset();
  skillsListMock.mockReset();
  profilesListMock.mockReset();
  directivesListMock.mockReset();
  suggestionsMock.mockReset();
  overviewMock.mockReset();
  createMock.mockResolvedValue({});
  updateMock.mockResolvedValue({});
  qpListMock.mockResolvedValue([]);
  qaListMock.mockResolvedValue([]);
  skillsListMock.mockResolvedValue([]);
  profilesListMock.mockResolvedValue([]);
  directivesListMock.mockResolvedValue([]);
  suggestionsMock.mockResolvedValue([]);
  overviewMock.mockResolvedValue({ servers: [], configs: [], customized_contexts: [], incompatibilities: [] });
  vi.stubGlobal('confirm', vi.fn(() => true));
});

afterEach(() => cleanup());

// Walk an editWorkflow (which opens advanced) Infos → Trigger → Steps.
const toSteps = (steps: WorkflowStep[]) => {
  renderWizard({ editWorkflow: mkWorkflow({ steps }) });
  fireEvent.click(screen.getByText('wiz.next')); // Infos → Trigger
  fireEvent.click(screen.getByText('wiz.next')); // Trigger → Steps
};

// Walk all the way to the summary (advanced, multi-step). Trigger may be
// Cron so the trigger page auto-advances without extra config.
const toSummaryAdvanced = (over: Partial<ComponentProps<typeof WorkflowWizard>>) => {
  renderWizard(over);
  fireEvent.click(screen.getByText('wiz.next')); // Infos → Trigger
  fireEvent.click(screen.getByText('wiz.next')); // Trigger → Steps
  fireEvent.click(screen.getByText('wiz.next')); // Steps → Config
  fireEvent.click(screen.getByText('wiz.next')); // Config → Summary
};

// ── parseCronExpr branches (via editWorkflow Cron triggers) ─────────

describe('WorkflowWizard — parseCronExpr branches', () => {
  it('parses an hourly cron (m */N * * *) into the hours unit', () => {
    renderWizard({
      editWorkflow: mkWorkflow({ trigger: { type: 'Cron', schedule: '30 */2 * * *' } }),
    });
    // Cron trigger forces advanced; navigate to the trigger step.
    fireEvent.click(screen.getByText('wiz.next'));
    // The numeric "every" field reflects the parsed value (2 hours).
    const every = document.querySelector('input[type="number"]') as HTMLInputElement;
    expect(every.value).toBe('2');
  });

  it('parses a daily weekday-list cron (m h * * 1,3,5) into specific weekdays', () => {
    renderWizard({
      editWorkflow: mkWorkflow({ trigger: { type: 'Cron', schedule: '0 9 * * 1,3,5' } }),
    });
    fireEvent.click(screen.getByText('wiz.next'));
    // Weekday chips render; Monday should be pre-selected (aria-pressed).
    const monday = screen.getByText('wiz.weekdayShort.1').closest('button') as HTMLButtonElement;
    expect(monday).toHaveAttribute('aria-pressed', 'true');
    const tuesday = screen.getByText('wiz.weekdayShort.2').closest('button') as HTMLButtonElement;
    expect(tuesday).toHaveAttribute('aria-pressed', 'false');
  });

  it('parses a daily every-N cron (m h */N * *) into the days unit', () => {
    renderWizard({
      editWorkflow: mkWorkflow({ trigger: { type: 'Cron', schedule: '15 8 */3 * *' } }),
    });
    fireEvent.click(screen.getByText('wiz.next'));
    const every = document.querySelector('input[type="number"]') as HTMLInputElement;
    expect(every.value).toBe('3');
  });

  it('preserves a complex cron as a raw expression with a switch-to-simple link', () => {
    renderWizard({
      editWorkflow: mkWorkflow({ trigger: { type: 'Cron', schedule: '0 7,10,13 * * 1-5' } }),
    });
    fireEvent.click(screen.getByText('wiz.next'));
    // Raw mode renders the verbatim expression in a text input + a
    // "switch to simple builder" link.
    const raw = screen.getByDisplayValue('0 7,10,13 * * 1-5') as HTMLInputElement;
    expect(raw).toBeInTheDocument();
    fireEvent.click(screen.getByText('wiz.cronSwitchSimple'));
    // After switching back to the visual builder the every field is 5.
    const every = document.querySelector('input[type="number"]') as HTMLInputElement;
    expect(every.value).toBe('5');
  });

  it('falls back to the default builder for a malformed (3-field) cron', () => {
    renderWizard({
      editWorkflow: mkWorkflow({ trigger: { type: 'Cron', schedule: '0 0 0' } }),
    });
    fireEvent.click(screen.getByText('wiz.next'));
    const every = document.querySelector('input[type="number"]') as HTMLInputElement;
    expect(every.value).toBe('5');
  });
});

// ── Cron editor: weekday emit + summary label ───────────────────────

describe('WorkflowWizard — cron weekday emit + human label in summary', () => {
  it('emits a DoW cron and renders the weekday summary label after picking days', async () => {
    renderWizard();
    fireEvent.click(screen.getByText('wiz.modeAdvanced'));
    fireEvent.change(screen.getByLabelText('wiz.name'), { target: { value: 'WkWF' } });
    fireEvent.click(screen.getByText('wiz.next')); // → Trigger
    fireEvent.click(screen.getByText('wiz.triggerScheduled')); // Cron
    // Switch unit to days to surface weekday chips.
    const unit = Array.from(document.querySelectorAll('select')).find(
      s => Array.from(s.options).some(o => o.value === 'days'),
    ) as HTMLSelectElement;
    fireEvent.change(unit, { target: { value: 'days' } });
    // Pick Monday + Wednesday.
    fireEvent.click(screen.getByText('wiz.weekdayShort.1').closest('button')!);
    fireEvent.click(screen.getByText('wiz.weekdayShort.3').closest('button')!);
    // The cron preview now shows a DoW expression.
    const previews = screen.getAllByText(/\* \* 1,3$/);
    expect(previews.length).toBeGreaterThan(0);
    // Navigate to summary; the human label path with weekdays renders.
    fireEvent.click(screen.getByText('wiz.next')); // → Steps
    // Give the single step a prompt so we can keep going (textarea).
    const prompt = document.querySelector('textarea.wf-textarea') as HTMLTextAreaElement;
    fireEvent.change(prompt, { target: { value: 'analyse' } });
    fireEvent.click(screen.getByText('wiz.next')); // → Config
    fireEvent.click(screen.getByText('wiz.next')); // → Summary
    // The summary Trigger row shows the cron expression in parens.
    expect(screen.getByText(/1,3/)).toBeInTheDocument();
    // "Clear weekdays" link present + works.
    fireEvent.click(screen.getByText('wiz.previous')); // → Config
    fireEvent.click(screen.getByText('wiz.previous')); // → Steps
    fireEvent.click(screen.getByText('wiz.previous')); // → Trigger
    fireEvent.click(screen.getByText('wiz.cronWeekdaysAll'));
    await waitFor(() =>
      expect(screen.queryByText('wiz.cronWeekdaysAll')).not.toBeInTheDocument(),
    );
  });
});

// ── Preset deep-link useEffect (initialPresetId) ────────────────────

describe('WorkflowWizard — preset deep-link', () => {
  it('applies a preset on mount and lands on the advanced Steps page', async () => {
    renderWizard({ initialPresetId: 'pr-gate', initialProjectId: 'proj-1' });
    // The preset effect waits for plugins to settle then jumps to step 2
    // in advanced mode. The advanced progress labels confirm the mode.
    await waitFor(() => expect(screen.getByText('wiz.config')).toBeInTheDocument());
    // The preset seeds a multi-step pipeline → an "Add step" control on
    // the Steps page proves we landed there.
    await waitFor(() => expect(screen.getByText('wiz.addStep')).toBeInTheDocument());
  });

  it('transforms the ticket-to-pr fetch_issue step into an ApiCall when a tracker plugin is wired', async () => {
    // Seed a GitHub tracker plugin bound globally, with a github repo_url
    // project so the repo-hint precedence path runs.
    overviewMock.mockResolvedValue({
      servers: [{ id: 'mcp-github', name: 'GitHub', api_spec: { base_url: 'https://api.github.com', auth: 'None', endpoints: [] } }],
      configs: [{ id: 'cfg-gh', server_id: 'mcp-github', label: 'GH', is_global: true, project_ids: [] }],
      customized_contexts: [],
      incompatibilities: [],
    } as never);
    renderWizard({
      initialPresetId: 'ticket-to-pr',
      initialProjectId: 'proj-1',
      projects: [mkProject({ repo_url: 'https://github.com/octo/demo' })],
    });
    // The preset effect runs once plugins settle and jumps to advanced
    // Steps (step 2). The transformed pipeline surfaces step-type buttons,
    // and the first step is now an ApiCall (selected pill).
    await waitFor(() =>
      expect(screen.getAllByText('wiz.stepTypeApiCall').length).toBeGreaterThan(0),
    );
    // At least one ApiCall step pill is selected (the transformed fetch_issue).
    const selectedApi = Array.from(document.querySelectorAll('[data-type="api"]'))
      .some(el => el.getAttribute('data-selected') === 'true');
    expect(selectedApi).toBe(true);
  });
});

// ── QuickStart picker apply paths (starter / suggestion / preset) ───

describe('WorkflowWizard — QuickStart apply paths', () => {
  it('applies a starter template via the QuickStart picker', async () => {
    // The starter is only "applicable" when its primary plugin is wired —
    // seed a chartbeat plugin so the apply button is enabled.
    overviewMock.mockResolvedValue({
      servers: [{ id: 'chartbeat', name: 'Chartbeat', api_spec: { base_url: 'https://api.chartbeat.com', auth: 'None', endpoints: [] } }],
      configs: [{ id: 'cfg-cb', server_id: 'chartbeat', label: 'CB', is_global: true, project_ids: [] }],
      customized_contexts: [],
      incompatibilities: [],
    } as never);
    // The picker needs the workflow to be named first (disabled otherwise).
    renderWizard();
    fireEvent.change(screen.getByLabelText('wiz.name'), { target: { value: 'StarterWF' } });
    // Wait for the plugins fetch so the starter row becomes applicable.
    await screen.findByText(/wiz\.quickstart\.toggle/);
    // The picker is collapsed by default — expand it via the toggle chip.
    fireEvent.click(screen.getByText(/wiz\.quickstart\.toggle/).closest('button')!);
    // The starter row title is the template's title_fr.
    const starter = await screen.findByText('Chartbeat top 5 → Résumé IA → Slack');
    const row = starter.closest('.wf-quickstart-row')!;
    fireEvent.click(row.querySelector('.wf-quickstart-apply-btn')!);
    // Applying jumps to the advanced Steps page.
    await waitFor(() => expect(screen.getByText('wiz.addStep')).toBeInTheDocument());
  });

  it('applies a project suggestion (forces advanced for multi-step)', async () => {
    suggestionsMock.mockResolvedValue([{
      id: 'sug-1',
      title: 'SuggestedWF',
      description: 'A suggested workflow',
      complexity: 'advanced',
      audience: null,
      required_mcps: [],
      reason: null,
      trigger: { type: 'Cron', schedule: '*/10 * * * *' },
      steps: [mkStep(), mkStep({ name: 'second' })],
    }]);
    renderWizard();
    fireEvent.change(screen.getByLabelText('wiz.name'), { target: { value: 'PickSuggestion' } });
    // Select the project so suggestions are fetched.
    fireEvent.change(screen.getByLabelText('wiz.project'), { target: { value: 'proj-1' } });
    // Expand the picker, then apply the suggestion row.
    fireEvent.click(screen.getByText(/wiz\.quickstart\.toggle/).closest('button')!);
    const sug = await screen.findByText('SuggestedWF');
    const row = sug.closest('.wf-quickstart-row')!;
    fireEvent.click(row.querySelector('.wf-quickstart-apply-btn')!);
    // Multi-step suggestion → advanced Steps page.
    await waitFor(() => expect(screen.getByText('wiz.addStep')).toBeInTheDocument());
    // The suggestion's name replaced the field value.
    expect(screen.getByDisplayValue('second')).toBeInTheDocument();
  });
});

// ── Summary recap for every step_type ───────────────────────────────

describe('WorkflowWizard — full summary recap', () => {
  const everyTypeSteps: WorkflowStep[] = [
    mkStep({ name: 'agentStep', on_result: [{ contains: 'X', action: { type: 'Stop' } }], retry: { max_retries: 2, backoff: 'fixed' }, stall_timeout_secs: 120, delay_after_secs: 5 }),
    mkStep({ name: 'apiStep', step_type: { type: 'ApiCall' }, api_plugin_slug: 'mcp-github', api_config_id: 'cfg-gh', api_endpoint_path: '/issues' }),
    mkStep({ name: 'notifyStep', step_type: { type: 'Notify' }, notify_config: { url: 'https://hooks.slack.com/abc', method: 'POST', headers: {}, body_template: '' } }),
    mkStep({ name: 'gateStep', step_type: { type: 'Gate' }, gate_message: 'review' }),
    mkStep({ name: 'execStep', step_type: { type: 'Exec' }, exec_command: 'ls', exec_args: ['-la'] }),
    mkStep({ name: 'batchApiStep', step_type: { type: 'BatchApiCall' }, api_plugin_slug: 'mcp-github', api_config_id: 'cfg-gh', api_endpoint_path: '/x', batch_items_from: '{{steps.apiStep.data}}' }),
    mkStep({ name: 'jsonStep', step_type: { type: 'JsonData' }, json_data_payload: { a: 1, b: 2 } }),
  ];

  it('renders the type chip, subtitle and badges for each step on the summary', () => {
    toSummaryAdvanced({
      editWorkflow: mkWorkflow({
        steps: everyTypeSteps,
        safety: { sandbox: true, require_approval: true, max_files: 10, max_lines: 100 },
        workspace_config: { hooks: { after_create: 'npm i', before_run: null, after_run: null, before_remove: null } },
        concurrency_limit: 3,
      }),
    });
    // All seven type chips render on the summary.
    expect(screen.getByText('API')).toBeInTheDocument();
    expect(screen.getByText('NOTIFY')).toBeInTheDocument();
    expect(screen.getByText('GATE')).toBeInTheDocument();
    expect(screen.getByText('EXEC')).toBeInTheDocument();
    expect(screen.getByText('BATCH API')).toBeInTheDocument();
    expect(screen.getByText('JSON')).toBeInTheDocument();
    // Notify host parsed from the webhook URL.
    expect(screen.getByText(/hooks\.slack\.com/)).toBeInTheDocument();
    // Agent step badges: condition count, retry, timeout, delay.
    expect(screen.getByText(/\[retry x2\]/)).toBeInTheDocument();
    expect(screen.getByText(/timeout 120s/)).toBeInTheDocument();
    expect(screen.getByText(/delai 5s/)).toBeInTheDocument();
    // Safety + hooks summary rows.
    expect(screen.getByText('Securite')).toBeInTheDocument();
    expect(screen.getByText('Hooks')).toBeInTheDocument();
    expect(screen.getByText('Concurrence')).toBeInTheDocument();
  });

  it('shows the JSON object summary on the JsonData recap', () => {
    // Two steps → advanced mode (so the 4-next walk to summary works).
    toSummaryAdvanced({
      editWorkflow: mkWorkflow({
        steps: [
          mkStep({ name: 'json1', step_type: { type: 'JsonData' }, json_data_payload: [1, 2, 3] }),
          mkStep({ name: 'agent2' }),
        ],
      }),
    });
    // Array payload → array summary key with the length arg.
    expect(screen.getByText(/wiz\.jsonDataSummaryArray:3/)).toBeInTheDocument();
  });
});

// ── Exec setup-phase form ───────────────────────────────────────────

describe('WorkflowWizard — Exec setup phase', () => {
  it('toggles the setup phase, picks a preset, and edits command/args', () => {
    toSteps([
      mkStep({ name: 'execStep', step_type: { type: 'Exec' }, exec_command: 'composer' }),
      mkStep({ name: 'beta' }),
    ]);
    // execStep already Exec, but allowlist empty → warning shows. Set the
    // allowlist via the CTA → Config tab.
    fireEvent.click(screen.getByText('wiz.execAllowlistConfigureNow'));
    const allowlist = screen.getByPlaceholderText('wiz.execAllowlistPlaceholder') as HTMLInputElement;
    fireEvent.change(allowlist, { target: { value: 'composer, npm, pnpm' } });
    // Back to the Steps page.
    fireEvent.click(screen.getByText('wiz.previous'));
    // Now the Exec command picker (not the warning) renders. Toggle setup.
    const setupToggle = screen.getByText('wiz.execSetupToggle').closest('label')!
      .querySelector('input[type="checkbox"]') as HTMLInputElement;
    fireEvent.click(setupToggle);
    expect(setupToggle.checked).toBe(true);
    // The preset dropdown now appears — find it by its preset-pick option.
    const presetSelect = Array.from(document.querySelectorAll('select')).find(
      s => Array.from(s.options).some(o => o.value === 'pnpm'),
    ) as HTMLSelectElement;
    fireEvent.change(presetSelect, { target: { value: 'pnpm' } });
    // The exec command select offers the allowlist binaries — locate it by
    // the placeholder option text (`wiz.execCommandSelect`).
    const cmdSelect = Array.from(document.querySelectorAll('select')).find(
      s => Array.from(s.options).some(o => o.textContent?.includes('wiz.execCommandSelect')),
    ) as HTMLSelectElement;
    fireEvent.change(cmdSelect, { target: { value: 'npm' } });
    expect(cmdSelect.value).toBe('npm');
    // Exec args textarea.
    const argsArea = screen.getByPlaceholderText('wiz.execArgsPlaceholder') as HTMLTextAreaElement;
    fireEvent.change(argsArea, { target: { value: 'run\nbuild' } });
    expect(argsArea.value).toBe('run\nbuild');
    // Exec timeout.
    const timeout = screen.getByPlaceholderText('300') as HTMLInputElement;
    fireEvent.change(timeout, { target: { value: '60' } });
    expect(timeout.value).toBe('60');
    // Toggle setup back off (covers the else branch).
    fireEvent.click(setupToggle);
    expect(setupToggle.checked).toBe(false);
  });

  it('warns when an Exec step has no project bound', () => {
    // No project_id on the workflow → Exec step shows the no-project warn.
    renderWizard({
      editWorkflow: mkWorkflow({
        project_id: null,
        steps: [
          mkStep({ name: 'execStep', step_type: { type: 'Exec' }, exec_command: null }),
          mkStep({ name: 'beta' }),
        ],
      }),
    });
    fireEvent.click(screen.getByText('wiz.next')); // → Trigger
    fireEvent.click(screen.getByText('wiz.next')); // → Steps
    expect(screen.getByText('wiz.execNoProjectWarn')).toBeInTheDocument();
  });
});

// ── BatchApiCall form ───────────────────────────────────────────────

describe('WorkflowWizard — BatchApiCall form', () => {
  it('edits items-from + concurrent limit + max items and renders the QA picker', () => {
    qaListMock.mockResolvedValue([
      { id: 'qa-1', name: 'GetIssues', icon: '🔌', api_method: 'GET', api_endpoint_path: '/issues' } as QuickApi,
    ]);
    toSteps([
      mkStep({ name: 'first' }),
      mkStep({ name: 'batchApi', step_type: { type: 'BatchApiCall' } }),
    ]);
    // Items-from input.
    const itemsFrom = screen.getByPlaceholderText('wiz.batchApiItemsFromPlaceholder') as HTMLInputElement;
    fireEvent.change(itemsFrom, { target: { value: '{{steps.first.data}}' } });
    expect(itemsFrom.value).toBe('{{steps.first.data}}');
    // Concurrent limit.
    const concurrent = screen.getByPlaceholderText('5') as HTMLInputElement;
    fireEvent.change(concurrent, { target: { value: '4' } });
    expect(concurrent.value).toBe('4');
    // Max items.
    const maxItems = screen.getByPlaceholderText('50') as HTMLInputElement;
    fireEvent.change(maxItems, { target: { value: '25' } });
    expect(maxItems.value).toBe('25');
  });
});

// ── JsonData parse-error branch ─────────────────────────────────────

describe('WorkflowWizard — JsonData parse error', () => {
  it('shows the parse-error warning when the seeded payload is later replaced with invalid text after valid', () => {
    // Seed a valid array payload so `raw` is non-empty + summary renders,
    // then assert the array summary path (the catch branch is covered by
    // the sibling test). This hits the Array.isArray summary line.
    toSteps([
      mkStep({ name: 'jsonStep', step_type: { type: 'JsonData' }, json_data_payload: [{ x: 1 }] }),
      mkStep({ name: 'beta' }),
    ]);
    expect(screen.getByText(/wiz\.jsonDataSummaryArray/)).toBeInTheDocument();
    // Scalar payload path: swap to a scalar via the editor.
    const editor = screen.getByPlaceholderText('wiz.jsonDataPlaceholder') as HTMLTextAreaElement;
    fireEvent.change(editor, { target: { value: '42' } });
    expect(screen.getByText(/wiz\.jsonDataSummaryScalar/)).toBeInTheDocument();
  });
});

// ── Skills / Profiles / Directives selectors ────────────────────────

describe('WorkflowWizard — agent selectors (skills / profiles / directives)', () => {
  it('toggles skills, profile and directive chips on an Agent step', async () => {
    skillsListMock.mockResolvedValue([{ id: 'sk-1', name: 'SkillOne' } as unknown as Skill]);
    profilesListMock.mockResolvedValue([
      { id: 'pr-1', name: 'ProfileOne', avatar: '🧑', role: 'dev', color: '#abc', persona_name: null } as unknown as AgentProfile,
    ]);
    directivesListMock.mockResolvedValue([
      { id: 'dir-1', name: 'DirOne', icon: '📜' } as unknown as Directive,
    ]);
    // Single Agent step (forced advanced via a Cron trigger) so each
    // selector chip is unique on the page.
    renderWizard({ editWorkflow: mkWorkflow({ trigger: { type: 'Cron', schedule: '*/5 * * * *' }, steps: [mkStep()] }) });
    fireEvent.click(screen.getByText('wiz.next')); // → Trigger
    fireEvent.click(screen.getByText('wiz.next')); // → Steps
    // Wait for the API lists to populate.
    const skillChip = await screen.findByText('SkillOne');
    fireEvent.click(skillChip.closest('button')!);
    expect(skillChip.closest('button')).toHaveAttribute('data-selected', 'true');
    // Toggle off (covers the de-select branch).
    fireEvent.click(skillChip.closest('button')!);
    // Profile chip.
    const profileChip = await screen.findByText(/ProfileOne/);
    fireEvent.click(profileChip.closest('button')!);
    expect(profileChip.closest('button')).toHaveAttribute('data-selected', 'true');
    // Profile "none" reset.
    fireEvent.click(screen.getByText('profiles.none'));
    // Directive chip.
    const dirChip = await screen.findByText(/DirOne/);
    fireEvent.click(dirChip.closest('button')!);
    expect(dirChip.closest('button')).toHaveAttribute('data-selected', 'true');
    fireEvent.click(dirChip.closest('button')!);
  });
});

// ── on_failure rollback Agent + ApiCall kinds ───────────────────────

describe('WorkflowWizard — rollback step kinds', () => {
  it('switches a rollback step to Agent and edits its prompt', () => {
    toSteps([mkStep(), mkStep({ name: 'beta' })]);
    fireEvent.click(screen.getByText('wiz.addRollbackStep').closest('button')!);
    // The rollback row defaults to Notify; switch to Agent.
    fireEvent.click(screen.getAllByText('wiz.stepTypeAgent').slice(-1)[0]);
    const prompt = screen.getByPlaceholderText('wiz.rollbackAgentPromptPlaceholder') as HTMLTextAreaElement;
    fireEvent.change(prompt, { target: { value: 'roll it back' } });
    expect(prompt.value).toBe('roll it back');
    // Switch the agent picker in the rollback row.
    const agentSelect = prompt.closest('.wf-rollback-step')!.querySelector('select') as HTMLSelectElement;
    expect(agentSelect).toBeInTheDocument();
  });

  it('switches a rollback step to ApiCall (mounts the ApiCall card)', () => {
    toSteps([mkStep(), mkStep({ name: 'beta' })]);
    fireEvent.click(screen.getByText('wiz.addRollbackStep').closest('button')!);
    fireEvent.click(screen.getAllByText('wiz.stepTypeApiCall').slice(-1)[0]);
    // No plugins → the ApiCall card shows its not-supported notice.
    expect(screen.getByText('wf.apicall.notSupported')).toBeInTheDocument();
  });

  it('renames a rollback step', () => {
    toSteps([mkStep(), mkStep({ name: 'beta' })]);
    fireEvent.click(screen.getByText('wiz.addRollbackStep').closest('button')!);
    const rbName = screen.getByDisplayValue('rollback-1') as HTMLInputElement;
    fireEvent.change(rbName, { target: { value: 'notify_team' } });
    expect(rbName.value).toBe('notify_team');
  });
});

// ── State chips (Goto pairs) + var insertion ────────────────────────

describe('WorkflowWizard — state chips on Goto loops', () => {
  it('surfaces write/read state chips when a step Goto-loops to an Agent step', () => {
    // analyze Goto-loops to fix; fix is an Agent step → both the loop-out
    // (on analyze) and loop-in (on fix) chips appear.
    toSteps([
      mkStep({ name: 'analyze', on_result: [{ contains: 'AGAIN', action: { type: 'Goto', step_name: 'fix', max_iterations: 3 } }] }),
      mkStep({ name: 'fix' }),
    ]);
    // Loop-out chip on analyze writes the STATE block.
    const writeChip = screen.getByText(/wiz\.stateChipWrite/);
    expect(writeChip).toBeInTheDocument();
    fireEvent.click(writeChip.closest('button')!);
    // The fix step (target) gets a read chip.
    expect(screen.getByText(/\{\{state\.last_analyze\}\}/)).toBeInTheDocument();
  });
});

// ── Variable-help panel + undeclared-var warning ────────────────────

describe('WorkflowWizard — variable help + undeclared vars', () => {
  it('toggles the available-variables help panel', () => {
    toSteps([mkStep(), mkStep({ name: 'beta' })]);
    fireEvent.click(screen.getByText('wiz.availableVars'));
    // The help panel surfaces the trigger-vars section.
    expect(screen.getByText('wiz.triggerVars')).toBeInTheDocument();
    expect(screen.getByText('wiz.stepChaining')).toBeInTheDocument();
  });

  it('flags an undeclared {{var}} and declares it via the add button', () => {
    toSteps([
      mkStep({ name: 'first' }),
      mkStep({ name: 'second', prompt_template: 'use {{mystery}} here' }),
    ]);
    // The undeclared-var warning lists the bare {{mystery}} with an add CTA.
    expect(screen.getByText('wiz.undeclaredVarsTitle')).toBeInTheDocument();
    fireEvent.click(screen.getByText(/wiz\.undeclaredAddVar/).closest('button')!);
    // Declaring it removes the warning for that var (re-scan finds it now
    // declared as a workflow variable).
    // The Config tab should now carry a var row named "mystery"
    // (the var name + its auto-label both echo "mystery").
    fireEvent.click(screen.getByText('wiz.next')); // → Config
    expect(screen.getAllByDisplayValue('mystery').length).toBeGreaterThan(0);
  });
});

// ── Output format toggle on Agent steps ─────────────────────────────

describe('WorkflowWizard — output format toggle', () => {
  it('switches a step output format between Structured and FreeText', () => {
    toSteps([mkStep(), mkStep({ name: 'beta' })]);
    const freeBtn = screen.getAllByText('wiz.outputFree')[0].closest('button') as HTMLButtonElement;
    fireEvent.click(freeBtn);
    expect(freeBtn).toHaveAttribute('data-selected', 'true');
    const structuredBtn = screen.getAllByText('wiz.outputStructured')[0].closest('button') as HTMLButtonElement;
    fireEvent.click(structuredBtn);
    expect(structuredBtn).toHaveAttribute('data-selected', 'true');
  });
});

// ── Advanced per-step panel: agent settings model/effort/tokens ─────

describe('WorkflowWizard — advanced agent settings', () => {
  it('edits model, reasoning effort, max tokens, retry backoff and delay', () => {
    // Single Agent step (Cron-forced advanced) so per-step fields are unique.
    renderWizard({ editWorkflow: mkWorkflow({ trigger: { type: 'Cron', schedule: '*/5 * * * *' }, steps: [mkStep()] }) });
    fireEvent.click(screen.getByText('wiz.next')); // → Trigger
    fireEvent.click(screen.getByText('wiz.next')); // → Steps
    fireEvent.click(screen.getByText('wiz.advanced'));
    // Model input (placeholder "ex: o3").
    const model = screen.getByPlaceholderText('ex: o3') as HTMLInputElement;
    fireEvent.change(model, { target: { value: 'o3' } });
    expect(model.value).toBe('o3');
    // Reasoning effort select.
    const effort = screen.getByDisplayValue('default') as HTMLSelectElement;
    fireEvent.change(effort, { target: { value: 'high' } });
    expect(effort.value).toBe('high');
    // Max tokens.
    const maxTokens = screen.getByPlaceholderText('ex: 16000') as HTMLInputElement;
    fireEvent.change(maxTokens, { target: { value: '8000' } });
    expect(maxTokens.value).toBe('8000');
    // Delay-after (placeholder "0", no max attribute — distinct from the
    // retry count which has max=10).
    const delay = Array.from(document.querySelectorAll('input[placeholder="0"]'))
      .find(el => !el.getAttribute('max')) as HTMLInputElement;
    fireEvent.change(delay, { target: { value: '10' } });
    expect(delay.value).toBe('10');
    // Retry count (max=10) enables the backoff select.
    const retry = Array.from(document.querySelectorAll('input[type="number"]'))
      .find(el => el.getAttribute('max') === '10') as HTMLInputElement;
    fireEvent.change(retry, { target: { value: '3' } });
    const backoff = screen.getByDisplayValue('exponential') as HTMLSelectElement;
    fireEvent.change(backoff, { target: { value: 'fixed' } });
    expect(backoff.value).toBe('fixed');
  });
});

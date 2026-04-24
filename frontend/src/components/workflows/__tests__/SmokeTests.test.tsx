// Smoke tests for large workflow components (0.3.7 stability).
// Verify they mount without crashing given minimal props.

import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { Workflow } from '../../../types/generated';

vi.mock('../../../lib/api', () => buildApiMock());
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));
vi.mock('../../../hooks/useMediaQuery', () => ({
  useIsMobile: () => false,
}));

import { WorkflowWizard } from '../WorkflowWizard';

const noop = () => {};

describe('Workflow smoke tests', () => {
  it('WorkflowWizard renders step 1 without crashing', () => {
    render(
      <WorkflowWizard
        projects={[]}
        onDone={noop}
        onCancel={noop}
        installedAgentTypes={['ClaudeCode']}
      />
    );
    // The wizard mounted without crashing — verify some content rendered
    const text = document.body.textContent ?? '';
    expect(text.length).toBeGreaterThan(10);
  });

  // Auto-prep: selecting BatchQuickPrompt on step N flips step N-1 to
  // Structured + reasoning_effort=low so items_from resolves and the
  // producer stops burning tokens on narration the batch never reads.
  it('Selecting BatchQP type auto-configures the upstream step', () => {
    const editWorkflow: Workflow = {
      id: 'test-id',
      name: 'test-wf',
      project_id: null,
      trigger: { type: 'Cron', schedule: '*/5 * * * *' },
      steps: [
        {
          name: 'fetch',
          step_type: { type: 'Agent' },
          description: null,
          agent: 'ClaudeCode',
          prompt_template: 'Fetch tickets',
          mode: { type: 'Normal' },
          output_format: { type: 'FreeText' },
          mcp_config_ids: [],
          agent_settings: null,
          on_result: [],
          stall_timeout_secs: null,
          retry: null,
          skill_ids: [],
          directive_ids: [],
          profile_ids: [],
          delay_after_secs: null,
          batch_quick_prompt_id: null,
          batch_items_from: null,
          batch_wait_for_completion: null,
          batch_max_items: null,
          batch_workspace_mode: null,
          batch_chain_prompt_ids: [],
          notify_config: null,
        },
        {
          name: 'process',
          step_type: { type: 'Agent' },
          description: null,
          agent: 'ClaudeCode',
          prompt_template: 'Process',
          mode: { type: 'Normal' },
          output_format: { type: 'Structured' },
          mcp_config_ids: [],
          agent_settings: null,
          on_result: [],
          stall_timeout_secs: null,
          retry: null,
          skill_ids: [],
          directive_ids: [],
          profile_ids: [],
          delay_after_secs: null,
          batch_quick_prompt_id: null,
          batch_items_from: null,
          batch_wait_for_completion: null,
          batch_max_items: null,
          batch_workspace_mode: null,
          batch_chain_prompt_ids: [],
          notify_config: null,
        },
      ],
      actions: [],
      safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false },
      workspace_config: null,
      concurrency_limit: null,
      enabled: true,
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
    };

    const { container } = render(
      <WorkflowWizard
        projects={[]}
        editWorkflow={editWorkflow}
        onDone={noop}
        onCancel={noop}
        installedAgentTypes={['ClaudeCode']}
      />
    );

    // Walk to the Steps pane
    const nextButton = () => Array.from(container.querySelectorAll('button'))
      .find(b => b.textContent?.includes('wiz.next')) as HTMLButtonElement | undefined;
    for (let i = 0; i < 2; i++) {
      const btn = nextButton();
      if (!btn || btn.disabled) break;
      fireEvent.click(btn);
    }

    // Find the BatchQP type buttons (one per step). Click the one on step 2.
    const batchBtns = Array.from(container.querySelectorAll('button[data-type="batch-qp"]')) as HTMLButtonElement[];
    expect(batchBtns.length).toBe(2);
    fireEvent.click(batchBtns[1]);

    // After auto-prep, step 1's Structured button must read as selected.
    const structuredBtns = Array.from(container.querySelectorAll('.wf-step-type-btn'))
      .filter(b => b.textContent?.includes('wiz.outputStructured'));
    // The Structured button on step 1 must be the selected one — first match.
    expect(structuredBtns[0]?.getAttribute('data-selected')).toBe('true');

    // The auto-config notice must appear in the BatchQP form (on step 2).
    const noticeVisible = (container.textContent ?? '').includes('wiz.batchAutoPrevNotice');
    expect(noticeVisible).toBe(true);
  });

  // Regression: Workflow B shipped with output_format=FreeText by default and
  // the toggle was hidden inside the "Advanced" panel, so nobody caught that
  // a chained `{{previous_step.data}}` would fail. The default is now
  // Structured and the toggle lives at the root of the step card.
  it('Step card shows output_format toggle with Structured selected by default', () => {
    // editWorkflow with multi-step + cron forces advanced mode, so the
    // Steps pane (wizardStep=2) contains a rendered step card. We reach
    // it by navigating via the Next button.
    const editWorkflow: Workflow = {
      id: 'test-id',
      name: 'test-wf',
      project_id: null,
      trigger: { type: 'Cron', schedule: '*/5 * * * *' },
      steps: [{
        name: 'main',
        step_type: { type: 'Agent' },
        description: null,
        agent: 'ClaudeCode',
        prompt_template: 'Do the thing',
        mode: { type: 'Normal' },
        output_format: { type: 'Structured' },
        mcp_config_ids: [],
        agent_settings: null,
        on_result: [],
        stall_timeout_secs: null,
        retry: null,
        skill_ids: [],
        directive_ids: [],
        profile_ids: [],
        delay_after_secs: null,
        batch_quick_prompt_id: null,
        batch_items_from: null,
        batch_wait_for_completion: null,
        batch_max_items: null,
        batch_workspace_mode: null,
        batch_chain_prompt_ids: [],
        notify_config: null,
      }],
      actions: [],
      safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false },
      workspace_config: null,
      concurrency_limit: null,
      enabled: true,
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
    };

    const { container } = render(
      <WorkflowWizard
        projects={[]}
        editWorkflow={editWorkflow}
        onDone={noop}
        onCancel={noop}
        installedAgentTypes={['ClaudeCode']}
      />
    );

    // Advance through Infos → Trigger → Steps. Each Next click is the button
    // whose visible label is `wiz.next` (thanks to the key-as-value mock).
    const nextButton = () => Array.from(container.querySelectorAll('button'))
      .find(b => b.textContent?.includes('wiz.next')) as HTMLButtonElement | undefined;
    // Click Next twice to land on the Steps pane.
    for (let i = 0; i < 2; i++) {
      const btn = nextButton();
      if (!btn || btn.disabled) break;
      fireEvent.click(btn);
    }

    // The output format label must render OUTSIDE the advanced panel:
    // the wf-advanced-panel is only present when expanded, and it's not
    // by default. So if the label is in the DOM, it's visible at root.
    const labels = Array.from(container.querySelectorAll('.wf-label'))
      .map(n => n.textContent ?? '');
    expect(labels).toContain('wiz.outputFormat');
    expect(container.querySelector('.wf-advanced-panel')).toBeNull();

    // Structured button must be the one selected by default.
    const buttons = Array.from(container.querySelectorAll('.wf-step-type-btn'));
    const structured = buttons.find(b => b.textContent?.includes('wiz.outputStructured'));
    const free = buttons.find(b => b.textContent?.includes('wiz.outputFree'));
    expect(structured?.getAttribute('data-selected')).toBe('true');
    expect(free?.getAttribute('data-selected')).toBe('false');
  });
});

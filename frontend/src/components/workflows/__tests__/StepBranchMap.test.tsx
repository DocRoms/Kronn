import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { StepBranchMap } from '../StepBranchMap';
import type { WorkflowStep } from '../../../types/generated';

const t = (key: string, ...args: (string | number)[]) => (args.length ? `${key}:${args.join(',')}` : key);

const step = (name: string, gotos: Array<{ contains: string; to: string }> = []): WorkflowStep => ({
  name, step_type: { type: 'Exec' }, description: null, agent: 'ClaudeCode', prompt_template: '',
  mode: { type: 'Normal' }, output_format: { type: 'FreeText' }, mcp_config_ids: [], agent_settings: null,
  on_result: gotos.map(g => ({ contains: g.contains, action: { type: 'Goto', step_name: g.to } })),
  stall_timeout_secs: null, retry: null, delay_after_secs: null, skill_ids: [], profile_ids: [], directive_ids: [],
  batch_quick_prompt_id: null, batch_items_from: null, batch_wait_for_completion: null, batch_max_items: null,
  batch_workspace_mode: null, batch_chain_prompt_ids: [], notify_config: null, api_plugin_slug: null,
  api_config_id: null, api_endpoint_path: null, api_method: null, api_query: null, api_headers: null,
  api_body: null, api_extract: null, api_pagination: null, api_timeout_ms: null, api_max_retries: null,
  api_output_var: null, gate_message: null, gate_request_changes_target: null, gate_notify_url: null,
  exec_command: null, exec_args: [], exec_timeout_secs: null,
} as unknown as WorkflowStep);

describe('StepBranchMap', () => {
  it('renders nothing for a purely linear workflow', () => {
    const { container } = render(<StepBranchMap steps={[step('a'), step('b')]} t={t} />);
    expect(container.querySelector('[data-testid="wf-branch-map"]')).toBeNull();
  });

  it('draws one arc per Goto edge (skipping dangling targets)', () => {
    const steps = [
      step('a'),
      step('b', [{ contains: 'ERROR', to: 'd' }]),
      step('c', [{ contains: 'RETRY', to: 'a' }, { contains: 'X', to: 'ghost' }]), // ghost = dangling, not drawn
      step('d'),
    ];
    render(<StepBranchMap steps={steps} t={t} />);
    // 3 edges total, but the dangling 'ghost' one isn't drawn → 2 arcs.
    expect(screen.getAllByTestId('wf-bm-arc')).toHaveLength(2);
    // A node label per step.
    expect(screen.getByText(/1\. a/)).toBeInTheDocument();
    expect(screen.getByText(/4\. d/)).toBeInTheDocument();
  });
});

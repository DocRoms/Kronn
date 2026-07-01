import { describe, it, expect } from 'vitest';
import { computeGotoEdges, hasBranches } from '../stepGraph';
import type { WorkflowStep } from '../../types/generated';

const step = (name: string, gotos: Array<{ contains: string; to: string }> = []): WorkflowStep => ({
  name,
  step_type: { type: 'Exec' },
  description: null, agent: 'ClaudeCode', prompt_template: '', mode: { type: 'Normal' },
  output_format: { type: 'FreeText' }, mcp_config_ids: [],
  agent_settings: null,
  on_result: gotos.map(g => ({ contains: g.contains, action: { type: 'Goto', step_name: g.to } })),
  stall_timeout_secs: null, retry: null, delay_after_secs: null,
  skill_ids: [], profile_ids: [], directive_ids: [],
  batch_quick_prompt_id: null, batch_items_from: null, batch_wait_for_completion: null,
  batch_max_items: null, batch_workspace_mode: null, batch_chain_prompt_ids: [],
  notify_config: null, api_plugin_slug: null, api_config_id: null, api_endpoint_path: null,
  api_method: null, api_query: null, api_headers: null, api_body: null, api_extract: null,
  api_pagination: null, api_timeout_ms: null, api_max_retries: null, api_output_var: null,
  gate_message: null, gate_request_changes_target: null, gate_notify_url: null,
  exec_command: null, exec_args: [], exec_timeout_secs: null,
} as unknown as WorkflowStep);

describe('computeGotoEdges', () => {
  it('resolves Goto targets to indices and flags backward loops', () => {
    const steps = [
      step('a'),
      step('b', [{ contains: 'ERROR', to: 'd' }]),   // forward b(1)→d(3)
      step('c', [{ contains: 'RETRY', to: 'a' }]),   // backward c(2)→a(0)
      step('d'),
    ];
    const edges = computeGotoEdges(steps);
    expect(edges).toHaveLength(2);
    const bd = edges.find(e => e.fromName === 'b')!;
    expect(bd.toIndex).toBe(3);
    expect(bd.label).toBe('ERROR');
    expect(bd.backward).toBe(false);
    const ca = edges.find(e => e.fromName === 'c')!;
    expect(ca.toIndex).toBe(0);
    expect(ca.backward).toBe(true);
  });

  it('marks a dangling Goto target as index -1', () => {
    const edges = computeGotoEdges([step('a', [{ contains: 'X', to: 'ghost' }])]);
    expect(edges[0].toIndex).toBe(-1);
    expect(edges[0].toName).toBe('ghost');
  });

  it('hasBranches is false for a purely linear workflow', () => {
    expect(hasBranches([step('a'), step('b'), step('c')])).toBe(false);
    expect(hasBranches([step('a', [{ contains: 'E', to: 'a' }])])).toBe(true);
  });
});

import { useState } from 'react';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../test/apiMock';
import type { AgentType, CatalogModelEntry, ModelCatalogView, ModelTier } from '../../types/generated';

const mocks = vi.hoisted(() => ({ list: vi.fn(), workflow: vi.fn(), detect: vi.fn(), access: vi.fn() }));
vi.mock('../../lib/api', () => buildApiMock({
  modelCatalogApi: { list: mocks.list as never },
  config: { getAgentAccess: mocks.access as never },
  workflows: { get: mocks.workflow as never },
  agents: { detect: mocks.detect as never },
}));
vi.mock('../../lib/I18nContext', () => ({ useT: () => ({ t: (key: string) => key }) }));
import { WorkflowStepAgents, type StepAgentChoices } from '../WorkflowStepAgents';

const TARGETS: Record<string, { runtime: string; prefix: string }> = {
  Codex: { runtime: 'agent:codex', prefix: 'fixture' },
  ClaudeCode: { runtime: 'agent:claude-code', prefix: 'claude' },
};
const MODES: Record<ModelTier, string> = { economy: 'minimal', default: 'low', reasoning: 'xhigh' };

function entry(agent: AgentType, tier: ModelTier): CatalogModelEntry {
  const { runtime, prefix } = TARGETS[agent];
  return {
    id: `${prefix}-catalog-${tier}`, model_id: `${prefix}-${tier}`, display_name: `${prefix} ${tier}`,
    runtime_target_id: runtime, agent_type: agent, provenance: 'live',
    availability: 'available', capabilities: ['chat'], reasoning_modes: [MODES[tier]],
    default_reasoning_mode: MODES[tier], tier_assignment: tier, manual_origin: false,
    first_seen_at: '2026-10-05T00:00:00Z', last_checked_at: '2026-10-05T00:00:00Z',
    created_at: '2026-10-05T00:00:00Z', updated_at: '2026-10-05T00:00:00Z',
  };
}

function view(agent: AgentType): ModelCatalogView {
  return {
    runtime_target_id: TARGETS[agent].runtime, agent_type: agent, stale: false, live_refresh_ok: true,
    models: (['economy', 'default', 'reasoning'] as ModelTier[]).map(tier => entry(agent, tier)),
  };
}

beforeEach(() => {
  mocks.list.mockReset().mockResolvedValue({ targets: [view('Codex'), view('ClaudeCode')] });
  mocks.detect.mockReset().mockResolvedValue([
    { agent_type: 'Codex', enabled: true, installed: true, runtime_available: true },
    { agent_type: 'ClaudeCode', enabled: true, installed: true, runtime_available: true },
    { agent_type: 'CopilotCli', enabled: true, installed: true, runtime_available: true },
  ]);
  mocks.access.mockReset().mockResolvedValue({ copilot_cli: { full_access: false } });
});
afterEach(cleanup);

function Harness({ onChange }: { onChange: (next: StepAgentChoices) => void }) {
  const [value, setValue] = useState<StepAgentChoices>({});
  return <WorkflowStepAgents workflowId="wf" value={value} onChange={next => { onChange(next); setValue(next); }} />;
}

async function openStep(tier: ModelTier, model?: string) {
  mocks.workflow.mockResolvedValue({ id: 'wf', steps: [{
    id: 'step-1', name: 'Draft', step_type: { type: 'Agent' }, agent: 'Codex',
    agent_settings: { tier, ...(model ? { model } : {}) },
  }] });
  const changed = vi.fn();
  render(<Harness onChange={changed} />);
  await act(async () => {});
  const details = await screen.findByTestId('action-card-step-agents');
  fireEvent.click(details.querySelector('summary')!);
  return changed;
}

const modelBox = () => screen.getByRole('combobox', { name: 'wiz.model' });

function effortOptions(): string[] {
  fireEvent.focus(screen.getByRole('combobox', { name: 'wiz.reasoningEffort' }));
  return screen.getAllByRole('option').map(option => option.textContent ?? '');
}

describe('KT-1095 — the launch card follows the step tier with the real catalogue selectors', () => {
  it.each(['default', 'economy', 'reasoning'] as ModelTier[])(
    'shows the %s-tier model and its effort for a step with no explicit model',
    async tier => {
      const changed = await openStep(tier);
      expect(modelBox()).toHaveAttribute('placeholder', `fixture-${tier}`);
      expect(effortOptions()).toContain(MODES[tier]);
      expect(effortOptions()).not.toContain(MODES[tier === 'default' ? 'reasoning' : 'default']);
      // Showing the plan is not a choice: nothing goes into the launch payload.
      expect(changed).not.toHaveBeenCalled();
    },
  );

  it('lets an explicit step model win over its tier', async () => {
    await openStep('reasoning', 'fixture-economy');
    expect(modelBox()).toHaveValue('fixture economy');
    expect(effortOptions()).toContain('minimal');
    expect(effortOptions()).not.toContain('xhigh');
  });

  it('keeps the step tier after switching the agent', async () => {
    const changed = await openStep('reasoning');
    fireEvent.click(screen.getByRole('button', { name: 'disc.action.stepAgents.agentFor' }));
    const claude = screen.getAllByRole('menuitem').find(item => /claude/i.test(item.textContent ?? ''));
    await act(async () => { fireEvent.click(claude!); });
    expect(changed).toHaveBeenLastCalledWith({ 'step-1': { agent: 'ClaudeCode' } });
    expect(modelBox()).toHaveAttribute('placeholder', 'claude-reasoning');
    expect(effortOptions()).toContain('xhigh');
  });
});

describe('KT-1150 — the launch selector badges agents that need full access', () => {
  async function openCopilotStep(fullAccess: boolean) {
    mocks.access.mockResolvedValue({ copilot_cli: { full_access: fullAccess } });
    mocks.workflow.mockResolvedValue({ id: 'wf', steps: [{
      id: 'step-1', name: 'Draft', step_type: { type: 'Agent' }, agent: 'CopilotCli',
      agent_settings: { tier: 'default' },
    }] });
    render(<Harness onChange={vi.fn()} />);
    await act(async () => {});
    fireEvent.click((await screen.findByTestId('action-card-step-agents')).querySelector('summary')!);
    fireEvent.click(screen.getByRole('button', { name: 'disc.action.stepAgents.agentFor' }));
  }

  it('badges a native agent whose full access is off', async () => {
    await openCopilotStep(false);
    const copilot = screen.getAllByRole('menuitem').find(item => /copilot/i.test(item.textContent ?? ''))!;
    expect(copilot).toHaveTextContent('agentPicker.needsFullAccess');
    const claude = screen.getAllByRole('menuitem').find(item => /claude/i.test(item.textContent ?? ''))!;
    expect(claude).not.toHaveTextContent('agentPicker.needsFullAccess');
  });

  it('shows no badge once full access is on', async () => {
    await openCopilotStep(true);
    expect(screen.queryByTestId('agent-needs-full-access')).toBeNull();
  });
});

/**
 * KT-1154 — a mentioned agent that would fail says so while the message is
 * still being written: an endpoint the last probe could not reach, or a model
 * the catalogue refuses, with the date of that refusal.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';
import { ChatInput } from '../ChatInput';
import type { AgentDetection, Discussion, ModelCatalogSnapshot } from '../../types/generated';
import { agents as agentsApi, discussions as discussionsApi, modelCatalogApi } from '../../lib/api';
import { clearReachabilityCache } from '../../hooks/useTargetReachability';

vi.mock('../../lib/api', () => buildApiMock({
  discussions: { participants: vi.fn().mockResolvedValue([]), nativeAgentMode: vi.fn().mockResolvedValue({ disabled: false }) },
}));

vi.mock('../../lib/stt-engine', () => ({
  audioBufferToFloat32: vi.fn(),
  transcribeAudio: vi.fn().mockResolvedValue(''),
}));

const discussion = {
  id: 'd-target-warnings',
  title: 'Target warnings',
  project_id: null,
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: [],
  message_count: 0,
  non_system_message_count: 0,
  skill_ids: [],
  profile_ids: [],
  directive_ids: [],
  archived: false,
  pinned: false,
  workspace_mode: 'Direct',
  workspace_path: null,
  worktree_branch: null,
  tier: 'default',
  pin_first_message: false,
  summary_cache: null,
  summary_up_to_msg_idx: null,
  shared_id: null,
  shared_with: [],
  workflow_run_id: null,
  created_at: '2026-10-10T09:00:00Z',
  updated_at: '2026-10-10T09:00:00Z',
} as unknown as Discussion;

const agents = [
  { agent_type: 'LiteLlm', installed: false, runtime_available: true, enabled: true },
  { agent_type: 'Codex', installed: true, runtime_available: false, enabled: true },
] as AgentDetection[];

/** Keeps the arguments visible, so the test reads what the chip would say. */
const t = (key: string, ...args: (string | number)[]) => [key, ...args].join('|');

function renderInput() {
  return render(
    <ChatInput
      discussion={discussion}
      agents={agents}
      sending={false}
      disabled={false}
      ttsEnabled={false}
      ttsState="idle"
      worktreeError={null}
      availableSkills={[]}
      availableDirectives={[]}
      onSend={vi.fn()}
      onStop={vi.fn()}
      onOrchestrate={vi.fn()}
      onTtsToggle={vi.fn()}
      onWorktreeErrorDismiss={vi.fn()}
      onWorktreeRetry={vi.fn()}
      isAgentRestricted={() => false}
      contextFiles={[]}
      uploadingFiles={false}
      toast={vi.fn() as never}
      t={t}
    />,
  );
}

function type(value: string) {
  fireEvent.change(screen.getByRole('textbox'), { target: { value } });
}

function deniedCatalog(): ModelCatalogSnapshot {
  return {
    targets: [{
      runtime_target_id: 'http:external-api-litellm',
      agent_type: 'LiteLlm',
      models: [{
        runtime_target_id: 'http:external-api-litellm',
        model_id: 'vertex_ai/claude-fable-5@default',
        display_name: 'vertex_ai/claude-fable-5@default',
        tier_assignment: 'default',
        availability: 'unavailable',
        unavailable_reason: 'access_denied',
        unavailable_detail: 'Access denied (HTTP 401)',
        last_checked_at: '2026-10-06T08:15:00Z',
        provenance: 'live',
        reasoning_modes: [],
        capabilities: ['chat'],
      }],
      tier_verdicts: [],
      alerts: [],
      stale: false,
      live_refresh_ok: true,
    }],
  } as unknown as ModelCatalogSnapshot;
}

describe('composer — mention warnings (KT-1154)', () => {
  beforeEach(() => {
    localStorage.clear();
    clearReachabilityCache();
    vi.mocked(agentsApi.reachability).mockReset().mockResolvedValue([]);
    vi.mocked(modelCatalogApi.list).mockReset().mockResolvedValue({ targets: [] } as ModelCatalogSnapshot);
    vi.spyOn(discussionsApi, 'participants').mockResolvedValue([]);
    vi.spyOn(discussionsApi, 'nativeAgentMode').mockResolvedValue({ disabled: false });
  });

  it('warns that a mentioned endpoint is unreachable, with the probe reason', async () => {
    vi.mocked(agentsApi.reachability).mockResolvedValue([{
      agent_type: 'LiteLlm',
      connection_id: null,
      connectivity: { state: 'unreachable', unreachable_reason: 'dns', checked_at: '2026-10-10T12:00:00Z' },
    }]);
    renderInput();
    type('@litellm une blague ');

    const warnings = await screen.findByTestId('composer-target-warnings');
    expect(warnings.textContent).toContain('disc.targetWarning.unreachable|@litellm|disc.targetWarning.reason.dns');
    expect(warnings.querySelector('[data-warning="unreachable"]')).not.toBeNull();
    expect(agentsApi.reachability).toHaveBeenCalledWith([{ agent_type: 'LiteLlm', connection_id: null }]);
  });

  it('shows a stored catalogue refusal with its date', async () => {
    vi.mocked(modelCatalogApi.list).mockResolvedValue(deniedCatalog());
    renderInput();
    type('@litellm une blague ');

    const warnings = await screen.findByTestId('composer-target-warnings');
    const text = warnings.textContent ?? '';
    expect(text).toContain('disc.targetWarning.refused|@litellm|vertex_ai/claude-fable-5@default');
    expect(text).toContain('modelCatalog.reason.access_denied');
    expect(text).toContain('2026-10-06');
    expect(warnings.querySelector('[data-warning="refused"]')).not.toBeNull();
  });

  it('flags the agent among several, and leaves the healthy one alone', async () => {
    vi.mocked(agentsApi.reachability).mockResolvedValue([{
      agent_type: 'LiteLlm',
      connection_id: null,
      connectivity: { state: 'unreachable', unreachable_reason: 'refused', checked_at: '2026-10-10T12:00:00Z' },
    }]);
    renderInput();
    type('@litellm @codex une blague ');

    await screen.findByTestId('composer-target-warnings');
    const chips = Array.from(document.querySelectorAll('.disc-composer-parallel-chip'));
    const flagged = chips.filter(chip => chip.getAttribute('data-warning') === 'unreachable');
    expect(flagged.map(chip => chip.textContent)).toEqual(['@litellm']);
    expect(chips.some(chip => chip.textContent === '@codex' && !chip.hasAttribute('data-warning'))).toBe(true);
  });

  it('says nothing for a reachable endpoint', async () => {
    vi.mocked(agentsApi.reachability).mockResolvedValue([{
      agent_type: 'LiteLlm',
      connection_id: null,
      connectivity: { state: 'verified', http_status: 401, checked_at: '2026-10-10T12:00:00Z' },
    }]);
    renderInput();
    type('@litellm une blague ');

    await waitFor(() => expect(agentsApi.reachability).toHaveBeenCalledTimes(1));
    expect(screen.queryByTestId('composer-target-warnings')).toBeNull();
  });

  it('asks once per set of agents, not on every keystroke', async () => {
    renderInput();
    type('@litellm u');
    type('@litellm un');
    type('@litellm une blague');
    await waitFor(() => expect(agentsApi.reachability).toHaveBeenCalledTimes(1));
    // A CLI-only draft has nothing to probe.
    type('@codex une blague');
    await Promise.resolve();
    expect(agentsApi.reachability).toHaveBeenCalledTimes(1);
  });
});

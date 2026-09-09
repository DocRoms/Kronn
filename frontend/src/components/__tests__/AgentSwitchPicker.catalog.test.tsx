import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../test/apiMock';
import type { CatalogModelEntry, ModelTiersConfig } from '../../types/generated';

const { list } = vi.hoisted(() => ({ list: vi.fn() }));
vi.mock('../../lib/api', () => buildApiMock({ modelCatalogApi: { list: list as never } }));
import { AgentSwitchPicker } from '../AgentSwitchPicker';

function model(id: string, tier: CatalogModelEntry['tier_assignment'] = null): CatalogModelEntry {
  return { id, runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode',
    model_id: id, display_name: id, tier_assignment: tier,
    provenance: 'live', availability: 'available', capabilities: ['chat'],
    reasoning_modes: [], manual_origin: false, first_seen_at: '2026-09-09T00:00:00Z',
    last_checked_at: '2026-09-09T00:00:00Z', created_at: '2026-09-09T00:00:00Z', updated_at: '2026-09-09T00:00:00Z' };
}

function config(reasoning: string | null): ModelTiersConfig {
  const empty = () => ({ economy: null, default: null, reasoning: null });
  return { claude_code: { ...empty(), reasoning }, codex: empty(), open_code: empty(),
    gemini_cli: empty(), kiro: empty(), vibe: empty(), copilot_cli: empty(),
    ollama: empty(), lite_llm: empty(), nvidia: empty() };
}

async function show(configured: string | null = null) {
  render(<AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={['ClaudeCode']} currentTier="default"
    modelTiers={config(configured)} onSelectionChange={vi.fn()} title="Choose" ariaLabel="Choose" />);
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Choose' })); });
}

beforeEach(() => {
  list.mockReset().mockResolvedValue({ targets: [{
    runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode', stale: false, live_refresh_ok: true,
    models: [model('catalog-assignment', 'reasoning'), model('operator-choice')],
  }] });
});
afterEach(cleanup);

describe('AgentSwitchPicker — configured identity and catalogue provenance', () => {
  it('uses the explicit tier override before a different catalogue assignment', async () => {
    await show('operator-choice');
    await screen.findByText('modelCatalog.provenance.live');
    expect(screen.getByRole('menuitem', { name: /Claude Code.*reasoning/ }))
      .toHaveAttribute('title', 'reasoning · operator-choice');
  });

  it('never silently substitutes the catalogue assignment for an unknown configured model', async () => {
    await show('operator-model-not-in-catalog');
    await waitFor(() => expect(list).toHaveBeenCalled());
    expect(screen.getByRole('menuitem', { name: /Claude Code.*reasoning/ }))
      .toHaveAttribute('title', 'reasoning · operator-model-not-in-catalog');
  });

  it('labels a stale live entry as cached instead of presenting a fresh discovery', async () => {
    list.mockResolvedValue({ targets: [{
      runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode', stale: true, live_refresh_ok: false,
      models: [model('catalog-assignment', 'reasoning')],
    }] });
    await show();
    expect(await screen.findByText('modelCatalog.provenance.cached')).toBeInTheDocument();
    expect(screen.queryByText('modelCatalog.provenance.live')).not.toBeInTheDocument();
  });

  it('keeps an explicitly configured unavailable identity disabled even when an assigned alternative exists', async () => {
    list.mockResolvedValue({ targets: [{
      runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode', stale: false, live_refresh_ok: true,
      models: [model('catalog-assignment', 'reasoning'), { ...model('operator-choice'), availability: 'unavailable' }],
    }] });
    await show('operator-choice');
    await waitFor(() => expect(screen.getByRole('menuitem', { name: /Claude Code.*reasoning/ })).toBeDisabled());
    expect(screen.getByRole('menuitem', { name: /Claude Code.*reasoning/ })).toHaveTextContent('modelCatalog.unavailable');
  });

  it('does not inherit another HTTP connection’s configured model', async () => {
    list.mockResolvedValue({ targets: [] });
    const tiers = config(null);
    tiers.lite_llm.reasoning = 'other-connection-model';
    render(<AgentSwitchPicker currentAgent="LiteLlm" availableAgents={['LiteLlm']} currentTier="default"
      currentConnectionId="team-a" availableTargets={[{ agent: 'LiteLlm', connectionId: 'team-a', label: 'Team A' }]}
      modelTiers={tiers} onTargetSelectionChange={vi.fn()} title="Choose" ariaLabel="Choose" />);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Choose' })); });
    await waitFor(() => expect(list).toHaveBeenCalled());
    expect(screen.getByRole('menuitem', { name: /Team A.*reasoning/ }))
      .toHaveAttribute('title', 'reasoning · Default agent model');
  });

  it('rejects a model entry belonging to a different runtime inside the target view', async () => {
    list.mockResolvedValue({ targets: [{
      runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode', stale: false, live_refresh_ok: true,
      models: [{ ...model('foreign-model', 'reasoning'), runtime_target_id: 'http:other' }],
    }] });
    await show();
    await waitFor(() => expect(list).toHaveBeenCalled());
    expect(screen.getByRole('menuitem', { name: /Claude Code.*reasoning/ }))
      .toHaveAttribute('title', 'reasoning · Default agent model');
    expect(screen.queryByText('modelCatalog.provenance.live')).not.toBeInTheDocument();
  });

  it('retains a snapshot as cached and reports a failed reload', async () => {
    await show();
    await screen.findByText('modelCatalog.provenance.live');
    fireEvent.keyDown(document, { key: 'Escape' });
    list.mockRejectedValue(new Error('offline'));
    fireEvent.click(screen.getByRole('button', { name: 'Choose' }));
    expect(await screen.findByText('modelCatalog.loadError')).toBeInTheDocument();
    expect(screen.getByText('modelCatalog.provenance.cached')).toBeInTheDocument();
    expect(screen.queryByText('modelCatalog.provenance.live')).not.toBeInTheDocument();
  });

  it('shows a saved per-item override only for the current selection', async () => {
    render(<AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={['ClaudeCode']} currentTier="default"
      currentModel="saved-explicit-model" modelTiers={config('operator-choice')}
      onSelectionChange={vi.fn()} title="Choose" ariaLabel="Choose" />);
    const trigger = screen.getByRole('button', { name: 'Choose' });
    expect(trigger).toHaveAttribute('title', 'Choose · default · saved-explicit-model');
    await act(async () => { fireEvent.click(trigger); });
    expect(screen.getByRole('menuitem', { name: /Claude Code.*default/ }))
      .toHaveAttribute('title', 'default · saved-explicit-model');
    expect(screen.getByRole('menuitem', { name: /Claude Code.*reasoning/ }))
      .toHaveAttribute('title', 'reasoning · operator-choice');
  });

  it('searches labels, exact connection identities and model IDs without refetching or selecting', async () => {
    const select = vi.fn();
    list.mockResolvedValue({ targets: [{
      runtime_target_id: 'http:team-a', agent_type: 'Custom', stale: false, live_refresh_ok: true,
      models: [{ ...model('vendor/model-rare', 'default'), runtime_target_id: 'http:team-a',
        agent_type: 'Custom', display_alias: 'Café modèle' }],
    }] });
    render(<AgentSwitchPicker currentAgent="Codex" availableAgents={['Codex', 'Custom']} currentTier="default"
      availableTargets={[{ agent: 'Codex' }, { agent: 'Custom', connectionId: 'team-a', label: 'Équipe A' },
        { agent: 'Custom', connectionId: 'team-b', label: 'Équipe B' }]}
      onTargetSelectionChange={select} title="Choose" ariaLabel="Choose" />);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Choose' })); });
    const search = screen.getByRole('searchbox', { name: 'agentPicker.search' });
    expect(search).toHaveFocus();
    for (const query of ['CAFE', 'vendor/model-rare', 'team-a', 'equipe a']) {
      fireEvent.change(search, { target: { value: query } });
      expect(screen.getByRole('group', { name: 'Équipe A' })).toBeInTheDocument();
      expect(screen.queryByRole('group', { name: 'Équipe B' })).not.toBeInTheDocument();
      expect(screen.queryByRole('group', { name: 'Codex' })).not.toBeInTheDocument();
    }
    fireEvent.change(search, { target: { value: 'Codex' } });
    expect(screen.getByRole('group', { name: 'Codex' })).toBeInTheDocument();
    expect(screen.queryByRole('group', { name: 'Équipe A' })).not.toBeInTheDocument();
    expect(list).toHaveBeenCalledTimes(1);
    expect(select).not.toHaveBeenCalled();
  });

  it('keeps an unknown configured identity searchable without substituting it', async () => {
    await show('operator-未知-model');
    fireEvent.change(screen.getByRole('searchbox', { name: 'agentPicker.search' }),
      { target: { value: '未知' } });
    expect(screen.getByRole('menuitem', { name: /Claude Code.*reasoning/ }))
      .toHaveAttribute('title', 'reasoning · operator-未知-model');
  });

  it('reports no matches and resets the query when reopened', async () => {
    await show();
    fireEvent.change(screen.getByRole('searchbox', { name: 'agentPicker.search' }),
      { target: { value: 'no-such-target' } });
    expect(screen.queryAllByRole('menuitem')).toHaveLength(0);
    expect(screen.getByRole('status')).toHaveTextContent('agentPicker.noMatch');
    fireEvent.keyDown(document, { key: 'Escape' });
    const trigger = screen.getByRole('button', { name: 'Choose' });
    expect(trigger).toHaveFocus();
    await act(async () => { fireEvent.click(trigger); });
    expect(screen.getByRole('searchbox', { name: 'agentPicker.search' })).toHaveValue('');
    expect(screen.getAllByRole('menuitem')).toHaveLength(3);
  });

  it('navigates enabled choices from search and returns focus on Escape', async () => {
    const select = vi.fn();
    list.mockResolvedValue({ targets: [{
      runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode', stale: false, live_refresh_ok: true,
      models: [{ ...model('missing', 'economy'), availability: 'unavailable' }, model('ready', 'reasoning')],
    }] });
    render(<AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={['ClaudeCode', 'Codex']}
      currentTier="default" onSelectionChange={select} title="Choose" ariaLabel="Choose" />);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Choose' })); });
    const search = screen.getByRole('searchbox', { name: 'agentPicker.search' });
    fireEvent.keyDown(search, { key: 'ArrowDown' });
    const first = screen.getByRole('menuitem', { name: /Claude Code.*reasoning/ });
    expect(first).toHaveFocus();
    fireEvent.keyDown(first, { key: 'End' });
    const last = screen.getByRole('menuitem', { name: /Codex.*reasoning/ });
    expect(last).toHaveFocus();
    fireEvent.keyDown(last, { key: 'ArrowDown' });
    expect(first).toHaveFocus();
    fireEvent.keyDown(first, { key: 'ArrowUp' });
    expect(search).toHaveFocus();
    fireEvent.keyDown(search, { key: 'Escape' });
    expect(screen.getByRole('button', { name: 'Choose' })).toHaveFocus();
    expect(select).not.toHaveBeenCalled();
  });

  it('keeps model provenance and unavailability accessible after filtering', async () => {
    list.mockResolvedValue({ targets: [{
      runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode', stale: true, live_refresh_ok: false,
      models: [{ ...model('gone-model', 'reasoning'), availability: 'unavailable' }],
    }] });
    await show();
    fireEvent.change(screen.getByRole('searchbox', { name: 'agentPicker.search' }),
      { target: { value: 'gone-model' } });
    const unavailable = screen.getByRole('menuitem', { name: /Claude Code.*reasoning/ });
    expect(unavailable).toBeDisabled();
    expect(unavailable).toHaveAccessibleDescription(/gone-model.*modelCatalog.provenance.cached.*modelCatalog.unavailable/);
  });

  it('provides search in the simple agent-only picker too', async () => {
    render(<AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={['ClaudeCode', 'Codex']}
      onChange={vi.fn()} title="Choose" ariaLabel="Choose" />);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Choose' })); });
    fireEvent.change(screen.getByRole('searchbox', { name: 'agentPicker.search' }), { target: { value: 'codex' } });
    expect(screen.getAllByRole('menuitem')).toHaveLength(1);
    expect(screen.getByRole('menuitem', { name: 'Codex' })).toBeInTheDocument();
  });

  it('closes only the picker on Escape inside its search field', async () => {
    const parentKey = vi.fn();
    render(<div onKeyDown={parentKey}>
      <AgentSwitchPicker currentAgent="ClaudeCode" availableAgents={['ClaudeCode', 'Codex']}
        onChange={vi.fn()} title="Choose" ariaLabel="Choose" />
    </div>);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Choose' })); });
    fireEvent.keyDown(screen.getByRole('searchbox', { name: 'agentPicker.search' }), { key: 'Escape' });
    expect(screen.queryByRole('dialog', { name: 'Choose' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Choose' })).toHaveFocus();
    expect(parentKey).not.toHaveBeenCalled();
  });

  it('selects the exact filtered target once and returns focus after asynchronous persistence', async () => {
    const select = vi.fn().mockResolvedValue(undefined);
    render(<AgentSwitchPicker currentAgent="Codex" availableAgents={['Codex', 'Custom']} currentTier="default"
      availableTargets={[{ agent: 'Codex' }, { agent: 'Custom', connectionId: 'team-a', label: 'Équipe A' },
        { agent: 'Custom', connectionId: 'team-b', label: 'Équipe B' }]}
      onTargetSelectionChange={select} title="Choose" ariaLabel="Choose" />);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Choose' })); });
    fireEvent.change(screen.getByRole('searchbox', { name: 'agentPicker.search' }), { target: { value: 'team-b' } });
    const option = screen.getByRole('menuitem', { name: /Équipe B.*reasoning/ });
    await act(async () => { fireEvent.click(option); fireEvent.click(option); });
    expect(select).toHaveBeenCalledTimes(1);
    expect(select).toHaveBeenCalledWith({ agent: 'Custom', connectionId: 'team-b', label: 'Équipe B' }, 'reasoning');
    expect(screen.queryByRole('dialog', { name: 'Choose' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Choose' })).toHaveFocus();
  });
});

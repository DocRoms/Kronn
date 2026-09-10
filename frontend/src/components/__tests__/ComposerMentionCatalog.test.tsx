import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../test/apiMock';
import type { AgentDetection, CatalogModelEntry, Discussion, ModelTiersConfig, ParticipantView } from '../../types/generated';
import type { ExternalApiConnectionView } from '../../lib/api';

const { list } = vi.hoisted(() => ({ list: vi.fn() }));
vi.mock('../../lib/api', () => buildApiMock({
  modelCatalogApi: { list },
  discussions: { participants: vi.fn().mockResolvedValue([]), nativeAgentMode: vi.fn().mockResolvedValue({ disabled: false }) },
  config: { getServerConfig: vi.fn().mockResolvedValue({ default_model_tier: 'default' }) },
}));
vi.mock('../../lib/stt-engine', () => ({ audioBufferToFloat32: vi.fn(), transcribeAudio: vi.fn() }));
import { ChatInput } from '../ChatInput';
import { NewDiscussionForm } from '../NewDiscussionForm';
import { loadDiscussionRoutingPreferences } from '../../lib/discussionRoutingPreferences';
import { discussions as discussionsApi } from '../../lib/api';

const t = (key: string, ...args: (string | number)[]) => [key, ...args].join(' · ');
const agent: AgentDetection = { agent_type: 'LiteLlm', name: 'LiteLLM', installed: true, enabled: true,
  path: null, version: null, latest_version: null, origin: 'host', install_command: null,
  host_managed: false, host_label: null, runtime_available: true, rtk_available: false, rtk_hook_configured: false };
const connection: ExternalApiConnectionView = { id: 'team-a', display_name: 'Équipe A', mention_alias: 'team-a',
  endpoint: 'https://example.invalid/v1', credential_slug: 'fixture-only', origin_preset: 'other',
  economy_model: null, default_model: 'same-id', reasoning_model: null, has_credential: false,
  created_at: '2026-09-09T00:00:00Z', updated_at: '2026-09-09T00:00:00Z' };
function model(runtime: string, patch: Partial<CatalogModelEntry> = {}): CatalogModelEntry {
  return { id: runtime, runtime_target_id: runtime, agent_type: 'LiteLlm', model_id: 'same-id',
    display_name: 'Same ID', display_alias: runtime === 'http:team-a' ? 'Modèle équipe A' : 'Foreign model',
    tier_assignment: 'default', provenance: 'live', availability: 'available', capabilities: ['chat'],
    reasoning_modes: [], manual_origin: false, first_seen_at: '2026-09-09T00:00:00Z',
    last_checked_at: '2026-09-09T00:00:00Z', created_at: '2026-09-09T00:00:00Z', updated_at: '2026-09-09T00:00:00Z', ...patch };
}
function snapshot(patch: Partial<CatalogModelEntry> = {}) {
  return { targets: ['http:team-b', 'http:team-a'].map(runtime => ({ runtime_target_id: runtime,
    agent_type: 'LiteLlm', stale: runtime === 'http:team-a', live_refresh_ok: runtime !== 'http:team-a',
    models: [model(runtime, runtime === 'http:team-a' ? patch : {})] })) };
}
function familyConfig(): ModelTiersConfig {
  const empty = () => ({ economy: null, default: null, reasoning: null });
  return { claude_code: empty(), codex: empty(), open_code: empty(), vibe: empty(), gemini_cli: empty(),
    kiro: empty(), copilot_cli: empty(), ollama: empty(), nvidia: empty(),
    lite_llm: { economy: 'foreign-family', default: 'foreign-family', reasoning: 'foreign-family' } };
}
async function showChat(savedModel?: string, connectionPatch: Partial<ExternalApiConnectionView> = {}) {
  const onSend = vi.fn();
  const discussion = { id: 'catalog-mentions', agent: 'LiteLlm', connection_id: 'team-a', tier: 'default',
    model: savedModel, participants: ['LiteLlm'], messages: [], language: 'fr', skill_ids: [], directive_ids: [] } as unknown as Discussion;
  render(<ChatInput discussion={discussion} agents={[agent]} externalConnections={[{ ...connection, ...connectionPatch }]} modelTiers={familyConfig()}
    sending={false} disabled={false} ttsEnabled={false} ttsState="idle" worktreeError={null}
    availableSkills={[]} availableDirectives={[]} onSend={onSend} onStop={vi.fn()} onOrchestrate={vi.fn()}
    onTtsToggle={vi.fn()} onWorktreeErrorDismiss={vi.fn()} onWorktreeRetry={vi.fn()}
    isAgentRestricted={() => false} toast={vi.fn()} t={t} />);
  const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
  await act(async () => { fireEvent.change(textarea, { target: { value: '@litellm' } }); });
  return { textarea, onSend };
}
async function showNew(connectionPatch: Partial<ExternalApiConnectionView> = {}) {
  const onSubmit = vi.fn();
  render(<NewDiscussionForm projects={[]} agents={[agent]} configLanguage="fr" agentAccess={null}
    externalConnections={[{ ...connection, ...connectionPatch }]} onSubmit={onSubmit} onClose={vi.fn()} onNavigate={vi.fn()} t={t} />);
  const textarea = document.querySelector('textarea')!;
  await act(async () => { fireEvent.change(textarea, { target: { value: '@team-a' } }); });
  return { textarea, onSubmit };
}
function tierButton(tier: string): HTMLButtonElement {
  return document.querySelector<HTMLButtonElement>(`.disc-mention-tier-choice[data-tier="${tier}"]`)!;
}
beforeEach(() => { localStorage.clear(); list.mockReset().mockResolvedValue(snapshot()); });
afterEach(cleanup);

describe('composer mentions — shared exact catalogue contract', () => {
  it.each(['chat', 'new'] as const)('searches the connection display name independently of its model alias (%s)', async mode => {
    const patch = { display_name: 'Bureau privé' };
    const { textarea } = mode === 'chat' ? await showChat(undefined, patch) : await showNew(patch);
    fireEvent.change(textarea, { target: { value: '@bureau' } });
    expect(document.querySelector('.disc-mention-popover')).toBeInTheDocument();
    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(textarea).toHaveValue(mode === 'chat' ? '@litellm ' : '@team-a ');
  });

  it.each(['chat', 'new'] as const)('keeps a valid keyboard index while catalogue search results arrive (%s)', async mode => {
    let resolve!: (value: ReturnType<typeof snapshot>) => void;
    list.mockReturnValue(new Promise(value => { resolve = value; }));
    const { textarea } = mode === 'chat' ? await showChat() : await showNew();
    fireEvent.change(textarea, { target: { value: '@modèle' } });
    expect(document.querySelector('.disc-mention-popover')).toBeNull();
    fireEvent.keyDown(textarea, { key: 'ArrowDown' });
    await act(async () => { resolve(snapshot()); });
    expect(tierButton('default')).toBeInTheDocument();
    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(textarea).toHaveValue(mode === 'chat' ? '@litellm ' : '@team-a ');
  });

  it.each(['chat', 'new'] as const)('searches model aliases and exact IDs without refetch or preference changes (%s)', async mode => {
    const { textarea } = mode === 'chat' ? await showChat() : await showNew();
    for (const query of ['equipe', 'Équipe', 'same-id']) {
      fireEvent.change(textarea, { target: { value: `@${query}` } });
      expect(document.querySelector('.disc-mention-popover')).toBeInTheDocument();
      expect(tierButton('default')).toHaveAccessibleDescription(expect.stringContaining('modelCatalog.provenance.cached'));
    }
    expect(list).toHaveBeenCalledTimes(1);
    expect(loadDiscussionRoutingPreferences('catalog-mentions')).toEqual({});
    fireEvent.change(textarea, { target: { value: '@Foreign' } });
    expect(document.querySelector('.disc-mention-popover')).toBeNull();
    fireEvent.change(textarea, { target: { value: '@equipe' } });
    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(textarea).toHaveValue(mode === 'chat' ? '@litellm ' : '@team-a ');
    expect(loadDiscussionRoutingPreferences('catalog-mentions')).toEqual({});
  });

  it.each(['chat', 'new'] as const)('finds an unknown configured model ID and inserts only the canonical target (%s)', async mode => {
    const patch = { default_model: 'vendor/model.v2:fast' };
    const { textarea } = mode === 'chat' ? await showChat(undefined, patch) : await showNew(patch);
    fireEvent.change(textarea, { target: { value: '@vendor/model.v2:fast' } });
    expect(document.querySelector('.disc-mention-popover')).toBeInTheDocument();
    expect(tierButton('default')).toHaveTextContent('modelCatalog.notInCatalog');
    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(textarea).toHaveValue(mode === 'chat' ? '@litellm ' : '@team-a ');
    expect(list).toHaveBeenCalledTimes(1);
  });

  it.each(['chat', 'new'] as const)('keeps searched unavailable models visible without keyboard or pointer substitution (%s)', async mode => {
    list.mockResolvedValue(snapshot({ availability: 'unavailable' }));
    const { textarea } = mode === 'chat' ? await showChat() : await showNew();
    fireEvent.change(textarea, { target: { value: '@equipe' } });
    expect(tierButton('default')).toBeDisabled();
    expect(tierButton('default')).toHaveTextContent('modelCatalog.unavailable');
    fireEvent.keyDown(textarea, { key: 'Enter' });
    fireEvent.mouseDown(tierButton('default'));
    expect(textarea).toHaveValue('@equipe');
  });

  it('resolves the principal HTTP connection and cached provenance, not a colliding model or family default', async () => {
    await showChat();
    expect(tierButton('reasoning')).toHaveAttribute('title', expect.stringContaining('Modèle équipe A'));
    expect(tierButton('reasoning')).toHaveAccessibleDescription(expect.stringContaining('modelCatalog.provenance.cached'));
    expect(document.querySelector('.disc-mention-popover')).not.toHaveTextContent('Foreign model');
    expect(tierButton('reasoning').title).not.toContain('foreign-family');
  });

  it('resolves an HTTP mention’s empty tier from its own default and displays provenance in the new form', async () => {
    await showNew();
    expect(tierButton('reasoning')).toHaveAttribute('title', expect.stringContaining('Modèle équipe A'));
    expect(tierButton('reasoning')).toHaveAccessibleDescription(expect.stringContaining('modelCatalog.provenance.cached'));
    expect(within(tierButton('reasoning')).getByText('modelCatalog.provenance.cached')).toBeVisible();
  });

  it('keeps the principal saved override for untouched insertion but resolves explicit tier choices independently', async () => {
    const { textarea } = await showChat('saved-unknown');
    expect(tierButton('default').title).toContain('Modèle équipe A');
    expect(document.querySelector('.disc-mention-tier-choices')).toHaveAttribute('aria-label', expect.stringContaining('saved-unknown'));
    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(textarea).toHaveValue('@litellm ');
    expect(document.querySelector('.disc-composer-routing-chips')).toBeNull();
    expect(loadDiscussionRoutingPreferences('catalog-mentions')).toEqual({});
  });

  it('persists only an explicitly selected tier and preserves the HTTP target at launch', async () => {
    const { textarea, onSubmit } = await showNew();
    fireEvent.mouseDown(tierButton('reasoning'));
    expect(textarea).toHaveValue('@team-a ');
    await act(async () => { fireEvent.click(document.querySelector<HTMLButtonElement>('.disc-create-btn')!); });
    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({
      agent: 'Custom', tier: 'reasoning', initialTargets: [{
        kind: 'discussion_agent', agent_type: 'Custom', connection_id: 'team-a', tier: 'reasoning',
      }],
    }));
  });

  it('skips an unavailable intermediate tier without wrapping or changing an untouched preference', async () => {
    list.mockResolvedValue({ targets: [{ runtime_target_id: 'http:team-a', agent_type: 'LiteLlm', stale: false, live_refresh_ok: true,
      models: [model('http:team-a', { availability: 'unavailable' }),
        model('http:team-a', { model_id: 'economy-only', tier_assignment: 'economy' }),
        model('http:team-a', { model_id: 'reasoning-only', tier_assignment: 'reasoning' })] }] });
    await showChat(undefined, { economy_model: 'economy-only', reasoning_model: 'reasoning-only' });
    const textarea = screen.getByRole('textbox');
    expect(tierButton('default')).toBeDisabled();
    fireEvent.keyDown(textarea, { key: 'ArrowLeft' });
    expect(tierButton('economy')).toHaveAttribute('data-keyboard-selected', 'true');
    fireEvent.keyDown(textarea, { key: 'ArrowRight' });
    expect(tierButton('reasoning')).toHaveAttribute('data-keyboard-selected', 'true');
    fireEvent.keyDown(textarea, { key: 'ArrowRight' });
    expect(tierButton('reasoning')).toHaveAttribute('data-keyboard-selected', 'true');
    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(loadDiscussionRoutingPreferences('catalog-mentions')).toEqual({ LiteLlm: 'reasoning' });
  });

  it.each(['chat', 'new'] as const)('disables a known unavailable identity and refuses implicit keyboard/mouse selection (%s)', async mode => {
    list.mockResolvedValue(snapshot({ availability: 'unavailable' }));
    const { textarea } = mode === 'chat' ? await showChat() : await showNew();
    expect(tierButton('reasoning')).toBeDisabled();
    expect(tierButton('reasoning')).toHaveTextContent('modelCatalog.unavailable');
    const before = textarea.value;
    fireEvent.keyDown(textarea, { key: 'ArrowRight' });
    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(textarea).toHaveValue(before);
    expect(document.querySelector('.disc-mention-popover')).toBeInTheDocument();
    fireEvent.mouseDown(tierButton('reasoning'));
    expect(textarea).toHaveValue(before);
  });

  it('never lets a disabled tier click bubble into a different available default', async () => {
    const data = snapshot();
    data.targets[1].models.push(model('http:team-a', { model_id: 'missing', tier_assignment: 'reasoning', availability: 'unavailable' }));
    list.mockResolvedValue(data);
    const { textarea } = await showNew({ reasoning_model: 'missing' });
    expect(tierButton('reasoning')).toBeDisabled();
    expect(tierButton('default')).not.toBeDisabled();
    fireEvent.mouseDown(tierButton('reasoning'));
    expect(textarea).toHaveValue('@team-a');
    expect(document.querySelector('.disc-mention-popover')).toBeInTheDocument();
  });

  it('keeps a joined CLI ordinal and exact session without assigning a native tier or model', async () => {
    const participant: ParticipantView = { id: 42, disc_id: 'catalog-mentions', agent_type: 'Codex', session_id: 'fixture-cli',
      role: 'peer', status: 'active', joined_at: '2026-09-09T00:00:00Z', left_at: null, last_seen: null, activity: null,
      presence_state: 'listening', read_live: true, write_state: 'ok', wake_mode: 'external_poll', next_poll_at: null,
      last_write_at: null, resume_reason: null, resume_since: null, model: 'cli-declared-model', conversation_id: null, cli_ordinal: 4 };
    vi.mocked(discussionsApi.participants).mockResolvedValueOnce([participant]);
    const data = snapshot();
    data.targets.push({ runtime_target_id: 'agent:codex', agent_type: 'Codex', stale: false, live_refresh_ok: true,
      models: [model('agent:codex', { agent_type: 'Codex', model_id: 'native-only', display_alias: 'Native assignment' })] });
    list.mockResolvedValue(data);
    const { textarea, onSend } = await showChat();
    await act(async () => { fireEvent.change(textarea, { target: { value: '@codex-cli-4' } }); });
    expect(document.querySelector('.disc-mention-tier-choices')).toBeNull();
    expect(screen.getByLabelText('disc.routingCliModelManaged')).toBeInTheDocument();
    fireEvent.change(textarea, { target: { value: '@cli-4' } });
    expect(screen.getByLabelText('disc.routingCliModelManaged')).toBeInTheDocument();
    fireEvent.change(textarea, { target: { value: '@native-only' } });
    expect(document.querySelector('.disc-mention-popover')).toBeNull();
    fireEvent.change(textarea, { target: { value: '@cli-4' } });
    fireEvent.keyDown(textarea, { key: 'ArrowRight' });
    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(textarea).toHaveValue('@codex-cli-4 ');
    fireEvent.keyDown(textarea, { key: 'Enter' });
    expect(onSend).toHaveBeenCalledTimes(1);
    expect(onSend.mock.calls[0][1]).toEqual([{ kind: 'cli', agent_type: 'Codex', cli_session_id: 42 }]);
    expect(loadDiscussionRoutingPreferences('catalog-mentions')).toEqual({});
  });

  it('fetches once while typing and retains stale identities with an explicit reload error', async () => {
    const { textarea } = await showChat();
    await waitFor(() => expect(list).toHaveBeenCalledTimes(1));
    fireEvent.change(textarea, { target: { value: '@litell' } });
    fireEvent.change(textarea, { target: { value: '@litellm' } });
    expect(list).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(textarea, { key: 'Escape' });
    list.mockRejectedValue(new Error('fixture offline'));
    await act(async () => { fireEvent.change(textarea, { target: { value: '@lite' } }); });
    expect(await screen.findByText('modelCatalog.loadError')).toBeVisible();
    expect(tierButton('reasoning')).toHaveAccessibleDescription(expect.stringContaining('modelCatalog.provenance.cached'));
  });
});

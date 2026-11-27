// KT-2003 — the reason the server writes under an agent that cannot be chosen
// is written from the locale files: GitHub Copilot without full access reads,
// in French, a French sentence whose menu path uses the interface's own tab
// and section names. A third-party text stays quoted as received; an unknown
// code falls back to the English sentence.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../test/apiMock';
import type { CatalogModelEntry, CatalogReasonCode, CatalogTierVerdict, ModelCatalogView } from '../../types/generated';

const { list } = vi.hoisted(() => ({ list: vi.fn() }));
vi.mock('../../lib/api', () => buildApiMock({ modelCatalogApi: { list: list as never } }));

import { AgentSwitchPicker, type AgentSwitchTarget } from '../AgentSwitchPicker';
import { I18nProvider } from '../../lib/I18nContext';
import { loadLocale, setUILocale } from '../../lib/i18n';
import fr from '../../lib/i18n/locales/fr';

const ENGLISH = 'GitHub Copilot runs only with full access. Enable it in Config › Agents › GitHub Copilot › Full access, or choose another agent.';
const FULL_ACCESS: CatalogReasonCode = { code: 'native_full_access_required', agent: 'GitHub Copilot' };

function model(runtime: string, id: string, agent: CatalogModelEntry['agent_type']): CatalogModelEntry {
  return {
    id: `${runtime}/${id}`, runtime_target_id: runtime, agent_type: agent, model_id: id, display_name: id,
    tier_assignment: null, provenance: 'live', availability: 'available', capabilities: ['chat'], reasoning_modes: [],
    manual_origin: false, first_seen_at: '2026-11-23T00:00:00Z', last_checked_at: '2026-11-23T00:00:00Z',
    created_at: '2026-11-23T00:00:00Z', updated_at: '2026-11-23T00:00:00Z',
  } as CatalogModelEntry;
}

const refused = (verdict: Partial<CatalogTierVerdict>): CatalogTierVerdict[] =>
  (['economy', 'default', 'reasoning'] as const).map(tier => ({
    tier, requested_model: null, effective_model: null, launchable: false, reason: 'provider_error', ...verdict,
  }));

function catalog(copilot: CatalogTierVerdict[]) {
  return { targets: [
    { runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode', stale: false, live_refresh_ok: true,
      models: [model('agent:claude-code', 'claude-sonnet-4-5', 'ClaudeCode')] },
    { runtime_target_id: 'agent:copilot-cli', agent_type: 'CopilotCli', stale: true, live_refresh_ok: false,
      models: [], tier_verdicts: copilot },
  ] as ModelCatalogView[] };
}

const TARGETS: AgentSwitchTarget[] = [{ agent: 'ClaudeCode' }, { agent: 'CopilotCli' }];

async function openInFrench() {
  await loadLocale('fr');
  setUILocale('fr');
  render(
    <I18nProvider>
      <AgentSwitchPicker currentAgent="ClaudeCode" currentTier="default" availableAgents={[]} availableTargets={TARGETS}
        onTargetSelectionChange={vi.fn()} title="Agent" ariaLabel="Agent" />
    </I18nProvider>,
  );
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: /^Agent( — |$)/ })); });
  await waitFor(() => expect(list).toHaveBeenCalled());
  await act(async () => {});
}

const reasonOf = (name: string) => within(screen.getByRole('group', { name })).getByTestId('agent-switch-reason');

beforeEach(() => { list.mockReset(); });
afterEach(() => { cleanup(); setUILocale('fr'); });

describe('the reason under an unavailable agent, in French (KT-2003)', () => {
  it('writes the full-access refusal from the French locale, with the interface’s own menu path', async () => {
    list.mockResolvedValue(catalog(refused({ detail: ENGLISH, detail_code: FULL_ACCESS })));
    await openInFrench();
    const path = [fr['nav.config'], fr['config.agents'], 'GitHub Copilot', fr['config.fullAccessName']].join(' › ');
    const reason = reasonOf('GitHub Copilot');
    expect(reason).toHaveTextContent(
      `GitHub Copilot ne fonctionne qu'avec l'accès complet. Activez-le dans ${path}, ou choisissez un autre agent.`,
    );
    expect(reason.textContent).not.toContain('runs only');
    expect(reason.textContent).not.toContain('Full access');
    // The badge's tooltip says the same, in French too.
    expect(within(screen.getByRole('group', { name: 'GitHub Copilot' })).getByTestId('agent-unavailable').getAttribute('title') ?? '')
      .not.toContain('runs only');
  });

  it('quotes a third-party text as received under a French sentence', async () => {
    list.mockResolvedValue(catalog(refused({ detail: 'copilot: not logged in (exit 1)', reason: 'auth_required' })));
    await openInFrench();
    expect(reasonOf('GitHub Copilot')).toHaveTextContent(
      `${fr['serverReason.lead.auth_required']} : « copilot: not logged in (exit 1) »`,
    );
  });

  it('writes a discovery sentence Kronn itself wrote in French, with no English quote', async () => {
    // The Claude CLI missing: the server keeps « Claude CLI is not installed »
    // and sends its code; nothing of the English sentence reaches the line.
    list.mockResolvedValue(catalog(refused({
      reason: 'cli_missing',
      detail: 'Claude CLI is not installed',
      detail_code: { code: 'agent_cli_missing', agent: 'GitHub Copilot' },
    })));
    await openInFrench();
    const reason = reasonOf('GitHub Copilot');
    expect(reason).toHaveTextContent('La CLI de GitHub Copilot n\'est pas installée ou ne peut pas être lancée.');
    expect(reason.textContent).not.toContain('«');
    expect(reason.textContent).not.toContain('not installed');
  });

  it('falls back to the English sentence for a code this interface does not know', async () => {
    list.mockResolvedValue(catalog(refused({
      detail: ENGLISH, detail_code: { code: 'a_newer_reason', agent: 'GitHub Copilot' } as unknown as CatalogReasonCode,
    })));
    await openInFrench();
    expect(reasonOf('GitHub Copilot')).toHaveTextContent(ENGLISH);
  });
});

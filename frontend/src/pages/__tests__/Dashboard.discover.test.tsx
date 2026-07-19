/**
 * 0.8.7 — Dashboard "Add project → Discover repos" regression suite.
 *
 * Pins :
 *   - Bug 1 : closing the modal RESETS selectedSourceIds / discoveredRepos /
 *     availableSources / discoverSources / discoverSourceErrors / repoSearch
 *     / discoverError, so a re-open starts from a clean slate. Without this
 *     a previously-toggled `selectedSourceIds = [github-perso]` would leak
 *     into the next session and the user would see "only my personal
 *     GitHub" repos.
 *   - Bug 1 (companion) : toggling the LAST active chip OFF clears the
 *     repo list (otherwise stale repos hang on while no source is selected,
 *     perceived as "the filter is broken").
 *   - Bug 2 : per-source errors from the response render as amber chips
 *     so the user knows WHY a configured source returned zero repos
 *     (the GitLab silent-fail trigger).
 */
import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, act, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// `vi.mock` is hoisted above top-level `const`s, so we use `vi.hoisted`
// to share the spy with the test body without tripping the hoist guard.
const { discoverRepos } = vi.hoisted(() => ({ discoverRepos: vi.fn() }));

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ connected: false })),
}));

vi.mock('../../lib/api', () => ({
  projects: {
    list: vi.fn().mockResolvedValue([]),
    scan: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    delete: vi.fn(),
    installTemplate: vi.fn(),
    auditStream: vi.fn(),
    bootstrap: vi.fn(),
    discoverRepos,
    clone: vi.fn(),
    checkDrift: vi.fn().mockResolvedValue(null),
    addFromFolder: vi.fn(),
  },
  mcps: {
    registry: vi.fn().mockResolvedValue([]),
    overview: vi.fn().mockResolvedValue({
      servers: [],
      configs: [
        { id: 'cfg-gh-perso', server_id: 'mcp-github', label: 'github' },
        { id: 'cfg-gh-eu', server_id: 'mcp-github', label: 'github Euronews' },
        { id: 'cfg-gl', server_id: 'mcp-gitlab', label: 'GitLab' },
      ],
      customized_contexts: [],
      incompatibilities: [],
    }),
  },
  agents: {
    detect: vi.fn().mockResolvedValue([{ agent_type: 'ClaudeCode', name: 'Claude Code', installed: true, enabled: true }]),
    install: vi.fn(), uninstall: vi.fn(), toggle: vi.fn(),
  },
  discussions: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(), create: vi.fn(), delete: vi.fn(), update: vi.fn(),
    sendMessageStream: vi.fn(), runAgent: vi.fn(), _streamSSE: vi.fn(),
  },
  config: {
    getLanguage: vi.fn().mockResolvedValue('fr'),
    getUiLanguage: vi.fn().mockResolvedValue('fr'),
    saveUiLanguage: vi.fn().mockResolvedValue(undefined),
    getSttModel: vi.fn().mockResolvedValue(null),
    saveSttModel: vi.fn().mockResolvedValue(undefined),
    getTtsVoices: vi.fn().mockResolvedValue({}),
    saveTtsVoice: vi.fn().mockResolvedValue(undefined),
    getAgentAccess: vi.fn().mockResolvedValue(null),
  },
  skills: { list: vi.fn().mockResolvedValue([]) },
  profiles: { list: vi.fn().mockResolvedValue([]) },
  workflows: { list: vi.fn().mockResolvedValue([]) },
}));

import { Dashboard } from '../Dashboard';

const THREE_SOURCES = [
  { id: 'cfg-gl', label: 'GitLab', provider: 'gitlab' },
  { id: 'cfg-gh-perso', label: 'github', provider: 'github' },
  { id: 'cfg-gh-eu', label: 'github Euronews', provider: 'github' },
];
const FOUR_REPOS = [
  { full_name: 'DocRoms/personal-1', name: 'personal-1', description: null, clone_url: 'https://github.com/DocRoms/personal-1.git', ssh_url: 'git@github.com:DocRoms/personal-1.git', language: null, stargazers_count: 0, updated_at: '2026-01-01', already_cloned: false },
  { full_name: 'DocRoms/personal-2', name: 'personal-2', description: null, clone_url: 'https://github.com/DocRoms/personal-2.git', ssh_url: 'git@github.com:DocRoms/personal-2.git', language: null, stargazers_count: 0, updated_at: '2026-01-02', already_cloned: false },
  { full_name: 'Euronews-tech/front_euronews', name: 'front_euronews', description: null, clone_url: 'https://github.com/Euronews-tech/front_euronews.git', ssh_url: 'git@github.com:Euronews-tech/front_euronews.git', language: null, stargazers_count: 0, updated_at: '2026-01-03', already_cloned: false },
  { full_name: 'Euronews-tech/front_africanews', name: 'front_africanews', description: null, clone_url: 'https://github.com/Euronews-tech/front_africanews.git', ssh_url: 'git@github.com:Euronews-tech/front_africanews.git', language: null, stargazers_count: 0, updated_at: '2026-01-04', already_cloned: false },
];

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  discoverRepos.mockReset();
});
afterEach(() => {
  vi.useRealTimers();
  cleanup();
  localStorage.clear();
});

const wrap = async (ui: React.ReactElement) => {
  let result: ReturnType<typeof render>;
  await act(async () => { result = render(<I18nProvider>{ui}</I18nProvider>); });
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
  return result!;
};

const openBootstrap = async () => {
  const btn = document.querySelector('[data-tour-id="new-project-btn"]') as HTMLButtonElement;
  expect(btn).not.toBeNull();
  await act(async () => { fireEvent.click(btn); });
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
};

const closeModal = async () => {
  // The overlay closes the modal when clicked (when no work is in-flight).
  const overlay = document.querySelector('.dash-modal-overlay') as HTMLElement;
  expect(overlay).not.toBeNull();
  await act(async () => { fireEvent.click(overlay); });
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
};

const clickDiscoverBtn = async () => {
  // The discover-trigger button in the "clone" mode of the modal.
  const dashScanBtn = [...document.querySelectorAll('button')].find(
    b => /discover|découvrir|d.tect/i.test(b.textContent || '')
  );
  if (!dashScanBtn) throw new Error('Discover button not found');
  await act(async () => { fireEvent.click(dashScanBtn); });
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
};

const switchToCloneTab = async () => {
  // The modal opens on 'bootstrap' mode by default ; switch to 'clone'
  // (where Discover lives) by clicking the corresponding tab button.
  const cloneTab = [...document.querySelectorAll('button')].find(
    b => /git|clone|d.couvrir|discover/i.test(b.textContent || '') && b.className.includes('dash-tab')
  );
  if (cloneTab) {
    await act(async () => { fireEvent.click(cloneTab); });
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });
  }
};

const baseProps = {
  navigate: vi.fn(),
  onNavigateDiscussion: vi.fn(),
  onResetWorkspace: vi.fn(),
  onProjectChange: vi.fn(),
  onReset: vi.fn(),
};

describe('Dashboard — Discover repos modal-close state reset (Bug 1)', () => {
  it('clears selectedSourceIds + discovered state when the modal closes', async () => {
    // First open : the discover call returns all 3 sources + 4 repos.
    discoverRepos.mockResolvedValue({
      repos: FOUR_REPOS, sources: ['github', 'github Euronews'],
      available_sources: THREE_SOURCES, errors: [],
    });

    await wrap(<Dashboard {...baseProps} />);
    await openBootstrap();
    await switchToCloneTab();
    await clickDiscoverBtn();
    await waitFor(() => expect(discoverRepos).toHaveBeenCalled());

    const firstCallArgs = discoverRepos.mock.calls[0][0];
    // First call has the empty source_ids that triggers backend "use all".
    expect(firstCallArgs.source_ids).toEqual([]);

    // Close the modal — the reset useEffect must wipe selectedSourceIds.
    await closeModal();
    discoverRepos.mockClear();

    // Reopen + discover again. If the reset DIDN'T fire, this 2nd call
    // would carry the auto-populated selectedSourceIds from the 1st run.
    // After the fix : 2nd call MUST also send `source_ids: []`.
    await openBootstrap();
    await switchToCloneTab();
    await clickDiscoverBtn();
    await waitFor(() => expect(discoverRepos).toHaveBeenCalled());

    const secondCallArgs = discoverRepos.mock.calls[0][0];
    expect(secondCallArgs.source_ids).toEqual([]);
  });
});

describe('Dashboard — Discover repos per-source errors (Bug 2)', () => {
  it('renders a chip for every source that reported an error', async () => {
    discoverRepos.mockResolvedValue({
      repos: FOUR_REPOS,
      sources: ['github', 'github Euronews'],
      available_sources: THREE_SOURCES,
      errors: [
        { source_id: 'cfg-gl', source_label: 'GitLab', provider: 'gitlab',
          message: '401 Unauthorized — token revoked or scopes missing' },
      ],
    });

    await wrap(<Dashboard {...baseProps} />);
    await openBootstrap();
    await switchToCloneTab();
    await clickDiscoverBtn();
    await waitFor(() => expect(discoverRepos).toHaveBeenCalled());

    const errBlock = document.querySelector('[data-testid="discover-source-errors"]');
    expect(errBlock).not.toBeNull();
    const text = errBlock!.textContent || '';
    expect(text).toContain('GitLab');
    expect(text).toContain('401 Unauthorized');
    // Provider classname for theming
    expect(errBlock!.querySelector('[data-provider="gitlab"]')).not.toBeNull();
  });

  it('renders NOTHING when the response has no errors (status quo)', async () => {
    discoverRepos.mockResolvedValue({
      repos: FOUR_REPOS, sources: ['github'],
      available_sources: THREE_SOURCES, errors: [],
    });
    await wrap(<Dashboard {...baseProps} />);
    await openBootstrap();
    await switchToCloneTab();
    await clickDiscoverBtn();
    await waitFor(() => expect(discoverRepos).toHaveBeenCalled());
    expect(document.querySelector('[data-testid="discover-source-errors"]')).toBeNull();
  });

  it('tolerates a legacy backend that omits the `errors` field entirely', async () => {
    // The frontend defaults `res.errors ?? []` so an older backend (pre-0.8.7)
    // doesn't crash the modal. Regression guard for the partial-rollout case.
    discoverRepos.mockResolvedValue({
      repos: FOUR_REPOS, sources: ['github'],
      available_sources: THREE_SOURCES,
      // no `errors` key at all
    });
    await wrap(<Dashboard {...baseProps} />);
    await openBootstrap();
    await switchToCloneTab();
    await clickDiscoverBtn();
    await waitFor(() => expect(discoverRepos).toHaveBeenCalled());
    expect(document.querySelector('[data-testid="discover-source-errors"]')).toBeNull();
  });
});

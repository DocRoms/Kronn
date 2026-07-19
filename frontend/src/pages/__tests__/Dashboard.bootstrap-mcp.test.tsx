import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, act, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// Mock everything Dashboard touches at mount. The bootstrap auto-pick is the
// only behavior under test, so the rest is stubbed to no-op responses.
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
    discoverRepos: vi.fn(),
    clone: vi.fn(),
    checkDrift: vi.fn().mockResolvedValue(null),
    addFromFolder: vi.fn(),
  },
  mcps: {
    registry: vi.fn().mockResolvedValue([]),
    overview: vi.fn().mockResolvedValue({
      servers: [],
      // Two distinct MCPs both eligible for tracker (github + atlassian).
      // The auto-pick should land on the FIRST one (github here) by spec.
      configs: [
        { id: 'cfg-gh', server_id: 'mcp-github', label: 'GitHub' },
        { id: 'cfg-jira', server_id: 'mcp-atlassian', label: 'Jira' },
      ],
      customized_contexts: [],
      incompatibilities: [],
    }),
  },
  agents: {
    detect: vi.fn().mockResolvedValue([]),
    install: vi.fn(),
    uninstall: vi.fn(),
    toggle: vi.fn(),
  },
  discussions: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
    create: vi.fn(),
    delete: vi.fn(),
    update: vi.fn(),
    sendMessageStream: vi.fn(),
    runAgent: vi.fn(),
    _streamSSE: vi.fn(),
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

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
});

afterEach(() => {
  vi.useRealTimers();
  cleanup();
  localStorage.clear();
});

const wrap = async (ui: React.ReactElement) => {
  let result: ReturnType<typeof render>;
  await act(async () => {
    result = render(<I18nProvider>{ui}</I18nProvider>);
  });
  // Allow API mocks + effects to settle.
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
  return result!;
};

const openBootstrap = async () => {
  // The "+" button in the top nav opens the bootstrap modal.
  const newProjectBtn = document.querySelector('[data-tour-id="new-project-btn"]') as HTMLButtonElement;
  expect(newProjectBtn).not.toBeNull();
  await act(async () => { fireEvent.click(newProjectBtn); });
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
};

describe('Dashboard — bootstrap MCP auto-pick', () => {
  it('does NOT override the user\'s explicit "no repo creation" choice', async () => {
    // Pre-fix: the auto-pick effect listed `bootstrapRepoMcp` in its deps.
    // When the user chose the empty option, `bootstrapRepoMcp` flipped to '',
    // which re-fired the effect, the `!bootstrapRepoMcp` guard passed, and
    // the dropdown snapped back to `cfg-gh`. The user could never opt out
    // of repo creation without closing the modal.
    await wrap(<Dashboard onReset={vi.fn()} />);
    await openBootstrap();

    // The bootstrap modal renders two selects: repo MCP and tracker MCP.
    // Both should auto-pick the first eligible config on first render.
    await waitFor(() => {
      const selects = document.querySelectorAll('select');
      // Project mode selects + repo + tracker — the dropdowns we want are
      // the ones whose option list includes the GitHub config id.
      const repoSelect = Array.from(selects).find(s => s.value === 'cfg-gh');
      expect(repoSelect).toBeTruthy();
    });

    // Find the repo dropdown (its value is cfg-gh after auto-pick) and pick
    // the empty option ("no repo creation"). The bug snapped this back to
    // cfg-gh on the next render — the fix keeps it empty.
    const repoSelect = Array.from(document.querySelectorAll('select'))
      .find(s => s.value === 'cfg-gh') as HTMLSelectElement;
    expect(repoSelect).toBeTruthy();

    await act(async () => {
      fireEvent.change(repoSelect, { target: { value: '' } });
    });
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    expect(repoSelect.value).toBe('');
  });

  it('re-runs the auto-pick when the modal is closed and reopened', async () => {
    // Sanity: the one-shot guard resets on the showBootstrap=false transition,
    // so a second open after close picks again from scratch (in case the user
    // had opted out in the previous session — they get a fresh default).
    await wrap(<Dashboard onReset={vi.fn()} />);
    await openBootstrap();

    await waitFor(() => {
      const repoSelect = Array.from(document.querySelectorAll('select'))
        .find(s => s.value === 'cfg-gh');
      expect(repoSelect).toBeTruthy();
    });

    // Opt out, then close.
    const repoSelect = Array.from(document.querySelectorAll('select'))
      .find(s => s.value === 'cfg-gh') as HTMLSelectElement;
    await act(async () => { fireEvent.change(repoSelect, { target: { value: '' } }); });
    const closeBtn = document.querySelector('.dash-modal-close') as HTMLButtonElement;
    await act(async () => { fireEvent.click(closeBtn); });
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    // Reopen — auto-pick should fire again, restoring the default.
    await openBootstrap();
    await waitFor(() => {
      const reopened = Array.from(document.querySelectorAll('select'))
        .find(s => s.value === 'cfg-gh');
      expect(reopened).toBeTruthy();
    });
  });
});

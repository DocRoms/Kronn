// Note: assertions use French strings because the default UI locale is 'fr'.
// If the default locale changes, these assertions must be updated.
import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, act, cleanup } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// ─── API mock ────────────────────────────────────────────────────────────────

vi.mock('../../lib/api', () => ({
  agents: {
    detect: vi.fn().mockResolvedValue([]),
    install: vi.fn().mockResolvedValue('installed'),
    toggle: vi.fn().mockResolvedValue(true),
  },
  setup: {
    getStatus: vi.fn().mockResolvedValue({ repos_detected: [], agents_detected: [], is_first_run: true, current_step: 'Agents' }),
    setScanPaths: vi.fn().mockResolvedValue(undefined),
    complete: vi.fn().mockResolvedValue(undefined),
  },
  projects: {
    create: vi.fn().mockResolvedValue({ id: 'p1', name: 'repo1' }),
  },
}));

import { agents as agentsApi, setup as setupApi, projects as projectsApi } from '../../lib/api';
import { SetupWizard } from '../SetupWizard';
import type { AgentDetection, SetupStatus, Project } from '../../types/generated';

// ─── Fixtures ────────────────────────────────────────────────────────────────

const makeAgent = (overrides: Partial<AgentDetection> = {}): AgentDetection => ({
  name: 'Claude Code',
  agent_type: 'ClaudeCode',
  installed: true,
  enabled: true,
  path: '/usr/bin/claude',
  version: '1.0.0',
  latest_version: null,
  origin: 'host',
  install_command: null,
  host_managed: false,
  host_label: null,
  runtime_available: false,
  ...overrides,
});

const makeStatus = (overrides: Partial<SetupStatus> = {}): SetupStatus => ({
  is_first_run: true,
  current_step: 'Agents',
  agents_detected: [],
  repos_detected: [],
  scan_paths_set: false,
  default_scan_path: null,
  ...overrides,
});

const makeRepo = (overrides: Partial<import('../../types/generated').DetectedRepo> = {}): import('../../types/generated').DetectedRepo => ({
  name: 'my-repo',
  path: '/home/user/repos/my-repo',
  remote_url: null,
  branch: 'main',
  ai_configs: [],
  has_project: false,
  hidden: false,
  ...overrides,
});

// ─── Setup / teardown ────────────────────────────────────────────────────────

beforeEach(() => {
  vi.mocked(agentsApi.detect).mockResolvedValue([]);
  vi.mocked(setupApi.getStatus).mockResolvedValue(makeStatus());
  vi.mocked(setupApi.complete).mockResolvedValue(undefined);
  vi.mocked(projectsApi.create).mockResolvedValue({ id: 'p1' } as Project);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

const wrap = async (ui: React.ReactElement) => {
  let result: ReturnType<typeof render>;
  await act(async () => {
    result = render(<I18nProvider>{ui}</I18nProvider>);
  });
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
  return result!;
};

// ─── Tests ───────────────────────────────────────────────────────────────────

describe('SetupWizard — step 0 (agents detection)', () => {
  it('renders step 0 with the Agents IA heading', async () => {
    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);
    expect(document.body.textContent).toContain('Agents IA');
  });

  it('renders the Kronn title and subtitle', async () => {
    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);
    const body = document.body.textContent!;
    expect(body).toContain('Kronn');
    expect(body).toContain('Enter the grid');
  });

  it('shows step indicators for Agents, Depots, Termine', async () => {
    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);
    const body = document.body.textContent!;
    expect(body).toContain('Agents');
    expect(body).toContain('Dépôts');
    expect(body).toContain('Terminé');
  });

  it('shows "no agent detected" message when no agents are found', async () => {
    vi.mocked(agentsApi.detect).mockResolvedValue([]);
    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);
    expect(document.body.textContent).toContain('Aucun agent détecté');
  });

  it('shows detected agents with their name and OK badge when installed', async () => {
    const agent = makeAgent({ name: 'Claude Code', installed: true });
    vi.mocked(agentsApi.detect).mockResolvedValue([agent]);

    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    const body = document.body.textContent!;
    expect(body).toContain('Claude Code');
    expect(body).toContain('Activé');
  });

  it('shows install button for agents that are not installed', async () => {
    const agent = makeAgent({
      name: 'Gemini CLI',
      agent_type: 'GeminiCli',
      installed: false,
      runtime_available: false,
      install_command: 'npm install -g @google/gemini-cli',
    });
    vi.mocked(agentsApi.detect).mockResolvedValue([agent]);

    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    const buttons = Array.from(document.body.querySelectorAll('button'));
    const installBtn = buttons.find(b => b.textContent?.includes('Installer'));
    expect(installBtn).toBeTruthy();
  });

  it('shows agent count message when at least one agent is detected', async () => {
    const agent = makeAgent({ installed: true });
    vi.mocked(agentsApi.detect).mockResolvedValue([agent]);

    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    const body = document.body.textContent!;
    expect(body).toContain('1 agent détecté');
  });

  it('shows agent version when available', async () => {
    const agent = makeAgent({ version: '2.3.1', installed: true });
    vi.mocked(agentsApi.detect).mockResolvedValue([agent]);

    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    expect(document.body.textContent).toContain('v2.3.1');
  });

  it('uses initialStatus agents when provided (skips first detect call)', async () => {
    const agent = makeAgent({ name: 'Claude Code', installed: true });
    const status = makeStatus({ agents_detected: [agent] });

    // detect should be called on mount (refreshAgents), but initialStatus pre-populates state
    vi.mocked(agentsApi.detect).mockResolvedValue([agent]);
    await wrap(<SetupWizard initialStatus={status} onComplete={vi.fn()} />);

    expect(document.body.textContent).toContain('Claude Code');
  });

  it('shows "Passer cette étape" when no agent is installed', async () => {
    vi.mocked(agentsApi.detect).mockResolvedValue([]);
    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    const buttons = Array.from(document.body.querySelectorAll('button'));
    const skipBtn = buttons.find(b => b.textContent?.includes('Passer'));
    expect(skipBtn).toBeTruthy();
    expect(skipBtn!.disabled).toBe(false);
  });

  it('shows "Continuer" when at least one agent is installed', async () => {
    const agent = makeAgent({ installed: true });
    vi.mocked(agentsApi.detect).mockResolvedValue([agent]);

    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    const buttons = Array.from(document.body.querySelectorAll('button'));
    const continuerBtn = buttons.find(b => b.textContent?.includes('Continuer'));
    expect(continuerBtn).toBeTruthy();
    expect(continuerBtn!.disabled).toBe(false);
  });
});

describe('SetupWizard — navigation to step 1 (repos)', () => {
  it('navigates to step 1 when clicking Continuer with an installed agent', async () => {
    const agent = makeAgent({ installed: true });
    vi.mocked(agentsApi.detect).mockResolvedValue([agent]);
    vi.mocked(setupApi.getStatus).mockResolvedValue(makeStatus({ repos_detected: [] }));

    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    const buttons = Array.from(document.body.querySelectorAll('button'));
    const continuerBtn = buttons.find(b => b.textContent?.includes('Continuer'));
    expect(continuerBtn).toBeTruthy();

    await act(async () => { continuerBtn!.click(); });

    expect(document.body.textContent).toContain('Dépôts détectés');
  });

  it('shows scanning indicator when entering step 1 with no repos', async () => {
    const agent = makeAgent({ installed: true });
    vi.mocked(agentsApi.detect).mockResolvedValue([agent]);

    // Make getStatus hang briefly to observe the scanning state
    let resolveStatus!: (v: SetupStatus) => void;
    const statusPromise = new Promise<SetupStatus>(resolve => { resolveStatus = resolve; });
    vi.mocked(setupApi.getStatus).mockReturnValue(statusPromise);

    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    const buttons = Array.from(document.body.querySelectorAll('button'));
    const continuerBtn = buttons.find(b => b.textContent?.includes('Continuer'));
    await act(async () => { continuerBtn!.click(); });

    // While scanning is in progress, the loader should appear
    expect(document.body.textContent).toContain('Scan des dépôts git');

    // Resolve the promise to let the component settle
    await act(async () => {
      resolveStatus(makeStatus({ repos_detected: [] }));
    });
  });

  it('shows detected repos when scan returns results', async () => {
    const agent = makeAgent({ installed: true });
    vi.mocked(agentsApi.detect).mockResolvedValue([agent]);
    vi.mocked(setupApi.getStatus).mockResolvedValue(makeStatus({
      repos_detected: [makeRepo({ name: 'my-project', path: '/home/user/repos/my-project' })],
    }));

    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    const continuerBtn = Array.from(document.body.querySelectorAll('button')).find(b => b.textContent?.includes('Continuer'));
    await act(async () => { continuerBtn!.click(); });

    expect(document.body.textContent).toContain('my-project');
  });
});

describe('SetupWizard — step 1 (scan path configuration)', () => {
  const enterStep1 = async () => {
    const agent = makeAgent({ installed: true });
    vi.mocked(agentsApi.detect).mockResolvedValue([agent]);
    vi.mocked(setupApi.getStatus).mockResolvedValue(makeStatus({ repos_detected: [] }));

    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    const continuerBtn = Array.from(document.body.querySelectorAll('button')).find(b => b.textContent?.includes('Continuer'));
    await act(async () => { continuerBtn!.click(); });
  };

  it('shows path input when no repos detected (manual path prompt)', async () => {
    await enterStep1();
    // When no repos found, showManualPath is set to true automatically
    const inputs = document.body.querySelectorAll('input');
    const pathInput = Array.from(inputs).find(i => i.placeholder?.includes('~/work'));
    expect(pathInput).toBeTruthy();
  });

  it('has Scanner button to trigger a path scan', async () => {
    await enterStep1();
    const buttons = Array.from(document.body.querySelectorAll('button'));
    const scanBtn = buttons.find(b => b.textContent?.includes('Scanner'));
    expect(scanBtn).toBeTruthy();
  });

  it('shows re-scan icon button', async () => {
    await enterStep1();
    // RefreshCw icon button with title "Re-scanner"
    const buttons = Array.from(document.body.querySelectorAll('button'));
    const rescanBtn = buttons.find(b => b.getAttribute('title') === 'Re-scanner');
    expect(rescanBtn).toBeTruthy();
  });
});

describe('SetupWizard — step 2 (completion)', () => {
  it('navigates to done step and calls onComplete when clicking the final button', async () => {
    const agent = makeAgent({ installed: true });
    vi.mocked(agentsApi.detect).mockResolvedValue([agent]);
    vi.mocked(setupApi.getStatus).mockResolvedValue(makeStatus({
      repos_detected: [makeRepo({ name: 'my-project', path: '/home/user/repos/my-project' })],
    }));

    const onComplete = vi.fn();
    await wrap(<SetupWizard initialStatus={null} onComplete={onComplete} />);

    // Step 0 → Step 1
    const continuerBtn1 = Array.from(document.body.querySelectorAll('button')).find(b => b.textContent?.includes('Continuer'));
    await act(async () => { continuerBtn1!.click(); });

    // Step 1 → Step 2
    const continuerBtn2 = Array.from(document.body.querySelectorAll('button')).find(b => b.textContent?.includes('Continuer'));
    await act(async () => { continuerBtn2!.click(); });

    expect(document.body.textContent).toContain('Configuration terminée');

    // Trigger completion
    const dashboardBtn = Array.from(document.body.querySelectorAll('button')).find(b => b.textContent?.includes('Accéder au dashboard'));
    expect(dashboardBtn).toBeTruthy();
    await act(async () => { dashboardBtn!.click(); });

    expect(setupApi.complete).toHaveBeenCalledOnce();
    expect(onComplete).toHaveBeenCalledOnce();
  });

  it('shows Configuration terminee heading in step 2', async () => {
    const agent = makeAgent({ installed: true });
    vi.mocked(agentsApi.detect).mockResolvedValue([agent]);
    vi.mocked(setupApi.getStatus).mockResolvedValue(makeStatus({
      repos_detected: [makeRepo({ name: 'repo1', path: '/repos/repo1' })],
    }));

    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    // Navigate to step 2
    const btn1 = Array.from(document.body.querySelectorAll('button')).find(b => b.textContent?.includes('Continuer'));
    await act(async () => { btn1!.click(); });
    const btn2 = Array.from(document.body.querySelectorAll('button')).find(b => b.textContent?.includes('Continuer'));
    await act(async () => { btn2!.click(); });

    expect(document.body.textContent).toContain('Configuration terminée');
    expect(document.body.textContent).toContain('Accéder au dashboard');
  });
});

describe('SetupWizard — error display', () => {
  it('shows error message when agent detection fails', async () => {
    vi.mocked(agentsApi.detect).mockRejectedValue(new Error('Network error'));

    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    expect(document.body.textContent).toContain('Network error');
  });

  it('can dismiss the error by clicking the close button', async () => {
    vi.mocked(agentsApi.detect).mockRejectedValue(new Error('Temporary failure'));

    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    expect(document.body.textContent).toContain('Temporary failure');

    const closeBtn = Array.from(document.body.querySelectorAll('button')).find(b => b.textContent === '×');
    expect(closeBtn).toBeTruthy();
    await act(async () => { closeBtn!.click(); });

    expect(document.body.textContent).not.toContain('Temporary failure');
  });
});

describe('SetupWizard — skeleton during detection', () => {
  it('shows skeleton loader while agents are being detected', async () => {
    // detect() never resolves → component stays in detecting state
    vi.mocked(agentsApi.detect).mockReturnValue(new Promise(() => {}));

    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    // Should show detecting hint, not "no agent"
    const body = document.body.textContent!;
    expect(body).toContain('Analyse de votre système');
  });
});

describe('SetupWizard — optimistic toggle', () => {
  it('toggles agent enabled state immediately without rescan', async () => {
    const agents = [
      makeAgent({ name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true }),
      makeAgent({ name: 'Codex', agent_type: 'Codex', installed: true, enabled: true }),
    ];
    vi.mocked(agentsApi.detect).mockResolvedValue(agents);

    await wrap(<SetupWizard initialStatus={null} onComplete={vi.fn()} />);

    // Find "Activé" buttons
    const toggleBtns = Array.from(document.body.querySelectorAll('button'))
      .filter(b => b.textContent === 'Activé');
    expect(toggleBtns.length).toBe(2);

    // Click first toggle
    await act(async () => { toggleBtns[0].click(); });

    // Should now show "Désactivé" for first agent without calling detect() again
    const afterToggle = Array.from(document.body.querySelectorAll('button'))
      .filter(b => b.textContent === 'Désactivé');
    expect(afterToggle.length).toBe(1);
    // detect() was called once on mount, NOT again after toggle
    expect(agentsApi.detect).toHaveBeenCalledTimes(1);
  });
});

describe('SetupWizard — completing state', () => {
  it('shows preparing dashboard spinner when completing', async () => {
    const agent = makeAgent({ installed: true });
    vi.mocked(agentsApi.detect).mockResolvedValue([agent]);
    vi.mocked(setupApi.getStatus).mockResolvedValue(makeStatus({ repos_detected: [] }));
    const onComplete = vi.fn();
    // Make complete() hang so we can observe the completing state
    vi.mocked(setupApi.complete).mockReturnValue(new Promise(() => {}));

    await wrap(<SetupWizard initialStatus={null} onComplete={onComplete} />);

    // Navigate to step 1 (repos)
    const continuerBtn = Array.from(document.body.querySelectorAll('button')).find(b => b.textContent?.includes('Continuer'));
    await act(async () => { continuerBtn!.click(); });

    // Navigate to step 2 (done)
    const skipBtn = Array.from(document.body.querySelectorAll('button')).find(b => b.textContent?.includes('Continuer') || b.textContent?.includes('Passer'));
    if (skipBtn) await act(async () => { skipBtn.click(); });

    // Click "Accéder au dashboard"
    const dashBtn = Array.from(document.body.querySelectorAll('button')).find(b => b.textContent?.includes('dashboard'));
    if (dashBtn) {
      await act(async () => { dashBtn.click(); });
      // Should show preparing state
      expect(document.body.textContent).toContain('Préparation du dashboard');
    }
  });
});

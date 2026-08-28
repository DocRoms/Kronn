import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, act, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { TourProvider } from '../../components/tour/TourProvider';

// Mock API
vi.mock('../../lib/api', () => ({
  setAuthToken: vi.fn(),
  config: {
    // 0.10.0 — SettingsPage renders <ContinualLearningSection> which reads this.
    getContinualLearningEnabled: vi.fn().mockResolvedValue(false),
    saveContinualLearningEnabled: vi.fn().mockResolvedValue(undefined),
    getTokens: vi.fn().mockResolvedValue({ keys: [], overrides: {} }),
    dbInfo: vi.fn().mockResolvedValue({
      size_bytes: 1024,
      project_count: 5,
      discussion_count: 12,
      message_count: 150,
      mcp_count: 3,
      workflow_count: 2,
      workflow_run_count: 8,
      custom_skill_count: 4,
      custom_profile_count: 2,
      custom_directive_count: 1,
    }),
    getScanDepth: vi.fn().mockResolvedValue(4),
    getScanPaths: vi.fn().mockResolvedValue(['/home/user/repos']),
    getScanIgnore: vi.fn().mockResolvedValue(['node_modules', '.git']),
    getServerConfig: vi.fn().mockResolvedValue({ host: '127.0.0.1', port: 3140, domain: null, max_concurrent_agents: 5, agent_stall_timeout_min: 5, agent_global_timeout_min: 30, local_agent_global_timeout_min: 240, auth_enabled: true, discussion_notes_enabled: true }),
    setServerConfig: vi.fn().mockResolvedValue(undefined),
    getNetworkExposure: vi.fn().mockResolvedValue({ exposed: false, restart_required: false, port: 3140, reachable_ips: [] }),
    setNetworkExposure: vi.fn().mockResolvedValue({ exposed: false, restart_required: false, port: 3140, reachable_ips: [] }),
    getRecoveryStatus: vi.fn().mockResolvedValue({ configured: false }),
    setRecovery: vi.fn().mockResolvedValue({ recovery_code: 'KRECOV1.mock.mock' }),
    restoreRecovery: vi.fn().mockResolvedValue(undefined),
    regenerateAuthToken: vi.fn().mockResolvedValue('new-token-456'),
    saveApiKey: vi.fn(),
    deleteApiKey: vi.fn(),
    activateApiKey: vi.fn(),
    syncAgentTokens: vi.fn(),
    toggleTokenOverride: vi.fn(),
    getLanguage: vi.fn(),
    saveLanguage: vi.fn(),
    getUiLanguage: vi.fn().mockResolvedValue('fr'),
    saveUiLanguage: vi.fn().mockResolvedValue(undefined),
    getSttModel: vi.fn().mockResolvedValue(null),
    saveSttModel: vi.fn().mockResolvedValue(undefined),
    getTtsVoices: vi.fn().mockResolvedValue({}),
    saveTtsVoice: vi.fn().mockResolvedValue(undefined),
    getGlobalContext: vi.fn().mockResolvedValue(''),
    saveGlobalContext: vi.fn().mockResolvedValue(undefined),
    getGlobalContextMode: vi.fn().mockResolvedValue('always'),
    saveGlobalContextMode: vi.fn().mockResolvedValue(undefined),
    getAntiHallucinationMode: vi.fn().mockResolvedValue('warn'),
    saveAntiHallucinationMode: vi.fn().mockResolvedValue(undefined),
    setScanDepth: vi.fn(),
    setScanPaths: vi.fn(),
    setScanIgnore: vi.fn(),
    getAgentAccess: vi.fn(),
    setAgentAccess: vi.fn(),
    getModelTiers: vi.fn().mockResolvedValue({
      claude_code: { economy: null, reasoning: null },
      codex: { economy: null, reasoning: null },
      gemini_cli: { economy: null, reasoning: null },
      kiro: { economy: null, reasoning: null },
      vibe: { economy: null, reasoning: null },
      copilot_cli: { economy: null, reasoning: null },
      ollama: { economy: null, reasoning: null },
    }),
    setModelTiers: vi.fn().mockResolvedValue(undefined),
    exportData: vi.fn(),
    importData: vi.fn(),
    discoverKeys: vi.fn().mockResolvedValue({ discovered: [], imported_count: 0 }),
  },
  // KT-337 — AgentsSection loads the NVIDIA catalogue on mount (header status +
  // model datalist). This file mocks the api module by hand, so the namespace has
  // to be here too or the mount reaches the network and the page never settles.
  nvidia: {
    models: vi.fn().mockResolvedValue({ models: [], endpoint: 'https://integrate.api.nvidia.com', has_key: false }),
    probe: vi.fn().mockResolvedValue({ model: '', verdict: 'Usable', detail: '' }),
  },
  // KT-339 — the unified External API zone lists its connections on mount, so
  // this hand-written mock needs the namespace or the mount would throw.
  externalApi: {
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn().mockResolvedValue({}),
    update: vi.fn().mockResolvedValue({}),
    remove: vi.fn().mockResolvedValue(null),
  },
  agents: {
    detect: vi.fn(),
    install: vi.fn(),
    uninstall: vi.fn(),
    toggle: vi.fn(),
  },
  stats: {
    agentUsage: vi.fn().mockResolvedValue([
      { agent_type: 'ClaudeCode', total_tokens: 5000, message_count: 10, by_project: [] },
    ]),
  },
  skills: {
    list: vi.fn().mockResolvedValue([
      { id: 'rust', name: 'Rust', description: 'Systems programming', icon: 'Zap', category: 'Language', content: 'Be concise.', is_builtin: true },
      { id: 'custom-security', name: 'Security', description: 'Security auditing', icon: 'Shield', category: 'Domain', content: 'Focus on security.', is_builtin: false },
    ]),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
  },
  projects: {
    setDefaultSkills: vi.fn().mockResolvedValue(true),
    setDefaultProfile: vi.fn().mockResolvedValue(true),
  },
  mcps: {
    hostDiscovery: vi.fn().mockResolvedValue([]),
    adoptHost: vi.fn().mockResolvedValue(undefined),
  },
  profiles: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
    updatePersonaName: vi.fn(),
  },
  directives: {
    list: vi.fn().mockResolvedValue([
      { id: 'dir-terse', name: 'Terse', description: 'Short answers', icon: 'MessageSquare', category: 'Output', content: 'Be brief.', is_builtin: true, conflicts: ['dir-verbose'] },
    ]),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
  },
  portableLibrary: {
    state: vi.fn().mockResolvedValue({ scope: 'global', items: [], drift: 'not_applicable', approved: false }),
    sync: vi.fn().mockResolvedValue({}),
    check: vi.fn().mockResolvedValue({}),
    approve: vi.fn().mockResolvedValue(true),
    migrate: vi.fn().mockResolvedValue({ created: [], unchanged: [] }),
    export: vi.fn().mockResolvedValue([]),
    import: vi.fn().mockResolvedValue({}),
  },
  autoTriggersApi: {
    listDisabled: vi.fn().mockResolvedValue([]),
    toggle: vi.fn().mockResolvedValue(false),
  },
  // AgentsSection probes /api/health to gate the Install button under Docker.
  health: {
    get: vi.fn().mockResolvedValue({ ok: true, version: 'test', host_os: 'test', in_docker: false }),
  },
  contacts: {
    networkInfo: vi.fn().mockResolvedValue({ tailscale_ip: null, advertised_host: null, detected_ips: [] }),
    list: vi.fn().mockResolvedValue([]),
    add: vi.fn(),
    delete: vi.fn(),
    inviteCode: vi.fn().mockResolvedValue('kronn:test@localhost:3456'),
    ping: vi.fn().mockResolvedValue(false),
  },
  usage: {
    get: vi.fn().mockResolvedValue({
      period_kind: 'daily',
      rows: [],
      totals: {
        input_tokens: 0, output_tokens: 0, cache_creation_tokens: 0,
        cache_read_tokens: 0, total_tokens: 0, total_cost: 0,
      },
      agents_detected: [],
    }),
  },
  debugApi: {
    getLogs: vi.fn().mockResolvedValue({
      lines: [],
      buffered: 0,
      capacity: 1_000,
    }),
    clearLogs: vi.fn().mockResolvedValue(undefined),
  },
  userContext: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
    upsert: vi.fn(),
    delete: vi.fn(),
  },
}));

import { SettingsPage } from '../SettingsPage';
import { config as configApi } from '../../lib/api';
import type { AgentsConfig, AgentDetection } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';

const noop = () => {};
const toastFn: ToastFn = vi.fn();

const sampleAgent: AgentDetection = {
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
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

afterEach(() => {
  cleanup();
  localStorage.clear();
});

const wrap = async (ui: React.ReactElement) => {
  let result: ReturnType<typeof render>;
  await act(async () => {
    result = render(
      <I18nProvider>
        <TourProvider setPage={noop}>{ui}</TourProvider>
      </I18nProvider>,
    );
  });
  // Wait for async data to settle (useApi hooks resolve in microtasks)
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
  return result!;
};

const defaultProps = {
  agents: [] as AgentDetection[],
  agentAccess: null as AgentsConfig | null,
  configLanguage: null as string | null,
  projects: [],
  refetchAgents: noop,
  refetchAgentAccess: noop,
  refetchLanguage: noop,
  refetchProjects: noop,
  refetchDiscussions: noop,
  onReset: noop,
  toast: toastFn,
};

describe('SettingsPage', () => {
  it('exposes a stable deep-link target for agent collaboration settings', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);

    expect(document.getElementById('settings-agent-handoffs')).toBeTruthy();
  });

  it('persists the discussion-note composer toggle', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    const toggle = screen.getByRole('switch', { name: 'Notes de discussion hors contexte' });
    expect(toggle).toHaveAttribute('aria-checked', 'true');

    fireEvent.click(toggle);

    await waitFor(() => expect(configApi.setServerConfig).toHaveBeenCalledWith({
      discussion_notes_enabled: false,
    }));
    expect(toggle).toHaveAttribute('aria-checked', 'false');
  });

  it('renders all main sections', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);
    const body = document.body.textContent!;

    // Configuration heading
    expect(body).toContain('Configuration');
    // Database section
    expect(body).toContain('Base de données');
    // Agents section
    expect(body).toContain('Agents');
    // Skills section
    expect(body).toContain('Skills');
    // Directives section
    expect(body).toContain('Directives');
    // Profiles section
    expect(body).toContain('Profils agent');
    expect(body).toContain('chaque step Agent');
    expect(body).toContain('chaque carte agent');
  });

  it('renders a persistent section index and marks the selected destination', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);

    const nav = screen.getByRole('navigation', { name: 'Sections' });
    expect(nav).toBeTruthy();
    expect(document.querySelector('.set-layout')).toBeTruthy();
    expect(document.body.textContent).toContain('Centre de contrôle');
    expect(document.body.textContent).toContain('1/1 agents actifs');
    expect(document.body.textContent).toContain('1 racine scannée');

    const buttons = [...nav.querySelectorAll<HTMLButtonElement>('.set-nav-btn')];
    const preferences = buttons.find(button => button.textContent?.includes('Interface & langues'));
    const database = buttons.find(button => button.textContent?.includes('Base de données'));
    expect(buttons.filter(button => button.getAttribute('aria-current') === 'location')).toHaveLength(1);
    expect(database).toBeTruthy();

    fireEvent.click(database!);
    expect(database!.getAttribute('aria-current')).toBe('location');
    expect(preferences?.hasAttribute('aria-current')).toBe(false);
  });

  it('keeps agent capabilities directly after agents and the two beta sections', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);

    const content = document.querySelector('.set-content');
    const sectionIds = [...(content?.children ?? [])]
      .map(child => child.id)
      .filter(Boolean);
    expect(sectionIds.slice(0, 5)).toEqual([
      'settings-identity',
      'settings-agent-config',
      'settings-sourcing',
      'settings-continual-learning',
      'settings-capabilities',
    ]);
  });

  it('combines appearance and both language settings in one preference card', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);

    const preferences = document.querySelector('#settings-appearance');
    expect(preferences).toBeTruthy();
    expect(preferences?.textContent).toContain('Interface & langues');
    expect(preferences?.textContent).toContain('Apparence');
    expect(preferences?.textContent).toContain("Langue de l'interface");
    expect(preferences?.textContent).toContain('Langue de sortie');
    expect(preferences?.querySelector('#settings-languages')).toBeTruthy();
    expect(document.querySelector('.set-content > #settings-languages')).toBeNull();
  });

  it('explains the output-language scope for launched agents and external CLIs', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);

    fireEvent.click(screen.getByRole('button', { name: 'Comment cette langue est-elle appliquée ?' }));
    expect(screen.getByText(/chaque exécution d’un agent lancé par l’application/)).toBeInTheDocument();
    expect(screen.getByText(/agent CLI externe déjà rattaché/)).toBeInTheDocument();
  });

  it('groups the legacy lower settings into workspace and system areas', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);

    const groups = [...document.querySelectorAll('.set-content-group-intro h2')].map(node => node.textContent);
    expect(groups).toEqual(['Expérience & projets', 'Système & données']);
    expect(document.querySelector('#settings-user-context + .set-content-group-intro')).toBeTruthy();
  });

  it('separates agent skills from MCP connections in the capabilities card', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);

    const capabilities = document.querySelector('#settings-capabilities');
    expect(capabilities).toBeTruthy();
    expect(capabilities?.textContent).toContain('Capacités des agents');

    const groups = [...(capabilities?.children ?? [])]
      .filter(child => child.classList.contains('set-capabilities-group'));
    expect(groups).toHaveLength(2);
    expect(groups[0]).toHaveAttribute('data-kind', 'agent');
    expect(groups[0].textContent).toContain('Compétences et comportements');
    expect([...groups[0].querySelectorAll('.set-accordion-section')].map(section => section.id)).toEqual([
      'settings-skills',
      'settings-profiles',
      'settings-directives',
    ]);
    expect(groups[1]).toHaveAttribute('data-kind', 'mcp');
    expect(groups[1].textContent).toContain('Connexions MCP');
    expect(groups[1].querySelector('#settings-host-mcps')).toBeTruthy();
  });

  it('explains capability types, origins and current global scope', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);
    fireEvent.click(screen.getByRole('button', { name: 'Comprendre les capacités' }));

    expect(screen.getByText(/Skill : procédure/)).toBeInTheDocument();
    expect(screen.getByText(/Profil agent : rôle/)).toBeInTheDocument();
    expect(screen.getByText(/Directive : règle/)).toBeInTheDocument();
    expect(screen.getByText(/actuellement globales/)).toBeInTheDocument();
  });

  it('filters the unified capability area by origin and type', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);
    document.querySelectorAll('.set-accordion-header').forEach(button => fireEvent.click(button));
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });

    const originGroup = screen.getByRole('group', { name: 'Provenance' });
    const personalButton = [...originGroup.querySelectorAll('button')]
      .find(button => button.textContent === 'Personnel');
    fireEvent.click(personalButton!);

    expect(document.body.textContent).not.toContain('Systems programming');
    expect(document.body.textContent).toContain('Security auditing');
    expect(document.body.textContent).not.toContain('Short answers');

    const typeGroup = screen.getByRole('group', { name: 'Type' });
    const directivesButton = [...typeGroup.querySelectorAll('button')]
      .find(button => button.textContent === 'Directives');
    fireEvent.click(directivesButton!);
    expect(document.querySelector('#settings-skills')).toBeNull();
    expect(document.querySelector('#settings-profiles')).toBeNull();
    expect(document.querySelector('#settings-directives')).toBeTruthy();

    const allOriginsButton = [...originGroup.querySelectorAll('button')]
      .find(button => button.textContent === 'Tous');
    fireEvent.click(allOriginsButton!);
    expect(document.body.textContent).toContain('Short answers');
  });

  it('shows the current version under the settings menu with stable release links', async () => {
    await wrap(<SettingsPage {...defaultProps} />);

    const nav = screen.getByRole('navigation', { name: 'Sections' });
    const versionCard = nav.querySelector('[data-testid="settings-nav-version"]');
    expect(versionCard).toBeTruthy();
    expect(versionCard!.textContent).toContain('Kronn v');

    const links = [...versionCard!.querySelectorAll<HTMLAnchorElement>('a')];
    expect(links).toHaveLength(2);
    expect(links[0].href).toBe('https://github.com/DocRoms/Kronn/releases');
    expect(links[1].href).toBe('https://github.com/DocRoms/Kronn');
    expect(links[1].textContent).toBe('Source code (AGPL-3.0)');
  });

  it('shows a fully-clickable guided-tour progress CTA below the version card', async () => {
    await wrap(<SettingsPage {...defaultProps} />);

    const nav = screen.getByRole('navigation', { name: 'Sections' });
    const versionCard = nav.querySelector('[data-testid="settings-nav-version"]');
    const desktopCta = versionCard?.nextElementSibling
      ?.querySelector<HTMLButtonElement>('[data-testid="settings-tour-progress"]');
    expect(desktopCta).toBeTruthy();
    expect(desktopCta?.querySelector('[role="progressbar"]')).toHaveAttribute('aria-valuenow', '0');

    fireEvent.click(desktopCta!);
    expect(screen.queryAllByTestId('settings-tour-progress')).toHaveLength(0);
  });

  it('renders skill cards after opening accordion', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    // Skills are behind an accordion — find and click the header
    const allButtons = document.querySelectorAll('.set-accordion-header');
    // Open ALL accordions to make content visible
    allButtons.forEach(btn => fireEvent.click(btn));
    // Wait for re-render
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });
    const body = document.body.textContent!;

    expect(body).toContain('Rust');
    expect(body).toContain('Systems programming');
    expect(body).toContain('Security');
    expect(body).toContain('Security auditing');

    const rustCard = [...document.querySelectorAll('.set-item-card')]
      .find(card => card.textContent?.includes('Systems programming'));
    const securityCard = [...document.querySelectorAll('.set-item-card')]
      .find(card => card.textContent?.includes('Security auditing'));
    expect(rustCard).toHaveAttribute('data-origin', 'kronn');
    expect(rustCard?.textContent).toContain('Kronn');
    expect(securityCard).toHaveAttribute('data-origin', 'personal');
    expect(securityCard?.textContent).toContain('Personnel');
  });

  it('custom skill card exposes Edit + Delete; builtin only Delete', async () => {
    // Regression: before 2026-04-17, custom skills could only be deleted,
    // not edited — users had to delete+recreate for a typo fix.
    await wrap(<SettingsPage {...defaultProps} />);
    const allButtons = document.querySelectorAll('.set-accordion-header');
    allButtons.forEach(btn => fireEvent.click(btn));
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    // Builtin skill (Rust) → no action buttons at all in its card header.
    const rustCard = [...document.querySelectorAll('.set-item-card')].find(
      card => card.textContent?.includes('Rust'),
    );
    expect(rustCard).toBeTruthy();
    expect(rustCard!.querySelector('button[title="Modifier ce skill"]')).toBeNull();

    // Custom skill (Security) → Edit button visible with the i18n title.
    const secCard = [...document.querySelectorAll('.set-item-card')].find(
      card => card.textContent?.includes('Security auditing'),
    );
    expect(secCard).toBeTruthy();
    const editBtn = secCard!.querySelector('button[title="Modifier ce skill"]');
    expect(editBtn).toBeTruthy();
  });

  it('renders directive cards after opening accordion', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    // Directives are behind an accordion — open all
    const allButtons = document.querySelectorAll('.set-accordion-header');
    allButtons.forEach(btn => fireEvent.click(btn));
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });
    const body = document.body.textContent!;

    expect(body).toContain('Terse');
    expect(body).toContain('Short answers');
    const directiveCard = [...document.querySelectorAll('.set-item-card')]
      .find(card => card.textContent?.includes('Short answers'));
    expect(directiveCard).toHaveAttribute('data-origin', 'kronn');
  });

  it('DB info shows all counters when > 0', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    const body = document.body.textContent!;

    // Check counter values are rendered
    expect(body).toContain('5');   // project_count
    expect(body).toContain('12');  // discussion_count
    expect(body).toContain('150'); // message_count
    expect(body).toContain('3');   // mcp_count

    // Check labels (French default)
    expect(body).toContain('Projets');
    expect(body).toContain('Discussions');
    expect(body).toContain('Messages');
    expect(body).toContain('Plugins');
    expect(body).toContain('Workflows');
    expect(body).toContain('Skills custom');
    expect(body).toContain('Profils custom');
    expect(body).toContain('Directives custom');
  });

  it('export button exists', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    expect(screen.getByText('Exporter')).toBeTruthy();
  });

  it('renders scan configuration sections in the same card', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    const body = document.body.textContent!;
    expect(body).toContain('Profondeur de scan');
    expect(body).toContain('Dossiers à scanner');
    expect(body).toContain('Patterns à ignorer');
  });

  it('renders the Usage section in settings', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);
    // ccusage-based card with stable test id + powered-by attribution
    expect(document.querySelector('[data-testid="usage-section"]')).toBeTruthy();
    expect(document.body.textContent!).toContain('ccusage');
  });

  it('renders the auto-detect button for API keys', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);
    expect(screen.getByText('Auto-détecter')).toBeDefined();
  });

  it('renders Vibe agent with API key management section', async () => {
    const vibeAgent: AgentDetection = {
      ...sampleAgent,
      name: 'Vibe',
      agent_type: 'Vibe',
    };
    await wrap(<SettingsPage {...defaultProps} agents={[vibeAgent]} />);
    const body = document.body.textContent!;
    expect(body).toContain('Vibe');
    expect(body).toContain('auth locale');
    expect(body).toContain('Ajouter une clé');
  });

  it('does NOT render per-project default skills section', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);
    const body = document.body.textContent!;
    expect(body).not.toContain('Skills par défaut par projet');
    expect(body).not.toContain('Default skills per project');
  });

  it('does NOT render per-project default profiles section', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);
    const body = document.body.textContent!;
    expect(body).not.toContain('Profil par défaut par projet');
    expect(body).not.toContain('Default profile per project');
  });

  it('keeps Usage inside Agents instead of exposing a separate nav section', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);
    const navButtons = document.querySelectorAll('.set-nav-btn');
    const labels = Array.from(navButtons).map(b => b.textContent);
    expect(labels).not.toContain('Usage');
    expect(document.querySelector('#settings-agent-config [data-testid="usage-section"]')).toBeTruthy();
    const defaults = document.querySelector('[data-testid="agent-defaults"]');
    const economyGrid = document.querySelector('.set-agent-economy-grid');
    expect(economyGrid?.querySelector('[data-testid="usage-section"]')).toBeTruthy();
    expect(defaults?.nextElementSibling).toBe(economyGrid);
    expect(economyGrid?.nextElementSibling).toHaveClass('set-agent-list-head');
    expect(document.querySelector('.set-agents-section')?.lastElementChild).toHaveClass('set-best-practices');
  });

  it('shows add key form when clicking Ajouter une cle', async () => {
    // ClaudeCode has a token field (anthropic), so the "Ajouter une cle" button should appear
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);

    // The "Ajouter une cle" button should be visible for ClaudeCode
    const addKeyBtn = screen.getByText('Ajouter une clé');
    expect(addKeyBtn).toBeTruthy();

    // Click it to show the add key form
    await act(async () => { fireEvent.click(addKeyBtn); });

    // After clicking, the input fields for name and key should appear
    const nameInput = document.querySelector('input[placeholder="Nom de la clé"]') as HTMLInputElement;
    expect(nameInput).toBeTruthy();

    const keyInput = document.querySelector('input[type="password"]') as HTMLInputElement;
    expect(keyInput).toBeTruthy();
  });

  it('renders Usage section with always-visible period toggle', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    // Daily / Weekly / Monthly period filter is in the head (always visible)
    expect(document.querySelector('[data-testid="usage-period-daily"]')).toBeTruthy();
    expect(document.querySelector('[data-testid="usage-period-weekly"]')).toBeTruthy();
    expect(document.querySelector('[data-testid="usage-period-monthly"]')).toBeTruthy();
    // No compatible logs: show the explicit empty state instead of an empty
    // details drawer.
    expect(document.querySelector('[data-testid="usage-empty"]')).toBeTruthy();
    expect(document.querySelector('[data-testid="usage-details-toggle"]')).toBeNull();
  });

  it('renders bio textarea in Identity section', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    const body = document.body.textContent!;
    expect(body).toContain('Bio');
  });

  it('renders CopilotCli agent when detected', async () => {
    const copilotAgent = {
      ...sampleAgent,
      name: 'GitHub Copilot',
      agent_type: 'CopilotCli' as const,
    };
    await wrap(<SettingsPage {...defaultProps} agents={[copilotAgent]} />);
    const body = document.body.textContent!;
    expect(body).toContain('GitHub Copilot');
  });

  it('max-agents slider reverts to the previous value and toasts when the save fails', async () => {
    // Silent-error fix (2026-07): the slider was optimistic with no refetch,
    // so a failed setServerConfig left the UI at a value the backend never
    // persisted. Pin: explicit revert + visible error toast.
    const { config } = await import('../../lib/api');
    (config.setServerConfig as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('boom'));
    await wrap(<SettingsPage {...defaultProps} />);

    const slider = screen.getByLabelText('Agents locaux simultanés max') as HTMLInputElement;
    // Seeded from the getServerConfig mock (max_concurrent_agents: 5).
    expect(slider.value).toBe('5');

    await act(async () => { fireEvent.change(slider, { target: { value: '9' } }); });
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    expect(slider.value).toBe('5'); // reverted, not stuck at 9
    expect(toastFn).toHaveBeenCalledWith(expect.stringContaining('boom'), 'error');
  });

  it('loads and persists the absolute agent execution timeout independently', async () => {
    await wrap(<SettingsPage {...defaultProps} />);

    const globalSlider = screen.getByLabelText("Durée maximale d'une exécution agent") as HTMLInputElement;
    const localGlobalSlider = screen.getByLabelText('Durée maximale des agents locaux (Ollama)') as HTMLInputElement;
    const stallSlider = screen.getByLabelText("Timeout d'inactivité agent") as HTMLInputElement;
    const concurrencySlider = screen.getByLabelText('Agents locaux simultanés max') as HTMLInputElement;
    const agentsCard = document.getElementById('settings-agent-config');
    const serverCard = document.getElementById('settings-server');
    expect(agentsCard).toContainElement(concurrencySlider);
    expect(agentsCard).toContainElement(globalSlider);
    expect(agentsCard).toContainElement(localGlobalSlider);
    expect(agentsCard).toContainElement(stallSlider);
    expect(serverCard).not.toContainElement(globalSlider);
    expect(globalSlider.value).toBe('30');
    expect(localGlobalSlider.value).toBe('240');
    expect(stallSlider.value).toBe('5');

    await act(async () => { fireEvent.change(globalSlider, { target: { value: '120' } }); });

    await waitFor(() => expect(configApi.setServerConfig).toHaveBeenCalledWith({
      agent_global_timeout_min: 120,
    }));
    expect(globalSlider.value).toBe('120');
    expect(localGlobalSlider.value).toBe('240');
    expect(stallSlider.value).toBe('5');

    await act(async () => { fireEvent.change(localGlobalSlider, { target: { value: '180' } }); });
    await waitFor(() => expect(configApi.setServerConfig).toHaveBeenCalledWith({
      local_agent_global_timeout_min: 180,
    }));
    expect(localGlobalSlider.value).toBe('180');
  });

  it('warns when inactivity can never outlast the absolute execution limit', async () => {
    (configApi.getServerConfig as ReturnType<typeof vi.fn>).mockResolvedValue({
      host: '127.0.0.1',
      port: 3140,
      domain: null,
      max_concurrent_agents: 5,
      agent_stall_timeout_min: 120,
      agent_global_timeout_min: 30,
      local_agent_global_timeout_min: 240,
      auth_enabled: true,
      discussion_notes_enabled: true,
    });
    await wrap(<SettingsPage {...defaultProps} />);

    expect(document.body.textContent).toContain(
      "le plafond global interrompra l'agent avant le timeout d'inactivité",
    );
  });
});

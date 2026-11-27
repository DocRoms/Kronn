/**
 * KT-1923 — the api module as the Settings page reads it on mount, for the
 * sub-page specs. A module of its own that imports nothing of the app, so a
 * hoisted `vi.mock('../../lib/api', async () => (await import('./settingsApiMock')).settingsApiMock())`
 * loads it before the app does.
 */
import { vi } from 'vitest';

export function settingsApiMock() {
  const serverConfig = {
    host: '127.0.0.1', port: 3140, domain: null, max_concurrent_agents: 5, agent_stall_timeout_min: 5,
    agent_global_timeout_min: 30, local_agent_global_timeout_min: 240, auth_enabled: true,
    discussion_notes_enabled: true, execution_variable_retention_days: 30, run_payload_retention_days: 30,
  };
  const exposure = { exposed: false, restart_required: false, port: 3140, reachable_ips: [], auth_token_set: false };
  return {
    setAuthToken: vi.fn(),
    config: {
      getContinualLearningEnabled: vi.fn().mockResolvedValue(false),
      saveContinualLearningEnabled: vi.fn().mockResolvedValue(undefined),
      getTokens: vi.fn().mockResolvedValue({ keys: [], overrides: {} }),
      dbUsage: vi.fn().mockResolvedValue({ file_bytes: 1024, wal_bytes: 0, free_bytes: 0, tables: [] }),
      dbInfo: vi.fn().mockResolvedValue({
        size_bytes: 1024, project_count: 5, discussion_count: 12, message_count: 150, mcp_count: 3,
        workflow_count: 2, workflow_run_count: 8, custom_skill_count: 4, custom_profile_count: 2, custom_directive_count: 1,
      }),
      getScanDepth: vi.fn().mockResolvedValue(4),
      getScanPaths: vi.fn().mockResolvedValue(['/home/user/repos']),
      getScanIgnore: vi.fn().mockResolvedValue(['node_modules', '.git']),
      getServerConfig: vi.fn().mockResolvedValue(serverConfig),
      setServerConfig: vi.fn().mockResolvedValue(undefined),
      getNetworkExposure: vi.fn().mockResolvedValue(exposure),
      setNetworkExposure: vi.fn().mockResolvedValue(exposure),
      getRecoveryStatus: vi.fn().mockResolvedValue({ configured: false }),
      setRecovery: vi.fn().mockResolvedValue({ recovery_code: 'KRECOV1.mock.mock' }),
      restoreRecovery: vi.fn().mockResolvedValue(undefined),
      regenerateAuthToken: vi.fn().mockResolvedValue('new-token-456'),
      saveApiKey: vi.fn(), deleteApiKey: vi.fn(), activateApiKey: vi.fn(), syncAgentTokens: vi.fn(), toggleTokenOverride: vi.fn(),
      getLanguage: vi.fn(), saveLanguage: vi.fn(),
      getUiLanguage: vi.fn().mockResolvedValue('fr'), saveUiLanguage: vi.fn().mockResolvedValue(undefined),
      getSttModel: vi.fn().mockResolvedValue(null), saveSttModel: vi.fn().mockResolvedValue(undefined),
      getTtsVoices: vi.fn().mockResolvedValue({}), saveTtsVoice: vi.fn().mockResolvedValue(undefined),
      getGlobalContext: vi.fn().mockResolvedValue(''), saveGlobalContext: vi.fn().mockResolvedValue(undefined),
      getGlobalContextMode: vi.fn().mockResolvedValue('always'), saveGlobalContextMode: vi.fn().mockResolvedValue(undefined),
      getAntiHallucinationMode: vi.fn().mockResolvedValue('warn'), saveAntiHallucinationMode: vi.fn().mockResolvedValue(undefined),
      setScanDepth: vi.fn(), setScanPaths: vi.fn(), setScanIgnore: vi.fn(),
      getAgentAccess: vi.fn(), setAgentAccess: vi.fn(),
      getAgentFallback: vi.fn().mockResolvedValue({ chains: [], cooldowns: [] }),
      getModelTiers: vi.fn().mockResolvedValue({
        claude_code: { economy: null, reasoning: null }, codex: { economy: null, reasoning: null },
        gemini_cli: { economy: null, reasoning: null }, kiro: { economy: null, reasoning: null },
        vibe: { economy: null, reasoning: null }, copilot_cli: { economy: null, reasoning: null },
        ollama: { economy: null, reasoning: null },
      }),
      setModelTiers: vi.fn().mockResolvedValue(undefined),
      exportData: vi.fn(), importData: vi.fn(),
      discoverKeys: vi.fn().mockResolvedValue({ discovered: [], imported_count: 0 }),
    },
    comfyui: {
      status: vi.fn().mockResolvedValue({
        state: 'not_found', endpoint: 'http://127.0.0.1:8188', version: null, models: [], image_model: null,
        video_model: null, video_blocker: null, install_command: null, download_url: 'https://www.comfy.org/download',
        suggestions: [], install_dir: '/home/user/ComfyUI-Installs', configured_endpoint: null, configured_install_dir: null,
        image_settings: {}, video_settings: {}, family_defaults: [], samplers: [], schedulers: [],
      }),
      setSlots: vi.fn(), setSettings: vi.fn(), setModelFamily: vi.fn(), source: vi.fn().mockResolvedValue(null), pull: vi.fn(),
    },
    ollama: {
      health: vi.fn().mockResolvedValue({ status: 'not_installed', endpoint: 'http://localhost:11434', models: [], hint: null }),
    },
    nvidia: {
      models: vi.fn().mockResolvedValue({ models: [], endpoint: 'https://integrate.api.nvidia.com', has_key: false }),
      probe: vi.fn().mockResolvedValue({ model: '', verdict: 'Usable', detail: '' }),
    },
    externalApi: {
      list: vi.fn().mockResolvedValue([]), create: vi.fn().mockResolvedValue({}),
      update: vi.fn().mockResolvedValue({}), remove: vi.fn().mockResolvedValue(null),
    },
    modelCatalogApi: {
      list: vi.fn().mockResolvedValue({ targets: [] }),
      refresh: vi.fn().mockResolvedValue({ targets: [] }),
      createManual: vi.fn(), updateManual: vi.fn(), deleteManual: vi.fn(),
    },
    agents: {
      detect: vi.fn(), versionCheck: vi.fn().mockResolvedValue([]), quotaStates: vi.fn().mockResolvedValue([]),
      rearmQuota: vi.fn().mockResolvedValue(true), install: vi.fn(), uninstall: vi.fn(), toggle: vi.fn(),
    },
    stats: { agentUsage: vi.fn().mockResolvedValue([]) },
    rtk: {
      version: vi.fn().mockResolvedValue({
        available: false, installed: null, latest_known: null, checked_at: null, check_error: null,
        update_available: false, update_command: '',
        ccusage: { installed: null, latest: null, checked_at: null, check_error: null, update_available: false },
      }),
    },
    skills: {
      list: vi.fn().mockResolvedValue([
        { id: 'rust', name: 'Rust', description: 'Systems programming', icon: 'Zap', category: 'Language', content: 'Be concise.', is_builtin: true, token_estimate: 0 },
        { id: 'custom-security', name: 'Security', description: 'Security auditing', icon: 'Shield', category: 'Domain', content: 'Focus on security.', is_builtin: false, token_estimate: 0 },
      ]),
      create: vi.fn(), update: vi.fn(), delete: vi.fn(),
    },
    projects: { setDefaultSkills: vi.fn().mockResolvedValue(true), setDefaultProfile: vi.fn().mockResolvedValue(true) },
    mcps: { hostDiscovery: vi.fn().mockResolvedValue([]), adoptHost: vi.fn().mockResolvedValue(undefined) },
    profiles: {
      list: vi.fn().mockResolvedValue([]), get: vi.fn(), create: vi.fn(), update: vi.fn(), delete: vi.fn(), updatePersonaName: vi.fn(),
    },
    directives: {
      list: vi.fn().mockResolvedValue([
        { id: 'dir-terse', name: 'Terse', description: 'Short answers', icon: 'M', category: 'Output', content: 'Be brief.', is_builtin: true, conflicts: [], token_estimate: 0 },
      ]),
      create: vi.fn(), update: vi.fn(), delete: vi.fn(),
    },
    autoTriggersApi: { listDisabled: vi.fn().mockResolvedValue([]), toggle: vi.fn().mockResolvedValue(false) },
    health: { get: vi.fn().mockResolvedValue({ ok: true, version: 'test', host_os: 'test', in_docker: false }) },
    contacts: {
      networkInfo: vi.fn().mockResolvedValue({ tailscale_ip: null, advertised_host: null, detected_ips: [] }),
      list: vi.fn().mockResolvedValue([]), add: vi.fn(), delete: vi.fn(),
      inviteCode: vi.fn().mockResolvedValue('kronn:test@localhost:3456'), ping: vi.fn().mockResolvedValue(false),
    },
    usage: {
      get: vi.fn().mockResolvedValue({
        period_kind: 'daily', rows: [],
        totals: { input_tokens: 0, output_tokens: 0, cache_creation_tokens: 0, cache_read_tokens: 0, total_tokens: 0, total_cost: 0 },
        agents_detected: [],
      }),
    },
    telemetry: { coverage: vi.fn().mockResolvedValue([]) },
    measuredRatio: vi.fn(() => null),
    debugApi: {
      getLogs: vi.fn().mockResolvedValue({ lines: [], buffered: 0, capacity: 1_000 }),
      clearLogs: vi.fn().mockResolvedValue(undefined),
    },
    apiCallLogs: { list: vi.fn().mockResolvedValue([]) },
    userContext: { list: vi.fn().mockResolvedValue([]), get: vi.fn(), upsert: vi.fn(), delete: vi.fn() },
    trust: {
      overview: vi.fn().mockResolvedValue({
        instance: { scope: 'instance' }, effects: [], never_covered: [],
        sandbox: { available: true, protect_vault: true, protected: true },
        proof: {
          enrolled: false, pending: false, recovery_codes_left: 0, locked_for_secs: 0,
          admin_secret_path: '/data/human-admin-secret', data_dir: '/data', in_docker: false,
        },
      }),
      audit: vi.fn().mockResolvedValue([]),
      instance: vi.fn().mockResolvedValue({
        setting: { scope: 'instance' }, effective: { level: 'standard', source: 'instance', explicit: false },
      }),
    },
  };
}

import type {
  McpConfigDisplay,
  McpIncompleteConfig,
  McpLastProbe,
  McpProbeResponse,
  PluginInterface,
  ProbeDiagnosticCode,
} from '../../types/generated';

export type PluginHealthState = 'ok' | 'warning' | 'error';
export type PluginHealthTranslate = (key: string, ...args: (string | number)[]) => string;

export interface PluginAccessHealth {
  access: PluginInterface;
  state: PluginHealthState;
  code: ProbeDiagnosticCode | 'never_tested' | 'missing_secret';
  testedAt: string | null;
}

export function healthStateLabel(t: PluginHealthTranslate, state: PluginHealthState): string {
  return t(`mcp.health.${state}`);
}

export function diagnosticLabel(t: PluginHealthTranslate, code: PluginAccessHealth['code']): string {
  return t(`mcp.probeDiagnostic.${code}`);
}

interface PluginHealthContext {
  liveProbe?: McpProbeResponse;
  liveTestedAt?: string;
  incomplete?: McpIncompleteConfig;
  hasEndpointDrift?: boolean;
}

const HEALTH_RANK: Record<PluginHealthState, number> = {
  ok: 0,
  warning: 1,
  error: 2,
};

function latestStoredProbe(config: McpConfigDisplay, access: PluginInterface): McpLastProbe | undefined {
  return config.last_probes
    .filter(probe => probe.access === access)
    .sort((left, right) => right.tested_at.localeCompare(left.tested_at))[0];
}

export function accessHealth(
  config: McpConfigDisplay,
  access: PluginInterface,
  context: PluginHealthContext = {},
): PluginAccessHealth {
  if (config.secrets_broken || context.incomplete) {
    return { access, state: 'error', code: 'missing_secret', testedAt: null };
  }

  const liveCheck = context.liveProbe?.checks.find(check => check.id === access);
  if (liveCheck) {
    return {
      access,
      state: liveCheck.ok ? 'ok' : liveCheck.required ? 'error' : 'warning',
      code: liveCheck.code,
      testedAt: context.liveTestedAt ?? null,
    };
  }

  const stored = latestStoredProbe(config, access);
  if (!stored) {
    return { access, state: 'warning', code: 'never_tested', testedAt: null };
  }
  return {
    access,
    state: stored.ok ? 'ok' : 'error',
    code: stored.code,
    testedAt: stored.tested_at,
  };
}

export function configHealth(
  config: McpConfigDisplay,
  context: PluginHealthContext = {},
): PluginHealthState {
  let state: PluginHealthState = config.registry_drift || context.hasEndpointDrift ? 'warning' : 'ok';
  for (const access of config.interfaces) {
    const next = accessHealth(config, access, context).state;
    if (HEALTH_RANK[next] > HEALTH_RANK[state]) state = next;
  }
  return state;
}

export function visibleToPluginProject(config: McpConfigDisplay, projectId: string): boolean {
  if (projectId === '__all__') return true;
  if (projectId === '__none__') return config.is_global || config.include_general || config.project_ids.length === 0;
  return config.is_global || config.project_ids.includes(projectId);
}

export function latestPluginTest(config: McpConfigDisplay): string | null {
  return config.last_probes.reduce<string | null>((latest, probe) => (
    !latest || probe.tested_at > latest ? probe.tested_at : latest
  ), null);
}

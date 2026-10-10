import type { AgentReadiness, AgentType } from '../types/generated';
import { AGENT_LABELS } from './constants';

type Translate = (key: string, ...args: (string | number)[]) => string;

/** ✅ ready, ❌ not ready, ❔ unknown (or still checking). */
export function readinessIcon(result: AgentReadiness | undefined): string {
  if (!result) return '…';
  if (result.status === 'ready') return '✅';
  if (result.status === 'not_ready') return '❌';
  return '❔';
}

/** The result's reason in the user's language. */
export function readinessMessage(result: AgentReadiness, t: Translate): string {
  const label = AGENT_LABELS[result.agent_type] ?? result.agent_type;
  let message: string;
  switch (result.reason) {
    case 'session_timeout':
      message = t(result.message_key, label, result.secs ?? 0);
      break;
    case 'session_failed':
      message = t(result.message_key, label, result.detail ?? '');
      break;
    default:
      message = t(result.message_key, label);
  }
  if (result.servers.length > 0) {
    message += ` ${t('readiness.servers', result.servers.join(', '))}`;
  }
  return message;
}

export function blockingAgents(results: AgentReadiness[]): AgentType[] {
  return results.filter(result => result.status === 'not_ready').map(result => result.agent_type);
}

/** Where the user fixes this reason: project MCP servers, or the agent's settings. */
export function readinessFixTarget(result: AgentReadiness): { page: string; scrollTo?: string } {
  if (result.reason === 'session_timeout' || result.reason === 'session_failed') {
    return { page: 'mcps' };
  }
  return { page: 'settings', scrollTo: 'settings-agent-config' };
}

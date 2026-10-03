import { AlertTriangle, Check, X } from 'lucide-react';
import { diagnosticLabel, healthStateLabel } from './pluginHealth';
import type { PluginAccessHealth, PluginHealthTranslate } from './pluginHealth';

export function PluginHealthBadge({
  health,
  t,
  compact = false,
}: {
  health: PluginAccessHealth;
  t: PluginHealthTranslate;
  compact?: boolean;
}) {
  const Icon = health.state === 'ok' ? Check : health.state === 'error' ? X : AlertTriangle;
  const testedAt = health.testedAt ? new Date(health.testedAt) : null;
  const testedLabel = testedAt && !Number.isNaN(testedAt.getTime())
    ? testedAt.toLocaleString()
    : t('mcp.neverTested');
  return <span className="mcp-access-health" data-state={health.state}>
    <span className="mcp-access-health-state"><Icon size={11} />{healthStateLabel(t, health.state)}</span>
    {!compact && <>
      <span className="mcp-access-health-diagnostic">{diagnosticLabel(t, health.code)}</span>
      <time dateTime={health.testedAt ?? undefined}>{t('mcp.lastTestAt', testedLabel)}</time>
    </>}
  </span>;
}

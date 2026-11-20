import type { McpConfigDisplay } from '../../types/generated';
import type { PluginHealthTranslate } from './pluginHealth';

function shown(at: string): string {
  const date = new Date(at);
  return Number.isNaN(date.getTime()) ? at : date.toLocaleString();
}

/** KT-1692 — what the connection says of its token: the last proactive check,
 *  or, for a connection Kronn cannot probe, when it last worked. */
export function tokenCheckSummary(
  cfg: Pick<McpConfigDisplay, 'token_probe' | 'token_check'>,
  t: PluginHealthTranslate,
): { state: 'ok' | 'refused' | 'inconclusive' | 'not_checked' | 'never'; text: string } {
  const check = cfg.token_check;
  if (cfg.token_probe && check?.checked_at && check.outcome) {
    const when = shown(check.checked_at);
    switch (check.outcome) {
      case 'ok': return { state: 'ok', text: t('mcp.tokenCheck.ok', when) };
      case 'refused': return { state: 'refused', text: t('mcp.tokenCheck.refused', when) };
      case 'inconclusive': return { state: 'inconclusive', text: t('mcp.tokenCheck.inconclusive', when) };
    }
  }
  if (check?.last_good_at) {
    return { state: 'not_checked', text: t('mcp.tokenCheck.notChecked', shown(check.last_good_at)) };
  }
  return { state: 'never', text: t('mcp.tokenCheck.never') };
}

import { KeyRound, RefreshCw } from 'lucide-react';
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

/** The connection card's token line and its "Check" action (only for a
 *  connection that has a probe). */
export function TokenCheckStatus({
  cfg,
  t,
  checking,
  onCheck,
}: {
  cfg: McpConfigDisplay;
  t: PluginHealthTranslate;
  checking: boolean;
  onCheck: () => void;
}) {
  const summary = tokenCheckSummary(cfg, t);
  return (
    <section className="mcp-detail-section mcp-token-check" data-testid="mcp-token-check" data-detail-tab="status" data-state={summary.state}>
      <h3 className="mcp-detail-section-title"><KeyRound size={12} /> {t('mcp.tokenCheck.title')}</h3>
      <p data-testid="mcp-token-check-summary">{summary.text}</p>
      {cfg.token_check?.detail && summary.state !== 'ok' && summary.state !== 'not_checked' && summary.state !== 'never' && (
        <p className="mcp-token-refused-message">{cfg.token_check.detail}</p>
      )}
      {cfg.token_probe ? (
        <button type="button" className="mcp-btn-action" onClick={onCheck} disabled={checking} data-testid="mcp-token-check-button">
          <RefreshCw size={12} className={checking ? 'spin' : undefined} />
          {t(checking ? 'mcp.tokenCheck.checking' : 'mcp.tokenCheck.check')}
        </button>
      ) : (
        <small data-testid="mcp-token-check-noprobe">{t('mcp.tokenCheck.noProbe')}</small>
      )}
    </section>
  );
}

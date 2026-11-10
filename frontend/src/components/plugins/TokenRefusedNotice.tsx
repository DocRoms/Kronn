import { AlertTriangle, Key } from 'lucide-react';
import type { McpAuthFailure } from '../../types/generated';
import type { PluginHealthTranslate } from './pluginHealth';

/** KT-1524 — the connection card says the provider refuses its token (or that
 *  the token could not be renewed), when it happened, what the provider
 *  answered, and offers the one action that cures it. */
export function TokenRefusedNotice({
  failure,
  t,
  onUpdate,
}: {
  failure: McpAuthFailure;
  t: PluginHealthTranslate;
  onUpdate: () => void;
}) {
  const failedAt = new Date(failure.failed_at);
  const when = Number.isNaN(failedAt.getTime()) ? failure.failed_at : failedAt.toLocaleString();
  const status = failure.http_status != null ? ` (HTTP ${failure.http_status})` : '';
  return (
    <div className="mcp-token-refused" role="alert" data-testid="mcp-token-refused" data-detail-tab="status">
      <AlertTriangle size={16} aria-hidden="true" />
      <div>
        <strong>{t(`mcp.authFailure.title.${failure.kind}`)}{status}</strong>
        <p>{t('mcp.authFailure.since', when)}</p>
        {failure.message && <p className="mcp-token-refused-message">{t('mcp.authFailure.provider', failure.message)}</p>}
        <button type="button" className="mcp-btn-action" onClick={onUpdate} data-testid="mcp-token-refused-update">
          <Key size={12} /> {t('mcp.authFailure.update')}
        </button>
      </div>
    </div>
  );
}

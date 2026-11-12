import type { ReactNode } from 'react';

/** Why a catalogue could not be refreshed, as the backend names it. */
const STALE_REASONS = [
  'disappeared', 'auth_required', 'timeout', 'cli_missing', 'invalid_catalog',
  'provider_error', 'unsupported', 'not_found', 'access_denied',
] as const;

interface Props {
  t: (key: string, ...args: (string | number)[]) => string;
  loading: boolean;
  /** The catalogue could not be loaded at all. */
  failed: boolean;
  /** The catalogue loaded and knows no model for this target. */
  empty: boolean;
  /** The last refresh of this target failed or is overdue. */
  stale: boolean;
  /** The backend's reason for it, said in plain words, never as the raw code. */
  reason?: string | null;
  reloading?: boolean;
  onReload: () => void;
}

/** KT-1706 — what the catalogue says about the model pickers of a card (loading, load
 *  failure, nothing known, stale): ONE muted line, drawn inside the « Models and
 *  fallbacks » block under its title. Null when there is nothing to say. */
export function CatalogHint({ t, loading, failed, empty, stale, reason, reloading, onReload }: Props) {
  const parts: ReactNode[] = [];
  if (loading) parts.push(<span key="loading" role="status">{t('common.loading')}</span>);
  if (failed) {
    parts.push(
      <span key="failed" role="alert">
        {t('modelCatalog.loadError')}{' '}
        <button type="button" className="set-icon-btn" disabled={reloading} onClick={onReload}>{t('modelCatalog.reload')}</button>
      </span>,
    );
  }
  if (empty) parts.push(<span key="empty">{t('modelCatalog.empty')}</span>);
  if (stale) {
    const known = STALE_REASONS.find(code => code === reason);
    parts.push(<span key="stale">{t(known ? `modelCatalog.staleHint.${known}` : 'modelCatalog.staleHint.unknown')}</span>);
  }
  if (parts.length === 0) return null;
  return <p className="set-agent-fallback-hint" data-testid="catalog-hint">{parts}</p>;
}

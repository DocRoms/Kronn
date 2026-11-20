import { X } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import type { NotCarried } from '../types/generated';
import './ExportNotCarriedDialog.css';

export interface ExportNotCarriedDialogProps {
  items: NotCarried[];
  onDownload: () => void;
  onCancel: () => void;
}

const KINDS = ['skill', 'directive', 'profile', 'page', 'workflow', 'quick_prompt', 'quick_api', 'quick_exec'];

/**
 * Shown before an export leaves (KT-1843): what the exported content names
 * and the file does not hold. The file is downloaded only from here.
 */
export function ExportNotCarriedDialog({ items, onDownload, onCancel }: ExportNotCarriedDialogProps) {
  const { t } = useT();
  return (
    <div className="dash-modal-overlay" onClick={onCancel}>
      <div
        className="dash-modal export-not-carried"
        role="dialog"
        aria-modal="true"
        aria-labelledby="export-not-carried-title"
        data-testid="export-not-carried"
        onClick={e => e.stopPropagation()}
        onKeyDown={e => { if (e.key === 'Escape') onCancel(); }}
      >
        <div className="dash-modal-header">
          <h3 id="export-not-carried-title" className="dash-modal-title">{t('exp.notCarried.title', items.length)}</h3>
          <button type="button" onClick={onCancel} className="dash-modal-close" aria-label={t('common.close')}>
            <X size={16} />
          </button>
        </div>
        <p className="export-not-carried-intro">{t('exp.notCarried.intro')}</p>
        <ul className="export-not-carried-list">
          {items.map(item => (
            <li key={`${item.kind}:${item.id}:${item.named_in}`} className="export-not-carried-row" data-testid="export-not-carried-row">
              <span className="export-not-carried-kind">
                {KINDS.includes(item.kind) ? t(`exp.notCarried.kind.${item.kind}`) : item.kind}
              </span>
              <span className="export-not-carried-text">
                <code className="export-not-carried-id">{item.id}</code>
                <span className="export-not-carried-where">{t('exp.notCarried.namedIn', item.named_in)}</span>
              </span>
            </li>
          ))}
        </ul>
        <div className="export-not-carried-footer">
          <button type="button" className="btn btn-ghost" onClick={onCancel}>{t('common.cancel')}</button>
          <button type="button" className="btn btn-primary" data-testid="export-not-carried-download" onClick={onDownload} autoFocus>
            {t('exp.notCarried.download')}
          </button>
        </div>
      </div>
    </div>
  );
}

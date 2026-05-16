import { useEffect } from 'react';
import { Shield, Container, Zap, Eye, Accessibility, Database, Network, X } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import type { AuditKind } from '../types/AuditKind';

interface SubAuditOption {
  kind: Exclude<AuditKind, 'Full' | 'Drift' | 'Custom'>;
  icon: typeof Shield;
}

const SUB_AUDIT_OPTIONS: SubAuditOption[] = [
  { kind: 'Security',      icon: Shield },
  { kind: 'Docker',        icon: Container },
  { kind: 'Performance',   icon: Zap },
  { kind: 'Accessibility', icon: Eye },
  { kind: 'Rgaa',          icon: Accessibility },
  { kind: 'Database',      icon: Database },
  { kind: 'ApiDesign',     icon: Network },
];

interface Props {
  open: boolean;
  onClose: () => void;
  onPick: (kind: Exclude<AuditKind, 'Drift' | 'Custom'>) => void;
  /**
   * When true, the modal opens directly on the sub-audit picker (no
   * Full-vs-Targeted prompt). Used from the "Validated" state where
   * the user already has a Full audit on file and just wants a
   * targeted re-scan.
   */
  targetedOnly?: boolean;
}

export default function SubAuditModal({ open, onClose, onPick, targetedOnly }: Props) {
  const { t } = useT();

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={t('audit.subAudit.modalLabel')}
      data-testid="sub-audit-modal"
      onClick={onClose}
      style={{
        position: 'fixed', inset: 0, zIndex: 999,
        background: 'rgba(0,0,0,0.55)',
        display: 'flex', alignItems: 'center', justifyContent: 'center',
        padding: 16,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          background: 'var(--kr-bg-card, #1d1f24)',
          border: '1px solid var(--kr-border-subtle, #2a2d33)',
          borderRadius: 8,
          padding: 20,
          maxWidth: 640,
          width: '100%',
          maxHeight: '85vh',
          overflowY: 'auto',
          color: 'var(--kr-text-primary, #e6e8eb)',
        }}
      >
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 12 }}>
          <h2 style={{ margin: 0, fontSize: 16 }}>{t('audit.subAudit.title')}</h2>
          <button
            type="button"
            onClick={onClose}
            aria-label={t('common.close')}
            data-testid="sub-audit-modal-close"
            style={{ background: 'transparent', border: 'none', color: 'inherit', cursor: 'pointer', padding: 4 }}
          >
            <X size={16} />
          </button>
        </div>
        <p style={{ margin: '0 0 16px 0', fontSize: 13, opacity: 0.85 }}>
          {t('audit.subAudit.intro')}
        </p>

        {!targetedOnly && (
          <button
            type="button"
            data-testid="sub-audit-pick-Full"
            onClick={() => { onPick('Full'); onClose(); }}
            style={{
              display: 'flex', width: '100%', alignItems: 'center', gap: 10,
              padding: '10px 12px', marginBottom: 8,
              background: 'var(--kr-bg-elevated, #24262c)',
              border: '1px solid var(--kr-border-subtle, #2a2d33)',
              borderRadius: 6, color: 'inherit', cursor: 'pointer', textAlign: 'left',
            }}
          >
            <span style={{ fontSize: 18 }}>🌐</span>
            <span>
              <strong>{t('audit.subAudit.fullTitle')}</strong>
              <br/>
              <span style={{ fontSize: 12, opacity: 0.75 }}>{t('audit.subAudit.fullDesc')}</span>
            </span>
          </button>
        )}

        <div style={{
          marginTop: targetedOnly ? 0 : 12,
          paddingTop: targetedOnly ? 0 : 12,
          borderTop: targetedOnly ? 'none' : '1px solid var(--kr-border-subtle, #2a2d33)',
        }}>
          <div style={{ fontSize: 12, opacity: 0.7, marginBottom: 6 }}>
            {t('audit.subAudit.targetedSectionTitle')}
          </div>
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(220px, 1fr))', gap: 8 }}>
            {SUB_AUDIT_OPTIONS.map((opt) => {
              const Icon = opt.icon;
              return (
                <button
                  key={opt.kind}
                  type="button"
                  data-testid={`sub-audit-pick-${opt.kind}`}
                  onClick={() => { onPick(opt.kind); onClose(); }}
                  style={{
                    display: 'flex', alignItems: 'flex-start', gap: 8,
                    padding: '10px 12px',
                    background: 'var(--kr-bg-elevated, #24262c)',
                    border: '1px solid var(--kr-border-subtle, #2a2d33)',
                    borderRadius: 6, color: 'inherit', cursor: 'pointer', textAlign: 'left',
                    fontSize: 13,
                  }}
                >
                  <Icon size={14} style={{ flexShrink: 0, marginTop: 2 }} />
                  <span>
                    <strong>{t(`audit.subAudit.kind.${opt.kind}.title`)}</strong>
                    <br/>
                    <span style={{ fontSize: 11, opacity: 0.75 }}>
                      {t(`audit.subAudit.kind.${opt.kind}.desc`)}
                    </span>
                  </span>
                </button>
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
}

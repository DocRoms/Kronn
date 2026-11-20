import { useState } from 'react';
import { useT } from '../lib/I18nContext';
import './DodEvidencePrompt.css';

interface Props {
  /** The criterion, read out with the field. */
  sentence: string;
  busy?: boolean;
  onConfirm: (evidence: string) => void;
  onCancel: () => void;
}

/** A post-integration criterion is ticked with the proof of its live check. */
export function DodEvidencePrompt({ sentence, busy = false, onConfirm, onCancel }: Props) {
  const { t } = useT();
  const [evidence, setEvidence] = useState('');
  const trimmed = evidence.trim();
  return (
    <form
      className="dod-evidence"
      data-testid="dod-evidence-prompt"
      onSubmit={event => {
        event.preventDefault();
        if (trimmed && !busy) onConfirm(trimmed);
      }}
    >
      <label>
        <span>{t('planning.dodEvidence')}</span>
        <textarea
          aria-label={t('planning.dodEvidenceFor', sentence)}
          autoFocus
          rows={2}
          value={evidence}
          placeholder={t('planning.dodEvidencePlaceholder')}
          onChange={event => setEvidence(event.target.value)}
          onKeyDown={event => {
            if (event.key === 'Escape') onCancel();
          }}
        />
      </label>
      <div>
        <button type="button" onClick={onCancel} disabled={busy}>{t('common.cancel')}</button>
        <button type="submit" data-primary="true" disabled={!trimmed || busy}>
          {t('planning.dodEvidenceConfirm')}
        </button>
      </div>
    </form>
  );
}

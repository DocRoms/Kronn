// KT-1496 — the dialog that asks for the strong confirmation inline, on any
// action that returns a secret or destroys data while an authenticator is
// enrolled. It says why, takes the code, and hands it back to the request that
// was refused (`lib/strongConfirm`). The code is spent on that one action.
import { useEffect, useId, useState, useSyncExternalStore } from 'react';
import { KeyRound } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { currentStrongPrompt, subscribeStrongPrompt } from '../lib/strongConfirm';
import './TrustLevel.css';

export function StrongConfirmDialog() {
  const prompt = useSyncExternalStore(subscribeStrongPrompt, currentStrongPrompt, currentStrongPrompt);
  if (!prompt) return null;
  // A new prompt is a new dialog: its input starts empty.
  return <Dialog key={`${prompt.method} ${prompt.path} ${prompt.message ?? ''}`} prompt={prompt} />;
}

function Dialog({ prompt }: { prompt: NonNullable<ReturnType<typeof currentStrongPrompt>> }) {
  const { t } = useT();
  const titleId = useId();
  const [code, setCode] = useState('');

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') prompt.answer(null);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [prompt]);

  const submit = () => {
    const trimmed = code.trim();
    if (trimmed !== '') prompt.answer(trimmed);
  };
  return (
    <div className="trust-backdrop" role="presentation" onClick={() => prompt.answer(null)}>
      <form
        className="trust-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        data-testid="strong-confirm-dialog"
        onClick={event => event.stopPropagation()}
        onSubmit={event => {
          event.preventDefault();
          submit();
        }}
      >
        <h3 id={titleId} className="trust-dialog-title">
          <KeyRound size={16} aria-hidden="true" />
          {t('strongConfirm.title')}
        </h3>
        <p className="trust-dialog-intro">{t('strongConfirm.why')}</p>
        <p className="set-meta">
          {t('strongConfirm.action', `${prompt.method} ${prompt.path}`)}
        </p>
        {prompt.message && (
          <p className="trust-blocked" role="alert" data-testid="strong-confirm-refused">
            {prompt.message}
          </p>
        )}
        <label className="trust-proof">
          <span>{t('strongConfirm.label')}</span>
          <input
            type="text"
            autoComplete="one-time-code"
            spellCheck={false}
            autoFocus
            value={code}
            onChange={event => setCode(event.target.value)}
            placeholder="123456"
            data-testid="strong-confirm-input"
          />
        </label>
        <div className="trust-dialog-actions">
          <button type="button" className="trust-btn" onClick={() => prompt.answer(null)} data-testid="strong-confirm-cancel">
            {t('common.cancel')}
          </button>
          <button
            type="submit"
            className="trust-btn trust-btn-danger"
            disabled={code.trim() === ''}
            data-testid="strong-confirm-submit"
          >
            {t('strongConfirm.confirm')}
          </button>
        </div>
      </form>
    </div>
  );
}

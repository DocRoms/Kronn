import { useState } from 'react';
import type { CSSProperties } from 'react';
import { Eye, EyeOff } from 'lucide-react';
import { useT } from '../lib/I18nContext';

/**
 * Single source of truth for rendering a secret/credential field across the
 * MCP/API page (refonte 2026-06-09). Replaces the 3-4 divergent "input + eye"
 * implementations and their scattered inline styles + visibility state.
 *
 * Three states, unambiguous by construction:
 *  - CREATE (no `stored`): editable input + show/hide eye.
 *  - STORED, not replacing: read-only masked `••••••••` + eye that PEEKS the
 *    stored value read-only (via `onRevealStored`, like the card) + a small
 *    "Remplacer" link. A stored secret is NEVER pre-filled into an editable
 *    input (a masked value can't round-trip into a real key — the desync bug).
 *  - REPLACING (`stored && replacing`): empty editable input + eye + "Annuler".
 *
 * The component owns ONLY its ephemeral display state (peeked value, show/hide).
 * The parent owns `value`, `stored`, `replacing` and the persistence.
 */
export interface SecretFieldProps {
  /** The editable value. In stored/not-replacing mode this stays '' until the user replaces. */
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  /** A value already exists server-side → masked + "Remplacer" until replaced. */
  stored?: boolean;
  /** Controlled: the user chose to replace the stored value (empty input shown). */
  replacing?: boolean;
  onReplace?: () => void;
  onCancelReplace?: () => void;
  /** Fetch the stored value for a read-only peek (the eye in stored mode). Returns the value or null. */
  onRevealStored?: () => Promise<string | null>;
}

const linkStyleBase: CSSProperties = {
  background: 'none',
  border: 'none',
  font: 'inherit',
  fontSize: '0.78em',
  cursor: 'pointer',
  padding: '0 0.4rem',
  whiteSpace: 'nowrap',
};

export function SecretField({
  value,
  onChange,
  placeholder,
  stored,
  replacing,
  onReplace,
  onCancelReplace,
  onRevealStored,
}: SecretFieldProps) {
  const { t } = useT();
  // Read-only peek of the stored value (stored mode only).
  const [peeked, setPeeked] = useState<string | null>(null);
  // Show/hide the value the user is typing (create / replacing).
  const [showTyped, setShowTyped] = useState(false);

  if (stored && !replacing) {
    const revealed = peeked != null;
    return (
      <>
        {/* Clicking INTO the field switches to replace mode (the field the
            user intends to type in). Live trap caught 2026-06-10: a peeked
            read-only value LOOKS editable — the user typed over it, keystrokes
            were silently swallowed, and the save wrote nothing. The eye stays
            a pure read-only peek (it's a sibling button, not the input). */}
        <input
          className="input mcp-input-mono"
          readOnly
          style={{ cursor: 'pointer' }}
          title={t('mcp.custom.replaceValue')}
          type={revealed ? 'text' : 'password'}
          value={revealed ? peeked! : '••••••••'}
          onChange={() => {}}
          onFocus={() => { setPeeked(null); onReplace?.(); }}
        />
        <button
          type="button"
          className="mcp-icon-btn"
          title={revealed ? t('mcp.hide') : t('mcp.show')}
          aria-label={revealed ? t('mcp.hide') : t('mcp.show')}
          onClick={async () => {
            if (revealed) {
              setPeeked(null);
              return;
            }
            if (!onRevealStored) return;
            try {
              const v = await onRevealStored();
              setPeeked(v ?? '');
            } catch {
              // best-effort peek; leave masked on failure
            }
          }}
        >
          {revealed
            ? <EyeOff size={12} style={{ color: 'var(--kr-accent-ink)' }} />
            : <Eye size={12} style={{ color: 'var(--kr-text-ghost)' }} />}
        </button>
        {onReplace && (
          <button
            type="button"
            style={{ ...linkStyleBase, color: 'var(--kr-accent-ink)' }}
            onClick={onReplace}
          >
            {t('mcp.custom.replaceValue')}
          </button>
        )}
      </>
    );
  }

  return (
    <>
      <input
        className="input mcp-input-mono"
        type={showTyped ? 'text' : 'password'}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder ?? t('mcp.custom.fieldValue')}
      />
      <button
        type="button"
        className="mcp-icon-btn"
        title={showTyped ? t('mcp.hide') : t('mcp.show')}
        aria-label={showTyped ? t('mcp.hide') : t('mcp.show')}
        onClick={() => setShowTyped((v) => !v)}
      >
        {showTyped
          ? <EyeOff size={12} style={{ color: 'var(--kr-accent-ink)' }} />
          : <Eye size={12} style={{ color: 'var(--kr-text-ghost)' }} />}
      </button>
      {stored && onCancelReplace && (
        <button
          type="button"
          style={{ ...linkStyleBase, color: 'var(--kr-text-ghost)' }}
          onClick={() => {
            setShowTyped(false);
            setPeeked(null);
            onCancelReplace();
          }}
        >
          {t('mcp.custom.cancelReplace')}
        </button>
      )}
    </>
  );
}

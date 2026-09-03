// KT-561 — deleting in two steps, where the control lives.
//
// A native `confirm()` is not a reliable gate: some contexts never show it and
// answer `false`, and the click then does nothing at all — no dialog, no
// deletion, no explanation. Worse, the callers awaited the request with no
// error handling, so a server refusal was silent too. Arming the button in
// place removes both: nothing is hidden behind a popup, and a refusal stays on
// screen next to the thing it refused.
import { useEffect, useId, useRef, useState } from 'react';
import { Loader2, Trash2 } from 'lucide-react';
import { userError } from '../lib/userError';
import './ConfirmDeleteButton.css';

export function ConfirmDeleteButton({
  onConfirm,
  label,
  confirmLabel,
  itemName,
  impact,
  impactUnknown,
  className,
  size = 12,
  testId,
  onError,
}: {
  /** Runs only on the second click. A rejection is shown, never swallowed. */
  onConfirm: () => Promise<void>;
  label: string;
  confirmLabel: string;
  /** Named in the accessible label, so a screen reader says WHICH one. */
  itemName: string;
  /**
   * What the deletion takes with it — past runs, steps that will break —
   * said while armed, before the second click. Resolved once per arming.
   */
  impact?: () => string | null | Promise<string | null>;
  /** Shown when `impact` fails: an unreadable count must not read as "nothing". */
  impactUnknown?: string;
  className?: string;
  size?: number;
  testId?: string;
  /** Also reported outside, when the surface has a toast of its own. */
  onError?: (message: string) => void;
}) {
  const [armed, setArmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [impactText, setImpactText] = useState<string | null>(null);
  const timer = useRef<number | null>(null);
  const arming = useRef(0);
  const impactId = useId();

  // An armed button that stays armed is a trap for the next click, which may
  // be aimed at something else entirely. The window restarts when the impact
  // lands, so a count that arrives late is still read before the button disarms.
  useEffect(() => {
    if (!armed) return;
    timer.current = window.setTimeout(() => setArmed(false), 5000);
    return () => { if (timer.current) window.clearTimeout(timer.current); };
  }, [armed, impactText]);

  useEffect(() => () => { if (timer.current) window.clearTimeout(timer.current); }, []);

  const arm = () => {
    setError(null);
    setImpactText(null);
    setArmed(true);
    if (!impact) return;
    // A result that belongs to an earlier arming must not describe this one.
    const seq = ++arming.current;
    const settle = (text: string | null) => { if (arming.current === seq) setImpactText(text); };
    let pending: string | null | Promise<string | null>;
    try {
      pending = impact();
    } catch {
      settle(impactUnknown ?? null);
      return;
    }
    if (pending instanceof Promise) {
      pending.then(settle, () => settle(impactUnknown ?? null));
    } else {
      settle(pending);
    }
  };

  const showImpact = armed && !busy && impactText !== null;

  return (
    <span className="kr-confirm-delete">
      <button
        type="button"
        className={className}
        data-danger="true"
        data-armed={armed}
        disabled={busy}
        title={armed ? confirmLabel : label}
        aria-label={`${armed ? confirmLabel : label} · ${itemName}`}
        aria-describedby={showImpact ? impactId : undefined}
        data-testid={testId}
        onClick={event => {
          event.stopPropagation();
          if (!armed) {
            arm();
            return;
          }
          setBusy(true);
          onConfirm()
            .then(() => setArmed(false))
            .catch((e: unknown) => {
              // Stays on screen: an object must never look deleted when the
              // server refused to delete it.
              const message = userError(e);
              setError(message);
              setArmed(false);
              onError?.(message);
            })
            .finally(() => setBusy(false));
        }}
      >
        {busy
          ? <Loader2 size={size} className="spin" />
          : armed ? <span className="kr-confirm-delete-word">{confirmLabel}</span> : <Trash2 size={size} />}
      </button>
      {showImpact && (
        <span id={impactId} className="kr-confirm-delete-impact" data-testid={testId ? `${testId}-impact` : undefined}>
          {impactText}
        </span>
      )}
      {error && (
        <span className="kr-confirm-delete-error" role="alert" data-testid={testId ? `${testId}-error` : undefined}>
          {error}
        </span>
      )}
    </span>
  );
}

// KT-1490 — a Quick Exec an agent saved that hands run values to a program
// Kronn does not model is saved inactive. A human activates it here, where
// they already are: the room that created it, a Page action, a workflow step.
// Activation is the human's only: it names the line that was read, and the
// server refuses it if the line changed since.
import { useEffect, useState } from 'react';
import { AlertTriangle, CheckCircle2, Loader2 } from 'lucide-react';
import { quickExecs as quickExecsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { workflowPath } from '../lib/routes';
import type { ActivationCardPayload } from '../lib/activationCard';
import { commandLine } from '../lib/commandLine';
import type { QuickExec } from '../types/generated';
import './QuickExecActivation.css';

/** Activates the line read, then reports it. */
async function activate(exec: QuickExec): Promise<void> {
  await quickExecsApi.activate(exec.id, { command: exec.command, args: exec.args });
}

type Phase = 'loading' | 'waiting' | 'activated' | 'missing' | 'refused';

interface CardProps {
  card: ActivationCardPayload;
}

/** The room's card: « Activer « X » : fastly reçoit des valeurs du run ». */
export function QuickExecActivationCard({ card }: CardProps) {
  const { t } = useT();
  const [phase, setPhase] = useState<Phase>('loading');
  const [exec, setExec] = useState<QuickExec | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [version, setVersion] = useState(0);

  useEffect(() => {
    let cancelled = false;
    Promise.all([quickExecsApi.list(), quickExecsApi.pendingApprovals()])
      .then(([execs, pending]) => {
        if (cancelled) return;
        const found = execs.find(item => item.id === card.quick_exec_id) ?? null;
        setExec(found);
        const waiting = found ? pending.find(item => item.id === found.id) : undefined;
        if (!found) setPhase('missing');
        else if (!waiting) setPhase('activated');
        else setPhase(waiting.refused ? 'refused' : 'waiting');
      })
      .catch(() => { if (!cancelled) setPhase('missing'); });
    return () => { cancelled = true; };
  }, [card.quick_exec_id, version]);

  // A line refused because it changed since it was read is read again, so the
  // card shows the new line the human is asked about.
  const onActivate = async () => {
    if (!exec || busy) return;
    setBusy(true);
    setError(null);
    try {
      await activate(exec);
      setPhase('activated');
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      setVersion(value => value + 1);
    } finally {
      setBusy(false);
    }
  };

  // The refusal is the server's to keep: a reload shows « Refusée » because
  // the server says so, not because this component remembers.
  const onRefuse = async () => {
    if (!exec || busy) return;
    setBusy(true);
    setError(null);
    try {
      await quickExecsApi.refuseActivation(exec.id, { command: exec.command, args: exec.args });
      setPhase('refused');
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      setVersion(value => value + 1);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section
      className="qe-activation-card"
      data-testid="qe-activation-card"
      data-phase={phase}
      aria-label={t('qeActivation.aria', card.name)}
    >
      {phase === 'loading' && <Loader2 size={12} aria-hidden="true" />}
      {phase === 'waiting' && exec && (
        <>
          <p className="qe-activation-card__title">
            <AlertTriangle size={12} aria-hidden="true" />
            <strong>{t('qeActivation.title', card.name, card.program)}</strong>
          </p>
          <pre className="qe-command qe-activation-card__line">{commandLine(exec.command, exec.args)}</pre>
          <p className="qe-activation-card__hint">{t('qeActivation.hint', card.program)}</p>
          <div className="qe-activation-card__actions">
            <button
              type="button"
              className="qe-activation-btn"
              data-kind="primary"
              disabled={busy}
              onClick={() => { void onActivate(); }}
            >
              {t('qeActivation.activate')}
            </button>
            <button
              type="button"
              className="qe-activation-btn"
              data-kind="secondary"
              disabled={busy}
              onClick={() => { void onRefuse(); }}
            >
              {t('qeActivation.refuse')}
            </button>
          </div>
          {error && <p role="alert" className="qe-activation-card__error">{error}</p>}
        </>
      )}
      {phase === 'activated' && (
        <p className="qe-activation-card__title">
          <CheckCircle2 size={12} aria-hidden="true" /> {t('qeActivation.done', card.name)}
        </p>
      )}
      {phase === 'refused' && <p className="qe-activation-card__title">{t('qeActivation.refused', card.name)}</p>}
      {phase === 'missing' && <p className="qe-activation-card__title">{t('qeActivation.missing', card.name)}</p>}
    </section>
  );
}

/** A Quick Exec waiting for a human, with the line the notice shows: the
 * activation names exactly this line, never one read later. */
export interface PendingActivation {
  id: string;
  name: string;
  program: string;
  command: string;
  args: string[];
}

/** An Exec line of a workflow step waiting for a human: activated in the
 * workflow's own page, where the line is edited. */
export interface PendingStepLine {
  workflowId: string;
  step: string;
}

interface NoticeProps {
  items: PendingActivation[];
  steps?: PendingStepLine[];
  /** Called once an item is activated, so the caller refreshes what it shows. */
  onActivated?: () => void;
  /** Called when the server refused the line shown (it changed since), so the
   * caller reads the new line and the notice shows it. */
  onRefresh?: () => void;
}

/** « en attente d'activation » with the activate button, where a Page action or
 * a workflow step would otherwise fail on an inactive Quick Exec. */
export function PendingActivationNotice({ items, steps = [], onActivated, onRefresh }: NoticeProps) {
  const { t } = useT();
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  if (items.length === 0 && steps.length === 0) return null;

  const onActivate = async (item: PendingActivation) => {
    if (busy) return;
    setBusy(item.id);
    setError(null);
    try {
      // The line shown is the line sent: a line that changed since is refused
      // by the server, and the notice is refreshed to show the new one.
      await quickExecsApi.activate(item.id, { command: item.command, args: item.args });
      onActivated?.();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      onRefresh?.();
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="qe-pending-activation" data-testid="qe-pending-activation" role="status">
      {items.map(item => (
        <div key={item.id} className="qe-pending-activation__item">
          <AlertTriangle size={12} aria-hidden="true" />
          <strong>{t('qeActivation.pending', item.name)}</strong>
          <span>{' — '}{t('qeActivation.receives', item.program)}</span>
          <pre className="qe-command">{commandLine(item.command, item.args)}</pre>
          <button
            type="button"
            className="qe-activation-btn"
            data-kind="primary"
            disabled={busy !== null}
            onClick={() => { void onActivate(item); }}
          >
            {t('qeActivation.activateNamed', item.name)}
          </button>
        </div>
      ))}
      {steps.map(line => (
        <div key={`${line.workflowId}:${line.step}`} className="qe-pending-activation__item">
          <AlertTriangle size={12} aria-hidden="true" />
          <strong>{t('qeActivation.stepPending', line.step)}</strong>
          <span>{' — '}</span>
          <a href={workflowPath(line.workflowId)} target="_blank" rel="noreferrer">
            {t('qeActivation.openWorkflow')}
          </a>
        </div>
      ))}
      {error && <p role="alert" className="qe-pending-activation__error">{error}</p>}
    </div>
  );
}

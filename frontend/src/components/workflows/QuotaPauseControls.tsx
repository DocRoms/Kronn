import { useEffect, useState } from 'react';
import { Clock, Loader2, RotateCcw } from 'lucide-react';
import { agents as agentsApi, config as configApi, workflows as workflowsApi } from '../../lib/api';
import { AGENT_LABELS } from '../../lib/constants';
import { useT } from '../../lib/I18nContext';
import { quotaPauseLabel, type QuotaWakeInfo } from '../../lib/quotaPause';
import type { AgentType } from '../../types/generated';
import './QuotaPauseControls.css';

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * "Resume now" for a run paused on a provider quota, reachable without opening
 * the run (KT-1657). `runId` is the run to resume: a sub-workflow child is
 * resumed through its parent.
 */
export function QuotaResumeButton({ runId, onResumed }: { runId: string; onResumed?: () => void }) {
  const { t } = useT();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const resume = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await workflowsApi.resumeRun(runId);
      onResumed?.();
    } catch (e) {
      setError(t('wf.quota.resumeFailed', errorText(e)));
    } finally {
      setBusy(false);
    }
  };

  return (
    <span className="wf-quota-resume">
      <button
        type="button"
        className="wf-quota-resume-btn"
        data-testid="quota-resume-now"
        disabled={busy}
        title={t('wf.quota.resumeHint')}
        onClick={(e) => { e.stopPropagation(); void resume(); }}
      >
        {busy ? <Loader2 size={10} className="spin" aria-hidden /> : <RotateCcw size={10} aria-hidden />}
        {busy ? t('wf.quota.resuming') : t('wf.quota.resumeNow')}
      </button>
      {error && <span className="wf-quota-resume-error" role="alert">{error}</span>}
    </span>
  );
}

/** The paused label of a run, with its "Resume now" action. */
export function QuotaPauseNotice({
  quota,
  resumeRunId,
  onResumed,
}: {
  quota?: QuotaWakeInfo | null;
  resumeRunId: string;
  onResumed?: () => void;
}) {
  const { t } = useT();
  return (
    <div className="wf-quota-pause" data-testid="quota-pause-notice" data-parked={quota && !quota.wake_at ? 'true' : undefined}>
      <Clock size={11} aria-hidden />
      <span className="wf-quota-pause-label">{quotaPauseLabel(t, quota)}</span>
      <QuotaResumeButton runId={resumeRunId} onResumed={onResumed} />
    </div>
  );
}

/**
 * Moves the refused step of a paused run to another installed agent. The run
 * keeps waiting: the switch is stored, then resumed by the person or woken by
 * the engine's provider re-check.
 */
export function QuotaAgentSwitch({
  runId,
  current,
  onSwitched,
}: {
  runId: string;
  current?: AgentType | null;
  onSwitched?: () => void;
}) {
  const { t } = useT();
  const [choices, setChoices] = useState<AgentType[]>([]);
  const [picked, setPicked] = useState('');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ kind: 'ok' | 'error'; text: string } | null>(null);

  useEffect(() => {
    let cancelled = false;
    void Promise.resolve().then(() => agentsApi.detect()).then(
      (list) => {
        if (cancelled) return;
        setChoices(
          (list ?? [])
            .filter(d => (d.installed || d.runtime_available) && d.enabled && d.agent_type !== 'Custom')
            .map(d => d.agent_type),
        );
      },
      () => { /* no list: the switch simply offers nothing */ },
    );
    return () => { cancelled = true; };
  }, []);

  // KT-1518 — the agent's own fallback chain is the alternative to waiting: its
  // first link is offered first, and preselected until the person picks another.
  const [fallbackAgent, setFallbackAgent] = useState<AgentType | null>(null);
  useEffect(() => {
    let cancelled = false;
    if (!current) return undefined;
    void Promise.resolve().then(() => configApi.getAgentFallback()).then(
      (overview) => {
        if (cancelled) return;
        const chain = overview?.chains.find(candidate => candidate.agent === current);
        setFallbackAgent(chain?.links[0]?.agent ?? null);
      },
      () => { /* no chain to show: the plain list stands */ },
    );
    return () => { cancelled = true; };
  }, [current]);

  const available = choices.filter(agent => agent !== current);
  const offered = fallbackAgent && available.includes(fallbackAgent) ? fallbackAgent : null;
  const options = offered ? [offered, ...available.filter(agent => agent !== offered)] : available;
  if (options.length === 0) return null;
  const selected = picked || offered || '';

  const label = (agent: string) => AGENT_LABELS[agent] ?? agent;
  const apply = async () => {
    if (!selected || busy) return;
    setBusy(true);
    setMessage(null);
    try {
      await workflowsApi.switchQuotaAgent(runId, { agent: selected as AgentType });
      setMessage({ kind: 'ok', text: t('wf.quota.switched', label(selected)) });
      onSwitched?.();
    } catch (e) {
      setMessage({ kind: 'error', text: t('wf.quota.switchFailed', errorText(e)) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="wf-quota-switch" data-testid="quota-agent-switch">
      <select
        aria-label={t('wf.quota.switchAgent')}
        title={t('wf.quota.switchAgentHint')}
        value={selected}
        disabled={busy}
        onChange={(e) => setPicked(e.target.value)}
      >
        <option value="">{t('wf.quota.switchAgentPick')}</option>
        {options.map(agent => (
          <option key={agent} value={agent}>
            {agent === offered ? `${label(agent)} (${t('wf.quota.configuredFallback')})` : label(agent)}
          </option>
        ))}
      </select>
      <button
        type="button"
        className="wf-quota-resume-btn"
        data-testid="quota-switch-apply"
        disabled={!selected || busy}
        title={t('wf.quota.switchAgentHint')}
        onClick={() => void apply()}
      >
        {t('wf.quota.switchAgent')}
      </button>
      {message && (
        <span
          className={message.kind === 'error' ? 'wf-quota-resume-error' : 'wf-quota-switch-ok'}
          role={message.kind === 'error' ? 'alert' : 'status'}
        >
          {message.text}
        </span>
      )}
    </div>
  );
}

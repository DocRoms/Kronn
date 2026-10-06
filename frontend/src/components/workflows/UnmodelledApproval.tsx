// KT-1017 — a run value reaching a line Kronn does not check alone (a
// program it does not model, or any line an agent wrote) is refused unless a
// human confirms it. One approval covers the whole step, so every line it
// covers is listed; the server keeps an agent's save from setting it.
import { useEffect, useState } from 'react';
import { workflows as workflowsApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';

interface UnmodelledApprovalProps {
  command: string;
  args: string[];
  /** The step's stdin template, which the same approval covers. */
  stdin?: string;
  /** The step's setup line, which the same approval covers. */
  setupCommand?: string;
  setupArgs?: string[];
  /** An agent last wrote these lines: any value needs the approval. */
  agentWritten?: boolean;
  /** Drop a stored approval this line does not need. Off when the step's
   *  approval also covers other lines (CollectApiData sources). */
  clearStale?: boolean;
  approved: boolean;
  onChange: (approved: boolean) => void;
}

export function UnmodelledApproval({
  command, args, stdin, setupCommand, setupArgs, agentWritten, clearStale = true, approved, onChange,
}: UnmodelledApprovalProps) {
  const { t } = useT();
  const [covered, setCovered] = useState<string[]>([]);
  const key = JSON.stringify([command, args, stdin ?? null, setupCommand ?? null, setupArgs ?? [], !!agentWritten]);

  useEffect(() => {
    if (!command.trim()) {
      setCovered([]);
      return;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      Promise.resolve()
        .then(() => workflowsApi.execLineCheck({
          command,
          args,
          ...(stdin ? { stdin } : {}),
          ...(setupCommand ? { setup_command: setupCommand, setup_args: setupArgs ?? [] } : {}),
          ...(agentWritten ? { agent_written: true } : {}),
        }))
        .then(check => {
          if (cancelled) return;
          const lines = check.covered ?? [];
          setCovered(lines);
          // An approval no line needs any more never stays behind.
          if (clearStale && lines.length === 0 && approved) onChange(false);
        })
        .catch(() => { if (!cancelled) setCovered([]); });
    }, 300);
    return () => { cancelled = true; clearTimeout(timer); };
    // `key` carries every line; `approved` is read, never a trigger.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  if (covered.length === 0) return null;
  const programs = [...new Set(covered.map(line => line.split(': ').slice(1).join(': ')))].join(', ');
  return (
    <label className="wf-unmodelled-approval">
      <input
        type="checkbox"
        checked={approved}
        onChange={event => onChange(event.target.checked)}
      />
      <span>
        {agentWritten ? `${t('exec.agentWrittenApprove')} ` : ''}
        {t('exec.unmodelledApprove', programs)}
      </span>
      <small>{t('exec.unmodelledCovers', covered.join(' · '))}</small>
    </label>
  );
}

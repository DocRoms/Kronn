// KT-1017 — a run value reaching a program Kronn does not model is refused
// unless a human confirms that program treats its arguments as plain data.
// The checkbox appears only for such a line; the server keeps an agent's
// save from setting it.
import { useEffect, useState } from 'react';
import { workflows as workflowsApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';

interface UnmodelledApprovalProps {
  command: string;
  args: string[];
  /** The step's stdin template, which an unmodelled program may also read. */
  stdin?: string;
  approved: boolean;
  onChange: (approved: boolean) => void;
}

export function UnmodelledApproval({ command, args, stdin, approved, onChange }: UnmodelledApprovalProps) {
  const { t } = useT();
  const [program, setProgram] = useState<string | null>(null);
  const key = JSON.stringify([command, args, stdin ?? null]);

  useEffect(() => {
    if (!command.trim()) {
      setProgram(null);
      return;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      Promise.resolve()
        .then(() => workflowsApi.execLineCheck(stdin ? { command, args, stdin } : { command, args }))
        .then(check => { if (!cancelled) setProgram(check.unmodelled_program ?? null); })
        .catch(() => { if (!cancelled) setProgram(null); });
    }, 300);
    return () => { cancelled = true; clearTimeout(timer); };
    // `key` carries command, args and stdin.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  if (!program) return null;
  return (
    <label className="wf-unmodelled-approval">
      <input
        type="checkbox"
        checked={approved}
        onChange={event => onChange(event.target.checked)}
      />
      <span>{t('exec.unmodelledApprove', program)}</span>
    </label>
  );
}

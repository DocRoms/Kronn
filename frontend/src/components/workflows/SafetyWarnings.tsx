import { useEffect, useState } from 'react';
import { ShieldAlert } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { workflows as workflowsApi } from '../../lib/api';
import type { SafetyCheckRequest, SafetyWarning } from '../../types/generated';
import './SafetyWarnings.css';

const WARNING_KEY: Record<SafetyWarning, string> = {
  sandbox_outside_container: 'wf.safetyWarning.sandbox_outside_container',
  limits_without_directory: 'wf.safetyWarning.limits_without_directory',
  limits_without_git: 'wf.safetyWarning.limits_without_git',
  approval_on_sub_workflow: 'wf.safetyWarning.approval_on_sub_workflow',
};

/** KT-1043 — Security settings a run on this host would refuse, said before any run. */
export function SafetyWarnings({ request }: { request: SafetyCheckRequest }) {
  const { t } = useT();
  const [warnings, setWarnings] = useState<SafetyWarning[]>([]);
  const key = JSON.stringify(request);

  useEffect(() => {
    let current = true;
    // A failed check shows nothing: the run itself still refuses with the reason.
    Promise.resolve()
      .then(() => workflowsApi.safetyCheck(JSON.parse(key) as SafetyCheckRequest))
      .then(found => { if (current) setWarnings(Array.isArray(found) ? found : []); })
      .catch(() => { if (current) setWarnings([]); });
    return () => { current = false; };
  }, [key]);

  if (warnings.length === 0) return null;
  return (
    <div className="wf-safety-warnings" role="alert" data-testid="wf-safety-warnings">
      {warnings.map(warning => (
        <div key={warning} className="wf-safety-warning">
          <ShieldAlert size={14} aria-hidden="true" />
          <span>{t(WARNING_KEY[warning])}</span>
        </div>
      ))}
    </div>
  );
}

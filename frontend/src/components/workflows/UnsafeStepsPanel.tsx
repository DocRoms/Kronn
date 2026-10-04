// KT-1017 — saved Exec steps that interpolate a value into inline code are
// refused at run time. This panel names them and, on request, shows the
// suggested positional-argument rewrite as a diff; nothing is applied until
// the user clicks Apply.
import { useEffect, useState } from 'react';
import { AlertTriangle } from 'lucide-react';
import { workflows as workflowsApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import type { UnsafeExecStep, Workflow } from '../../types/generated';

interface UnsafeStepsPanelProps {
  workflow: Workflow;
  onApply: (issue: UnsafeExecStep) => Promise<void>;
}

const issueKey = (issue: UnsafeExecStep) =>
  `${issue.on_failure ? 'rollback' : 'main'}:${issue.step_name}:${issue.phase}`;

export function UnsafeStepsPanel({ workflow, onApply }: UnsafeStepsPanelProps) {
  const { t } = useT();
  const [issues, setIssues] = useState<UnsafeExecStep[]>([]);
  const [open, setOpen] = useState<string | null>(null);
  const [applying, setApplying] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    Promise.resolve()
      .then(() => workflowsApi.unsafeSteps(workflow.id))
      .then(found => { if (!cancelled) setIssues(found); })
      .catch(() => { if (!cancelled) setIssues([]); });
    return () => { cancelled = true; };
  }, [workflow]);

  if (issues.length === 0) return null;

  const apply = async (issue: UnsafeExecStep) => {
    const key = issueKey(issue);
    if (applying) return;
    setApplying(key);
    try {
      await onApply(issue);
      setOpen(null);
    } finally {
      setApplying(null);
    }
  };

  return (
    <section className="wf-unsafe-panel" role="alert" aria-label={t('wf.unsafeTitle')}>
      <h4 className="wf-unsafe-title">
        <AlertTriangle size={14} />
        {t('wf.unsafeTitle')}
      </h4>
      <p className="wf-unsafe-intro">{t('wf.unsafeIntro')}</p>
      <ul className="wf-unsafe-list">
        {issues.map(issue => {
          const key = issueKey(issue);
          return (
            <li key={key} className="wf-unsafe-item">
              <div className="wf-unsafe-item-head">
                <span>
                  <strong>{issue.step_name}</strong>
                  {issue.phase === 'setup' ? ` (${t('wf.unsafeSetup')})` : ''}
                  {' — '}
                  <code>{issue.placeholder || issue.reason}</code>
                </span>
                <button
                  type="button"
                  className="wf-btn-secondary"
                  aria-expanded={open === key}
                  onClick={() => setOpen(open === key ? null : key)}
                >
                  {t('wf.unsafeSuggest')}
                </button>
              </div>
              {open === key && (
                issue.suggested_args ? (
                  <div className="wf-unsafe-fix">
                    <pre className="wf-unsafe-diff" aria-label={t('wf.unsafeDiff')}>
                      <span className="wf-unsafe-diff-old">{`- ${issue.command} ${JSON.stringify(issue.args)}`}</span>
                      {'\n'}
                      <span className="wf-unsafe-diff-new">{`+ ${issue.command} ${JSON.stringify(issue.suggested_args)}`}</span>
                    </pre>
                    <button
                      type="button"
                      className="wf-btn-secondary wf-unsafe-apply"
                      disabled={applying !== null}
                      onClick={() => { void apply(issue); }}
                    >
                      {t('wf.unsafeApply')}
                    </button>
                  </div>
                ) : (
                  <div className="wf-unsafe-fix">
                    <p className="wf-unsafe-manual">{t('wf.unsafeManual')}</p>
                    <p className="wf-unsafe-manual-reason">{issue.manual_fix}</p>
                  </div>
                )
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}

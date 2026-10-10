// KT-1017 — saved Exec steps that interpolate a value into inline code are
// refused at run time. This panel names them and, on request, shows the
// suggested positional-argument rewrite as a diff; nothing is applied until
// the user clicks Apply. When a step needs a manual fix, it offers a ready
// prompt to hand to an agent. KT-1138 — a line that only lacks a human's
// approval is told apart from a value inside code, and blocked sub-workflows
// or rollbacks are named so a saved chain is never mistaken for a ready one.
import { useEffect, useState } from 'react';
import { AlertTriangle } from 'lucide-react';
import { workflows as workflowsApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import type { UnsafeExecStep, Workflow, WorkflowBlocker, WorkflowReadiness } from '../../types/generated';

interface UnsafeStepsPanelProps {
  workflow: Workflow;
  onApply: (issue: UnsafeExecStep) => Promise<void>;
  /** KT-1017 — a human approves a step whose line needs it. */
  onApprove?: (issue: UnsafeExecStep) => Promise<void>;
}

const issueKey = (issue: UnsafeExecStep) =>
  `${issue.on_failure ? 'rollback' : 'main'}:${issue.step_name}:${issue.phase}:${issue.source_alias ?? ''}`;

const phaseLabel = (issue: UnsafeExecStep, t: (key: string, ...args: (string | number)[]) => string) => {
  if (issue.phase === 'setup') return ` (${t('wf.unsafeSetup')})`;
  if (issue.phase === 'stdin') return ' (stdin)';
  if (issue.phase === 'source') return ` (${t('wf.unsafeSource', issue.source_alias ?? '')})`;
  return '';
};

export function UnsafeStepsPanel({ workflow, onApply, onApprove }: UnsafeStepsPanelProps) {
  const { t } = useT();
  const [issues, setIssues] = useState<UnsafeExecStep[]>([]);
  const [open, setOpen] = useState<string | null>(null);
  const [applying, setApplying] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [readiness, setReadiness] = useState<WorkflowReadiness | null>(null);

  useEffect(() => {
    let cancelled = false;
    Promise.resolve()
      .then(() => workflowsApi.unsafeSteps(workflow.id))
      .then(found => { if (!cancelled) setIssues(found); })
      .catch(() => { if (!cancelled) setIssues([]); });
    Promise.resolve()
      .then(() => workflowsApi.readiness(workflow.id))
      .then(found => { if (!cancelled) setReadiness(found ?? null); })
      .catch(() => { if (!cancelled) setReadiness(null); });
    return () => { cancelled = true; };
  }, [workflow]);

  // Blockers that live in another workflow of the chain, grouped by workflow.
  const elsewhere = new Map<string, { name: string; total: number; human: number }>();
  // This workflow's blockers the unsafe-line list does not already show
  // (unpinned scripts, allowlist, destructive or missing config).
  const shown = (blocker: WorkflowBlocker) => issues.some(issue =>
    issue.step_name === blocker.step
    && issue.phase === blocker.phase
    && issue.on_failure === blocker.on_failure
    && issue.reason === blocker.reason);
  const local: WorkflowBlocker[] = [];
  for (const blocker of readiness?.blockers ?? []) {
    if (blocker.workflow_id === workflow.id) {
      if (!shown(blocker)) local.push(blocker);
      continue;
    }
    const entry = elsewhere.get(blocker.workflow_id) ?? { name: blocker.workflow_name, total: 0, human: 0 };
    entry.total += 1;
    if (blocker.human_only) entry.human += 1;
    elsewhere.set(blocker.workflow_id, entry);
  }

  if (issues.length === 0 && local.length === 0 && elsewhere.size === 0) return null;

  const codeIssues = issues.filter(issue => issue.reason !== 'unmodelled_program');
  const approvalIssues = issues.filter(issue => issue.reason === 'unmodelled_program');
  const needsHuman = approvalIssues.length > 0 || local.some(blocker => blocker.human_only);
  const title = codeIssues.length > 0
    ? t('wf.unsafeTitle')
    : needsHuman ? t('wf.approvalTitle') : t('wf.readinessTitle');

  // A line that needs a human's approval is not an agent's to work around.
  const manualIssues = issues.filter(
    issue => !issue.suggested_args && issue.reason !== 'unmodelled_program',
  );
  const agentPrompt = manualIssues.length === 0 ? '' : t(
    'wf.unsafeAgentPrompt',
    workflow.name,
    workflow.id,
    String(manualIssues.length),
    manualIssues
      .map(issue => {
        const what = issue.placeholder || issue.reason;
        const why = issue.manual_fix ? ` — ${issue.manual_fix}` : '';
        return `- ${issue.step_name}${phaseLabel(issue, t)} : ${what}${why}`;
      })
      .join('\n'),
  );

  const copyPrompt = async () => {
    try {
      await navigator.clipboard.writeText(agentPrompt);
      setCopied(true);
    } catch {
      // Clipboard can be refused; the prompt stays visible to copy by hand.
      setCopied(false);
    }
  };

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

  const approve = async (issue: UnsafeExecStep) => {
    if (applying || !onApprove) return;
    setApplying(issueKey(issue));
    try {
      await onApprove(issue);
      setOpen(null);
    } finally {
      setApplying(null);
    }
  };

  return (
    <section
      className="wf-unsafe-panel"
      role="alert"
      aria-label={title}
    >
      <h4 className="wf-unsafe-title">
        <AlertTriangle size={14} />
        {title}
      </h4>
      {codeIssues.length > 0 && <p className="wf-unsafe-intro" data-kind="code">{t('wf.unsafeIntro')}</p>}
      {approvalIssues.length > 0 && (
        <p className="wf-unsafe-intro" data-kind="approval">{t('wf.approvalIntro')}</p>
      )}
      {local.length > 0 && (
        <ul className="wf-unsafe-list" data-testid="readiness-local">
          {local.map((blocker, index) => (
            <li
              key={`${blocker.on_failure ? 'rollback' : 'main'}:${blocker.step ?? ''}:${blocker.phase ?? ''}:${blocker.kind}:${index}`}
              className="wf-unsafe-item"
              data-kind={blocker.kind}
            >
              <div className="wf-unsafe-item-head">
                <span>
                  <strong>{blocker.step ?? blocker.workflow_name}</strong>
                  {blocker.phase ? ` (${blocker.phase})` : ''}
                  {blocker.on_failure ? ` — ${t('wf.readinessRollback')}` : ''}
                  {' — '}
                  <code>{blocker.reason ?? blocker.kind}</code>
                  {blocker.human_only ? ` — ${t('wf.readinessHumanOnly')}` : ''}
                </span>
              </div>
              <p className="wf-unsafe-manual-reason">{blocker.message}</p>
              <p className="wf-unsafe-manual">{blocker.action}</p>
            </li>
          ))}
        </ul>
      )}
      {elsewhere.size > 0 && (
        <div className="wf-unsafe-chain" data-testid="readiness-chain">
          <p className="wf-unsafe-intro">{t('wf.readinessChildren')}</p>
          <ul className="wf-unsafe-list">
            {[...elsewhere.entries()].map(([id, entry]) => (
              <li key={id} className="wf-unsafe-item">
                {t('wf.readinessChildItem', entry.name, entry.total, entry.human)}
              </li>
            ))}
          </ul>
        </div>
      )}
      <ul className="wf-unsafe-list">
        {issues.map(issue => {
          const key = issueKey(issue);
          return (
            <li key={key} className="wf-unsafe-item">
              <div className="wf-unsafe-item-head">
                <span>
                  <strong>{issue.step_name}</strong>
                  {phaseLabel(issue, t)}
                  {' — '}
                  <code>{issue.placeholder || issue.reason}</code>
                  {issue.reason === 'unmodelled_program'
                    ? ` — ${issue.agent_written ? t('wf.unsafeAgentWritten') : t('wf.unsafeUnmodelled', issue.command)}`
                    : ''}
                </span>
                <button
                  type="button"
                  className="wf-btn-secondary"
                  aria-expanded={open === key}
                  onClick={() => setOpen(open === key ? null : key)}
                >
                  {issue.suggested_args
                    ? t('wf.unsafeSuggest')
                    : issue.reason === 'unmodelled_program' ? t('wf.approvalReview') : t('wf.unsafeHow')}
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
                    {issue.reason === 'unmodelled_program' && onApprove && (
                      <>
                        <pre className="wf-unsafe-diff">{`${issue.command} ${JSON.stringify(issue.args)}`}</pre>
                        <button
                          type="button"
                          className="wf-btn-secondary wf-unsafe-apply"
                          disabled={applying !== null}
                          onClick={() => { void approve(issue); }}
                        >
                          {t('wf.unsafeApprove')}
                        </button>
                      </>
                    )}
                  </div>
                )
              )}
            </li>
          );
        })}
      </ul>
      {agentPrompt && (
        <div className="wf-unsafe-agent">
          <p className="wf-unsafe-agent-intro">{t('wf.unsafeAgentIntro')}</p>
          <pre className="wf-unsafe-diff wf-unsafe-prompt" aria-label={t('wf.unsafeAgentPromptLabel')}>{agentPrompt}</pre>
          <button type="button" className="wf-btn-secondary" onClick={() => { void copyPrompt(); }}>
            {copied ? t('wf.unsafeAgentCopied') : t('wf.unsafeAgentCopy')}
          </button>
        </div>
      )}
    </section>
  );
}

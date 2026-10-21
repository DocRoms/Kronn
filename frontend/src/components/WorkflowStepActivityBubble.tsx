import { Cpu, Loader2, Workflow } from 'lucide-react';
import type { ActiveWorkflowStep } from '../types/generated';
import { AGENT_LABELS, AGENT_MENTIONS, agentTextColor } from '../lib/constants';
import { useT } from '../lib/I18nContext';
import { useLiveRunById } from '../lib/agentRunProgress';
import { AgentRunProgressPanel } from './AgentRunProgressPanel';

/**
 * A workflow step working for this room (KT-1252): the step's identity, then
 * its live progress (phase, tool categories, counts) once the server reports
 * it, else a placeholder. A step still preparing — its agent has not entered
 * the room — says so rather than looking like a stall.
 */
export function WorkflowStepActivityBubble({
  step,
  discussionId,
}: {
  step: ActiveWorkflowStep;
  discussionId?: string;
}) {
  const { t } = useT();
  const run = useLiveRunById(discussionId, step.progress_run_id);
  const trigger = AGENT_MENTIONS.find(mention => mention.type === step.agent_type)?.trigger
    ?? AGENT_LABELS[step.agent_type]
    ?? step.agent_type;
  const agentLabel = AGENT_LABELS[step.agent_type] ?? step.agent_type;
  const shortRun = step.run_id.slice(0, 8);
  const preparing = step.preparing === true;

  return (
    <div
      className="disc-msg-row"
      data-role="agent"
      data-testid={`workflow-step-running-${step.run_id}-${step.step_key}`}
      data-preparing={preparing ? 'true' : undefined}
      aria-live="polite"
    >
      <div className="disc-msg-bubble" data-role="agent">
        <div
          className="disc-msg-agent-label"
          style={{ color: agentTextColor(step.agent_type), justifyContent: 'space-between' }}
        >
          <span className="flex-row gap-2">
            <Cpu size={10} /> {trigger}
            <span className="disc-msg-agent-kind"> · {t('disc.targetDiscussionAgent')}</span>
          </span>
          <Loader2 size={10} className="spin" aria-hidden="true" />
        </div>
        <div className="disc-workflow-step-identity" data-testid="workflow-step-identity">
          <Workflow size={12} aria-hidden="true" />
          <strong>{step.workflow_name}</strong>
          <span aria-hidden="true">›</span>
          <span>{step.step_name}</span>
          <span className="disc-workflow-step-run">{t('disc.workflowRun', shortRun)}</span>
        </div>
        {run ? (
          <AgentRunProgressPanel run={run} agentLabel={agentLabel} />
        ) : (
          <div className="disc-streaming-waiting" data-testid="workflow-step-waiting">
            <span className="disc-pulse-dot" />
            {preparing ? t('disc.workflowStepPreparing') : t('disc.workflowStepRunning')}
          </div>
        )}
      </div>
    </div>
  );
}

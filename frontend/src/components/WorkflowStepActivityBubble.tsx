import { Cpu, Loader2, Workflow } from 'lucide-react';
import type { ActiveWorkflowStep } from '../types/generated';
import { AGENT_LABELS, AGENT_MENTIONS, agentTextColor } from '../lib/constants';
import { useT } from '../lib/I18nContext';

export function WorkflowStepActivityBubble({ step }: { step: ActiveWorkflowStep }) {
  const { t } = useT();
  const trigger = AGENT_MENTIONS.find(mention => mention.type === step.agent_type)?.trigger
    ?? AGENT_LABELS[step.agent_type]
    ?? step.agent_type;
  const shortRun = step.run_id.slice(0, 8);

  return (
    <div
      className="disc-msg-row"
      data-role="agent"
      data-testid={`workflow-step-running-${step.run_id}-${step.step_key}`}
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
        <div className="disc-streaming-waiting">
          <span className="disc-pulse-dot" />
          {t('disc.workflowStepRunning')}
        </div>
      </div>
    </div>
  );
}

import { useEffect, useMemo, useRef, useState } from 'react';
import { Cpu, UserPlus, Users } from 'lucide-react';
import { discussions, planning } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import {
  assigneeFor,
  assigneeLabel,
  assigneeOptions,
  nextRoleOnToggle,
  type AssigneeOption,
} from '../lib/planningAssignees';
import type {
  AgentType,
  ParticipantView,
  PlanningAssignee,
  PlanningAssigneeRole,
  PlanningTaskDetail,
} from '../types/generated';
import type { ToastFn } from '../hooks/useToast';
import './TaskAssignees.css';

const ROLES: PlanningAssigneeRole[] = ['owner', 'reviewer'];

interface ChipsProps {
  assignees: PlanningAssignee[];
  /** Chips shown before collapsing the rest into `+N`. */
  max?: number;
}

/**
 * KT-154 — who is on a task, at a glance: one pastille per agent, role and
 * (for a running execution worker) tier included, no click needed.
 */
// `assignees` defaults to empty: a payload from an older backend has no field.
export function TaskAssigneeChips({ assignees = [], max = 3 }: ChipsProps) {
  const { t } = useT();
  if (assignees.length === 0) return null;
  const shown = assignees.slice(0, max);
  const hidden = assignees.length - shown.length;
  const describe = (assignee: PlanningAssignee) => {
    const parts = [assigneeLabel(assignee, t('planning.assignee.cli'))];
    if (assignee.role) parts.push(t(`planning.assignee.role.${assignee.role}`));
    if (assignee.source === 'execution') {
      parts.push(t('planning.assignee.runningWorker'));
      if (assignee.tier) parts.push(t(`disc.tier.${assignee.tier}`));
    }
    return parts.join(' · ');
  };
  return (
    <span className="task-assignees" data-testid="task-assignees">
      {shown.map(assignee => {
        const key = [
          assignee.source,
          assignee.kind,
          assignee.agent_type,
          assignee.cli_session_id ?? '',
          assignee.connection_id ?? '',
        ].join(':');
        return (
          <span
            key={key}
            className="task-assignee"
            data-source={assignee.source}
            data-role={assignee.role ?? 'none'}
            title={describe(assignee)}
            aria-label={describe(assignee)}
          >
            {assignee.source === 'execution' && <Cpu size={9} aria-hidden="true" />}
            <span className="task-assignee-name">
              {assigneeLabel(assignee, t('planning.assignee.cliShort'))}
            </span>
            {assignee.role === 'reviewer' && (
              <span className="task-assignee-role">{t('planning.assignee.role.reviewer')}</span>
            )}
            {assignee.source === 'execution' && assignee.tier && (
              <span className="task-assignee-role">{t(`disc.tier.${assignee.tier}`)}</span>
            )}
          </span>
        );
      })}
      {hidden > 0 && (
        <span className="task-assignee task-assignee-more" title={assignees.slice(max).map(describe).join('\n')}>
          +{hidden}
        </span>
      )}
    </span>
  );
}

interface MenuProps {
  taskId: string;
  discussionId: string;
  discussionAgent: AgentType | null | undefined;
  assignees: PlanningAssignee[];
  /** Called with the refreshed task after every assign / unassign. */
  onChanged: (task: PlanningTaskDetail) => void;
  toast: ToastFn;
  /** `inline` renders the picker open in a detail sheet; `menu` is the quick popover. */
  variant?: 'menu' | 'inline';
}

/**
 * KT-154 — assign / unassign from the plan row (quick menu) or from the task
 * sheet (inline). Candidates are the room's discussion agent and its joined CLI
 * sessions, each by exact identity; a role button toggles owner / reviewer and
 * pressing the active role again removes the assignment.
 */
export function TaskAssigneePicker({
  taskId,
  discussionId,
  discussionAgent,
  assignees = [],
  onChanged,
  toast,
  variant = 'menu',
}: MenuProps) {
  const { t } = useT();
  const [open, setOpen] = useState(variant === 'inline');
  const [participants, setParticipants] = useState<ParticipantView[]>([]);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const rootRef = useRef<HTMLSpanElement | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    discussions.participants(discussionId)
      .then(rows => {
        if (cancelled) return;
        // `participants` may also return plain sessions; only a provider-typed
        // row carries what an option needs.
        setParticipants(rows as ParticipantView[]);
      })
      .catch(() => {
        if (!cancelled) setParticipants([]);
      });
    return () => {
      cancelled = true;
    };
  }, [open, discussionId]);

  useEffect(() => {
    if (!open || variant !== 'menu') return;
    const close = (event: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', escape);
    };
  }, [open, variant]);

  const options = useMemo(
    () => assigneeOptions(discussionAgent, participants),
    [discussionAgent, participants],
  );

  const toggle = async (option: AssigneeOption, clicked: PlanningAssigneeRole) => {
    const current = assigneeFor(assignees, option);
    if (current?.source === 'execution') return;
    const identity = {
      kind: option.kind,
      agent_type: option.agent_type,
      cli_session_id: option.cli_session_id ?? null,
      connection_id: option.connection_id ?? null,
    };
    const role = nextRoleOnToggle(current?.role, clicked);
    setBusyKey(option.key);
    try {
      const task = current && role === null
        ? await planning.unassign(taskId, { assignee: { ...identity, role: null } })
        : await planning.assign(taskId, { assignee: { ...identity, role } });
      onChanged(task);
    } catch (cause) {
      toast(userError(cause), 'error');
    } finally {
      setBusyKey(null);
    }
  };

  const list = (
    <div
      className="task-assignee-list"
      role="group"
      aria-label={t('planning.assignee.manage')}
      data-variant={variant}
    >
      {options.length === 0 && (
        <span className="task-assignee-empty">{t('planning.assignee.noCandidates')}</span>
      )}
      {options.map(option => {
        const current = assigneeFor(assignees, option);
        const label = assigneeLabel(option, t('planning.assignee.cli'));
        return (
          <div className="task-assignee-row" key={option.key} data-assigned={Boolean(current)}>
            <span className="task-assignee-row-name">{label}</span>
            <span className="task-assignee-roles">
              {ROLES.map(role => (
                <button
                  type="button"
                  key={role}
                  className="task-assignee-role-btn"
                  aria-pressed={current?.role === role}
                  // The worker of a running execution is derived, not assigned:
                  // there is nothing to toggle until it is also assigned by hand.
                  disabled={busyKey === option.key || current?.source === 'execution'}
                  aria-label={t('planning.assignee.setRole', label, t(`planning.assignee.role.${role}`))}
                  onClick={() => void toggle(option, role)}
                >
                  {t(`planning.assignee.role.${role}`)}
                </button>
              ))}
            </span>
          </div>
        );
      })}
    </div>
  );

  if (variant === 'inline') {
    return <span className="task-assignee-picker" ref={rootRef}>{list}</span>;
  }
  return (
    <span className="task-assignee-picker" ref={rootRef}>
      <button
        type="button"
        className="task-assignee-trigger"
        aria-expanded={open}
        aria-haspopup="true"
        title={t('planning.assignee.manage')}
        aria-label={t('planning.assignee.manage')}
        onClick={() => setOpen(value => !value)}
      >
        {assignees.length > 0 ? <Users size={11} /> : <UserPlus size={11} />}
      </button>
      {open && <span className="task-assignee-popover">{list}</span>}
    </span>
  );
}

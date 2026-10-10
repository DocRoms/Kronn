/**
 * KT-1100 — per-workflow run retention: how long runs without changes,
 * successful runs and failed runs are kept. An empty field inherits the
 * global setting; 0 keeps those runs forever.
 */
import { useState } from 'react';
import { Archive, ChevronDown, ChevronRight, Info } from 'lucide-react';
import type { WorkflowRetention } from '../../types/generated';

interface RunRetentionCardProps {
  value: WorkflowRetention | null | undefined;
  onChange: (next: WorkflowRetention | null) => void;
  t: (key: string, ...args: (string | number)[]) => string;
}

export const DEFAULT_NO_OP_RETENTION_HOURS = 24;

const toText = (value: number | null | undefined) => (value == null ? '' : String(value));

/** Empty means "inherit"; anything else must be a whole number ≥ 0. */
function parseWindow(text: string): number | null | undefined {
  const trimmed = text.trim();
  if (!trimmed) return undefined;
  const n = Number(trimmed);
  return Number.isInteger(n) && n >= 0 ? n : null;
}

export function RunRetentionCard({ value, onChange, t }: RunRetentionCardProps) {
  const [noOp, setNoOp] = useState(() => toText(value?.no_op_hours));
  const [success, setSuccess] = useState(() => toText(value?.success_days));
  const [failure, setFailure] = useState(() => toText(value?.failure_days));
  const hasOverrides = Boolean(noOp || success || failure);
  const [expanded, setExpanded] = useState(hasOverrides);

  const sync = (next: { noOp?: string; success?: string; failure?: string }) => {
    const windows: [keyof WorkflowRetention, number | null | undefined][] = [
      ['no_op_hours', parseWindow(next.noOp ?? noOp)],
      ['success_days', parseWindow(next.success ?? success)],
      ['failure_days', parseWindow(next.failure ?? failure)],
    ];
    const retention: WorkflowRetention = {};
    for (const [key, value] of windows) {
      // An invalid field keeps the last valid value rather than sending junk.
      if (value === null) return;
      if (value !== undefined) retention[key] = value;
    }
    onChange(Object.keys(retention).length === 0 ? null : retention);
  };

  const field = (
    id: string,
    label: string,
    unit: string,
    text: string,
    placeholder: string,
    set: (v: string) => void,
    key: 'noOp' | 'success' | 'failure',
  ) => (
    <div className="wf-retention-row">
      <label htmlFor={id} className="wf-retention-label">
        {label}
        <span className="text-muted wf-retention-unit">{unit}</span>
      </label>
      <input
        id={id}
        type="number"
        min="0"
        step="1"
        className="wf-input wf-retention-input"
        value={text}
        placeholder={placeholder}
        onChange={e => { set(e.target.value); sync({ [key]: e.target.value }); }}
      />
    </div>
  );

  return (
    <div className="wf-section wf-retention-card">
      <button
        type="button"
        className="wf-retention-toggle"
        aria-expanded={expanded}
        onClick={() => setExpanded(e => !e)}
      >
        {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <Archive size={14} />
        <span>{t('wf.retention.title')}</span>
        <span className="text-muted wf-retention-summary">
          {hasOverrides ? t('wf.retention.summaryActive') : t('wf.retention.summaryDefaults', DEFAULT_NO_OP_RETENTION_HOURS)}
        </span>
      </button>
      {expanded && (
        <div className="wf-retention-body">
          <p className="text-muted wf-retention-help">
            <Info size={11} /> {t('wf.retention.description')}
          </p>
          {field('wf-retention-noop', t('wf.retention.noOpLabel'), t('wf.retention.hoursUnit'), noOp,
            String(DEFAULT_NO_OP_RETENTION_HOURS), setNoOp, 'noOp')}
          {field('wf-retention-success', t('wf.retention.successLabel'), t('wf.retention.daysUnit'), success,
            t('wf.retention.globalPlaceholder'), setSuccess, 'success')}
          {field('wf-retention-failure', t('wf.retention.failureLabel'), t('wf.retention.daysUnit'), failure,
            t('wf.retention.globalPlaceholder'), setFailure, 'failure')}
        </div>
      )}
    </div>
  );
}

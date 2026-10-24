/**
 * KT-1448 — named references of a workflow: a name for a Page, workflow,
 * Quick Prompt / API / Exec or skill, read by steps as `{{refs.<name>}}` and
 * by Exec steps as `KRONN_REF_<NAME>`. Exports carry the objects.
 */
import { useState } from 'react';
import { ChevronDown, ChevronRight, Info, Link2, Plus, Trash2 } from 'lucide-react';
import type { ResourceRef, ResourceRefKind } from '../../types/generated';
import { REF_KINDS, isSafeRefId, isValidRefName, type RefOptions } from '../../lib/workflowRefs';

type Refs = Partial<Record<string, ResourceRef>>;

interface WorkflowRefsCardProps {
  value: Refs;
  onChange: (next: Record<string, ResourceRef>) => void;
  options: RefOptions;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function WorkflowRefsCard({ value, onChange, options, t }: WorkflowRefsCardProps) {
  const entries = Object.entries(value).filter((entry): entry is [string, ResourceRef] => Boolean(entry[1]));
  const [expanded, setExpanded] = useState(entries.length > 0);
  const [name, setName] = useState('');
  const [kind, setKind] = useState<ResourceRefKind>('page');
  const [id, setId] = useState('');
  const trimmed = name.trim();
  const nameError = trimmed && !isValidRefName(trimmed)
    ? t('wiz.refs.invalidName')
    : trimmed && value[trimmed] ? t('wiz.refs.duplicate') : null;
  const canAdd = Boolean(trimmed) && !nameError && Boolean(id);
  const current = Object.fromEntries(entries);

  const add = () => {
    if (!canAdd) return;
    onChange({ ...current, [trimmed]: { kind, id } });
    setName(''); setId('');
  };
  const remove = (refName: string) => {
    const next = { ...current };
    delete next[refName];
    onChange(next);
  };
  const labelOf = (ref: ResourceRef) => options[ref.kind].find(option => option.id === ref.id)?.label;

  return (
    <div className="wf-section wf-retention-card" data-testid="workflow-refs">
      <button type="button" className="wf-retention-toggle" aria-expanded={expanded} onClick={() => setExpanded(e => !e)}>
        {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <Link2 size={14} />
        <span>{t('wiz.refs.title')}</span>
        <span className="text-muted wf-retention-summary">{t('wiz.refs.count', entries.length)}</span>
      </button>
      {expanded && (
        <div className="wf-retention-body">
          <p className="text-muted wf-retention-help"><Info size={11} /> {t('wiz.refs.hint')}</p>
          {entries.length > 0 && (
            <ul className="wf-refs-list">
              {entries.map(([refName, ref]) => (
                <li key={refName} className="flex-row gap-3">
                  <code>{`{{refs.${refName}}}`}</code>
                  <span className="text-muted">{t(`wiz.refs.kind.${ref.kind}`)}</span>
                  <span>{labelOf(ref) ?? ref.id}</span>
                  <code className="text-muted">{`KRONN_REF_${refName.toUpperCase()}`}</code>
                  <button type="button" className="wf-icon-btn" aria-label={t('wiz.refs.remove', refName)} onClick={() => remove(refName)}>
                    <Trash2 size={12} />
                  </button>
                </li>
              ))}
            </ul>
          )}
          <div className="flex-row gap-3 mt-4">
            <input
              className="wf-input"
              value={name}
              placeholder="board"
              aria-label={t('wiz.refs.name')}
              aria-invalid={Boolean(nameError)}
              onChange={e => setName(e.target.value)}
            />
            <select className="wf-select" aria-label={t('wiz.refs.kind')} value={kind}
              onChange={e => { setKind(e.target.value as ResourceRefKind); setId(''); }}>
              {REF_KINDS.map(item => <option key={item} value={item}>{t(`wiz.refs.kind.${item}`)}</option>)}
            </select>
            <select className="wf-select" aria-label={t('wiz.refs.object')} value={id} onChange={e => setId(e.target.value)}>
              <option value="">{t('wiz.refs.pick')}</option>
              {options[kind].filter(option => isSafeRefId(option.id)).map(option => (
                <option key={option.id} value={option.id}>{option.label}</option>
              ))}
            </select>
            <button type="button" className="wf-small-btn" disabled={!canAdd} onClick={add}>
              <Plus size={10} /> {t('wiz.refs.add')}
            </button>
          </div>
          {nameError && <p className="text-sm text-error" role="alert">{nameError}</p>}
        </div>
      )}
    </div>
  );
}

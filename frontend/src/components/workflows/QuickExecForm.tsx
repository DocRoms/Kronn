import { useState } from 'react';
import { Plus, Save, ShieldCheck, Trash2, X } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import type {
  CollectQuickExecOutputFormat,
  CreateQuickExecRequest,
  Project,
  PromptVariable,
  QuickExec,
} from '../../types/generated';

interface QuickExecFormProps {
  editExec?: QuickExec;
  projects: Project[];
  onSave: (request: CreateQuickExecRequest) => Promise<void> | void;
  onCancel: () => void;
}

const blankVariable = (): PromptVariable => ({
  name: '',
  label: '',
  placeholder: '',
  description: null,
  required: true,
  pattern: null,
  source: 'user_input',
  source_ref: null,
  allow_manual_override: false,
});

export function QuickExecForm({ editExec, projects, onSave, onCancel }: QuickExecFormProps) {
  const { t } = useT();
  const [name, setName] = useState(editExec?.name ?? '');
  const [icon, setIcon] = useState(editExec?.icon ?? '⌘');
  const [description, setDescription] = useState(editExec?.description ?? '');
  const [projectId, setProjectId] = useState(editExec?.project_id ?? '');
  const [command, setCommand] = useState(editExec?.command ?? '');
  const [args, setArgs] = useState(editExec?.args.join('\n') ?? '');
  const [timeout, setTimeoutValue] = useState(editExec?.timeout_secs ?? 60);
  const [outputFormat, setOutputFormat] = useState<CollectQuickExecOutputFormat>(
    editExec?.output_format ?? 'json',
  );
  const [variables, setVariables] = useState<PromptVariable[]>(editExec?.variables ?? []);
  const [saving, setSaving] = useState(false);

  const save = async () => {
    if (!name.trim() || !command.trim() || saving) return;
    setSaving(true);
    try {
      await onSave({
        name: name.trim(),
        icon: icon.trim() || '⌘',
        description: description.trim(),
        project_id: projectId || null,
        command: command.trim(),
        args: args.split('\n').filter(argument => argument.length > 0),
        timeout_secs: Math.max(1, Math.min(1800, timeout)),
        output_format: outputFormat,
        variables: variables
          .filter(variable => variable.name.trim())
          .map(variable => ({
            ...variable,
            name: variable.name.trim(),
            label: variable.label.trim() || variable.name.trim(),
          })),
      });
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="qp-form qe-form" aria-label={t('qe.formTitle')}>
      <div className="qp-form-header">
        <div>
          <h2>{editExec ? t('qe.editTitle') : t('qe.newTitle')}</h2>
          <p>{t('qe.formHint')}</p>
        </div>
        <button className="wf-icon-btn" onClick={onCancel} aria-label={t('common.close')}>
          <X size={15} />
        </button>
      </div>

      <div className="qe-security-note">
        <ShieldCheck size={16} />
        <span>{t('qe.securityHint')}</span>
      </div>

      <div className="qe-form-grid">
        <label>
          <span>{t('qe.icon')}</span>
          <input className="wf-input" value={icon} onChange={event => setIcon(event.target.value)} maxLength={8} />
        </label>
        <label className="qe-form-wide">
          <span>{t('qe.name')} *</span>
          <input className="wf-input" value={name} onChange={event => setName(event.target.value)} autoFocus />
        </label>
        <label className="qe-form-wide">
          <span>{t('qe.description')}</span>
          <input className="wf-input" value={description} onChange={event => setDescription(event.target.value)} />
        </label>
        <label>
          <span>{t('qe.project')}</span>
          <select className="wf-select" value={projectId} onChange={event => setProjectId(event.target.value)}>
            <option value="">{t('qe.noProject')}</option>
            {projects.map(project => <option key={project.id} value={project.id}>{project.name}</option>)}
          </select>
        </label>
        <label>
          <span>{t('qe.command')} *</span>
          <input
            className="wf-input font-mono"
            value={command}
            onChange={event => setCommand(event.target.value.replace(/[^A-Za-z0-9._-]/g, ''))}
            placeholder="aws"
          />
        </label>
        <label className="qe-form-wide">
          <span>{t('qe.args')}</span>
          <textarea
            className="wf-textarea qe-args"
            value={args}
            onChange={event => setArgs(event.target.value)}
            placeholder={'cloudwatch\nget-metric-data\n--output\njson'}
          />
          <small>{t('qe.argsHint')}</small>
        </label>
        <label>
          <span>{t('qe.outputFormat')}</span>
          <select
            className="wf-select"
            value={outputFormat}
            onChange={event => setOutputFormat(event.target.value as CollectQuickExecOutputFormat)}
          >
            <option value="json">JSON</option>
            <option value="csv">CSV → JSON</option>
            <option value="text">{t('qe.text')}</option>
            <option value="lines">{t('qe.lines')}</option>
          </select>
        </label>
        <label>
          <span>{t('qe.timeout')}</span>
          <input
            className="wf-input"
            type="number"
            min={1}
            max={1800}
            value={timeout}
            onChange={event => setTimeoutValue(Number(event.target.value) || 1)}
          />
        </label>
      </div>

      <div className="qe-variables">
        <div className="flex-between">
          <div>
            <h3>{t('qe.variables')}</h3>
            <p>{t('qe.variablesHint')}</p>
          </div>
          <button className="wf-small-btn" type="button" onClick={() => setVariables(current => [...current, blankVariable()])}>
            <Plus size={12} /> {t('qe.addVariable')}
          </button>
        </div>
        {variables.map((variable, index) => (
          <div className="qe-variable-row" key={index}>
            <input
              className="wf-input font-mono"
              aria-label={t('qe.variableName')}
              value={variable.name}
              onChange={event => setVariables(current => current.map((item, itemIndex) => itemIndex === index
                ? { ...item, name: event.target.value.replace(/[^A-Za-z0-9_.-]/g, '') }
                : item))}
              placeholder="region"
            />
            <input
              className="wf-input"
              aria-label={t('qe.variableLabel')}
              value={variable.label}
              onChange={event => setVariables(current => current.map((item, itemIndex) => itemIndex === index
                ? { ...item, label: event.target.value }
                : item))}
              placeholder={t('qe.variableLabel')}
            />
            <label className="qe-variable-required">
              <input
                type="checkbox"
                checked={variable.required}
                onChange={event => setVariables(current => current.map((item, itemIndex) => itemIndex === index
                  ? { ...item, required: event.target.checked }
                  : item))}
              />
              {t('qe.required')}
            </label>
            <button
              className="wf-icon-btn"
              type="button"
              aria-label={t('common.delete')}
              onClick={() => setVariables(current => current.filter((_, itemIndex) => itemIndex !== index))}
            >
              <Trash2 size={13} />
            </button>
          </div>
        ))}
      </div>

      <div className="qp-form-actions">
        <button className="wf-small-btn" type="button" onClick={onCancel}>{t('common.cancel')}</button>
        <button className="wf-create-btn" type="button" disabled={!name.trim() || !command.trim() || saving} onClick={() => void save()}>
          <Save size={13} /> {saving ? t('qe.saving') : t('qe.save')}
        </button>
      </div>
    </section>
  );
}

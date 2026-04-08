import { useState, useRef, useEffect } from 'react';
import { useT } from '../../lib/I18nContext';
import { AGENT_LABELS } from '../../lib/constants';
import type { QuickPrompt, PromptVariable, CreateQuickPromptRequest, Project, AgentType } from '../../types/generated';
import { Plus, Save, X } from 'lucide-react';

const ALL_AGENTS: AgentType[] = ['ClaudeCode', 'Codex', 'GeminiCli', 'Kiro', 'Vibe', 'CopilotCli'];

/** Extract {{variable}} names from a template string (includes {{var}} and {{#var}}) */
function extractVars(template: string): string[] {
  const matches = template.match(/\{\{#?(\w+)\}\}/g) ?? [];
  const names = matches.map(m => m.replace(/\{\{#?|\}\}/g, ''));
  // Deduplicate, exclude closing tags {{/var}}
  return [...new Set(names)];
}

interface Props {
  editPrompt?: QuickPrompt;
  projects: Project[];
  onSave: (req: CreateQuickPromptRequest) => Promise<void>;
  onCancel: () => void;
}

export function QuickPromptForm({ editPrompt, projects, onSave, onCancel }: Props) {
  const { t } = useT();
  const [name, setName] = useState(editPrompt?.name ?? '');
  const [icon, setIcon] = useState(editPrompt?.icon ?? '');
  const [template, setTemplate] = useState(editPrompt?.prompt_template ?? '');
  const [variables, setVariables] = useState<PromptVariable[]>(editPrompt?.variables ?? []);
  const [agent, setAgent] = useState<AgentType>(editPrompt?.agent ?? 'ClaudeCode');
  const [projectId, setProjectId] = useState(editPrompt?.project_id ?? '');
  const [saving, setSaving] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Auto-sync variables from template
  useEffect(() => {
    const detected = extractVars(template);
    setVariables(prev => {
      const existing = new Map(prev.map(v => [v.name, v]));
      return detected.map(name => existing.get(name) ?? { name, label: name, placeholder: '' });
    });
  }, [template]);

  const insertVariable = () => {
    const el = textareaRef.current;
    if (!el) return;
    const varName = `var${variables.length + 1}`;
    const insert = `{{${varName}}}`;
    const pos = el.selectionStart;
    const before = template.slice(0, pos);
    const after = template.slice(pos);
    setTemplate(before + insert + after);
    // Restore cursor after React re-render
    requestAnimationFrame(() => {
      el.focus();
      const newPos = pos + insert.length;
      el.setSelectionRange(newPos, newPos);
    });
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      await onSave({
        name,
        icon: icon || null,
        prompt_template: template,
        variables,
        agent,
        project_id: projectId || null,
        skill_ids: editPrompt?.skill_ids ?? [],
        tier: editPrompt?.tier ?? 'default',
      });
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="qp-form">
      <div className="flex-between mb-4">
        <h3 className="font-semibold">{editPrompt ? name || t('qp.name') : t('qp.new')}</h3>
        <button className="wf-icon-btn" onClick={onCancel}><X size={14} /></button>
      </div>

      <div className="flex-row gap-4 mb-4">
        <input
          className="wf-input"
          style={{ width: 50, textAlign: 'center', fontSize: 20 }}
          value={icon}
          onChange={e => setIcon(e.target.value)}
          placeholder="⚡"
          maxLength={2}
        />
        <input
          className="wf-input flex-1"
          value={name}
          onChange={e => setName(e.target.value)}
          placeholder={t('qp.namePlaceholder')}
        />
      </div>

      <div className="flex-row gap-4 mb-4">
        <select className="wf-select" value={agent} onChange={e => setAgent(e.target.value as AgentType)}>
          {ALL_AGENTS.map(a => <option key={a} value={a}>{AGENT_LABELS[a] ?? a}</option>)}
        </select>
        <select className="wf-select" value={projectId} onChange={e => setProjectId(e.target.value)}>
          <option value="">{t('wiz.noProject')}</option>
          {projects.map(p => <option key={p.id} value={p.id}>{p.name}</option>)}
        </select>
      </div>

      {/* Variables */}
      {variables.length > 0 && (
        <div className="qp-vars mb-4">
          {variables.map((v, i) => (
            <div key={v.name} className="qp-var-row">
              <code className="qp-var-name">{`{{${v.name}}}`}</code>
              <input
                className="wf-input flex-1"
                value={v.label}
                onChange={e => setVariables(prev => prev.map((pv, j) => j === i ? { ...pv, label: e.target.value } : pv))}
                placeholder={t('qp.varLabel')}
              />
              <input
                className="wf-input flex-1"
                value={v.placeholder}
                onChange={e => setVariables(prev => prev.map((pv, j) => j === i ? { ...pv, placeholder: e.target.value } : pv))}
                placeholder={t('qp.varPlaceholder')}
              />
            </div>
          ))}
        </div>
      )}

      <div className="flex-between mb-2">
        <label className="wf-label mb-0">{t('qp.prompt')}</label>
        <button className="qp-add-var-btn" onClick={insertVariable}>
          <Plus size={12} /> {t('qp.addVariable')}
        </button>
      </div>
      <textarea
        ref={textareaRef}
        className="wf-textarea mb-4"
        rows={8}
        value={template}
        onChange={e => setTemplate(e.target.value)}
        placeholder={t('qp.promptPlaceholder')}
      />
      <p className="qp-syntax-hint">{t('qp.syntaxHint')}</p>

      <div className="flex-row gap-4">
        <button className="wf-create-btn" onClick={handleSave} disabled={saving || !name || !template}>
          <Save size={14} /> {saving ? '...' : t('qp.save')}
        </button>
      </div>
    </div>
  );
}

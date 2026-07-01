import { useState, useRef, useEffect } from 'react';
import { useT } from '../../lib/I18nContext';
import { AGENT_LABELS } from '../../lib/constants';
import { config as configApi, ollama as ollamaApi } from '../../lib/api';
import type {
  QuickPrompt,
  PromptVariable,
  CreateQuickPromptRequest,
  Project,
  AgentType,
  Skill,
  AgentProfile,
  Directive,
  ModelTier,
} from '../../types/generated';
import { Plus, Save, X, Check, Zap, UserCircle, FileText, ChevronRight } from 'lucide-react';

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
  /** 0.8.5 — full skills catalog, used by the multi-select picker. Empty array hides the section. */
  skills?: Skill[];
  /** 0.8.5 — full profiles catalog. Empty array hides the section. */
  profiles?: AgentProfile[];
  /** 0.8.5 — full directives catalog. Empty array hides the section. */
  directives?: Directive[];
  onSave: (req: CreateQuickPromptRequest) => Promise<void>;
  onCancel: () => void;
}

export function QuickPromptForm({
  editPrompt,
  projects,
  skills = [],
  profiles = [],
  directives = [],
  onSave,
  onCancel,
}: Props) {
  const { t } = useT();
  const [name, setName] = useState(editPrompt?.name ?? '');
  const [icon, setIcon] = useState(editPrompt?.icon ?? '');
  const [description, setDescription] = useState(editPrompt?.description ?? '');
  const [template, setTemplate] = useState(editPrompt?.prompt_template ?? '');
  const [variables, setVariables] = useState<PromptVariable[]>(editPrompt?.variables ?? []);
  const [agent, setAgent] = useState<AgentType>(editPrompt?.agent ?? 'ClaudeCode');
  const [projectId, setProjectId] = useState(editPrompt?.project_id ?? '');
  // 0.8.6 phase 4 — tier carried through across edits. New QPs start
  // with 'default' then the effect below replaces it with the user's
  // `ServerConfig.default_model_tier` on mount (strict semantic — only
  // applied to new QPs, never overwrites an editPrompt's saved tier).
  const [tier, setTier] = useState<ModelTier>(editPrompt?.tier ?? 'default');
  // 0.8.10 — optional explicit model (wins over tier at run time). Free text
  // (any tag / remote host) with pulled Ollama models offered as suggestions.
  const [model, setModel] = useState<string>(editPrompt?.agent_settings?.model ?? '');
  const [ollamaModels, setOllamaModels] = useState<string[]>([]);
  useEffect(() => {
    ollamaApi.models()
      .then(r => setOllamaModels((r.models ?? []).map(m => m.name)))
      .catch(() => {});
  }, []);
  // 0.8.5 — three binding axes mirroring the Discussion form.
  const [skillIds, setSkillIds] = useState<string[]>(editPrompt?.skill_ids ?? []);
  const [profileIds, setProfileIds] = useState<string[]>(editPrompt?.profile_ids ?? []);
  const [directiveIds, setDirectiveIds] = useState<string[]>(editPrompt?.directive_ids ?? []);
  // Accordion: only one section open at a time, none by default.
  const [expandedBinding, setExpandedBinding] = useState<'skills' | 'profiles' | 'directives' | null>(null);
  const [saving, setSaving] = useState(false);
  // Race-free guard: `disabled={saving}` is closure-stale between two
  // synchronous clicks, so a fast double-click on Save creates the
  // QuickPrompt twice (`quickPromptsApi.create` is not idempotent).
  const savingRef = useRef(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // 0.8.6 phase 4 — for NEW QPs (no editPrompt), pre-fill `tier` from
  // the user's saved default. For edits we keep the existing tier so a
  // settings change doesn't silently bump every legacy QP into Reasoning.
  useEffect(() => {
    if (editPrompt) return;
    configApi.getServerConfig()
      .then(cfg => {
        if (cfg?.default_model_tier) setTier(cfg.default_model_tier);
      })
      .catch(() => { /* keep 'default' fallback */ });
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Auto-sync variables from template. We preserve any description /
  // required flag / label the user already set — only the `name` field
  // is authoritative from the template.
  useEffect(() => {
    const detected = extractVars(template);
    setVariables(prev => {
      const existing = new Map(prev.map(v => [v.name, v]));
      return detected.map(n => existing.get(n) ?? {
        name: n, label: n, placeholder: '', description: null, required: true,
      });
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
    if (savingRef.current) return;
    savingRef.current = true;
    setSaving(true);
    try {
      await onSave({
        name,
        icon: icon || null,
        prompt_template: template,
        variables,
        agent,
        project_id: projectId || null,
        skill_ids: skillIds,
        profile_ids: profileIds,
        directive_ids: directiveIds,
        tier,
        agent_settings: model.trim()
          ? { model: model.trim(), tier: null, reasoning_effort: null, max_tokens: null }
          : null,
        description,
      });
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  };

  const bindingCount = skillIds.length + profileIds.length + directiveIds.length;

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

      {/* 0.8.10 — optional explicit model, wins over the tier at run time.
          Free text (any tag / remote host); pulled Ollama models offered as
          suggestions when the agent is Ollama. Empty = resolve from tier. */}
      <label className="wf-label">{t('wiz.model')}</label>
      <input
        className="wf-input mb-4"
        value={model}
        onChange={e => setModel(e.target.value)}
        placeholder={agent === 'Ollama' ? 'ex: qwen3:8b — vide = selon le tier' : 'vide = selon le tier'}
        list={agent === 'Ollama' ? 'qp-ollama-models' : undefined}
      />
      {agent === 'Ollama' && ollamaModels.length > 0 && (
        <datalist id="qp-ollama-models">
          {ollamaModels.map(m => <option key={m} value={m} />)}
        </datalist>
      )}

      {/* Prompt description — documents what this QP does. */}
      <label className="wf-label">{t('qp.descriptionLabel')}</label>
      <textarea
        className="wf-textarea mb-4"
        rows={2}
        value={description}
        onChange={e => setDescription(e.target.value)}
        placeholder={t('qp.descriptionPlaceholder')}
      />

      {/* 0.8.5 — Bindings accordion: skills + profiles + directives.
          Renders the same chip-pickers the Discussion form uses so the
          user has a single mental model. Hidden entirely when no
          catalogs are provided (e.g. embed contexts that don't load
          them). */}
      {(skills.length + profiles.length + directives.length > 0) && (
        <div className="qp-bindings mb-4" data-testid="qp-bindings">
          <div className="disc-advanced-section-label" style={{ marginBottom: 6 }}>
            {t('qp.bindingsLabel')}
            {bindingCount > 0 && (
              <span className="disc-advanced-count" style={{ marginLeft: 6 }}>
                {bindingCount}
              </span>
            )}
          </div>

          {skills.length > 0 && (
            <div className="disc-advanced-section">
              <button
                type="button"
                className="disc-advanced-section-toggle"
                onClick={() => setExpandedBinding(prev => prev === 'skills' ? null : 'skills')}
                data-testid="qp-bindings-skills-toggle"
              >
                <ChevronRight size={9} className="disc-chevron" data-expanded={expandedBinding === 'skills'} />
                <Zap size={10} />
                <span>{t('skills.selectSkills')}</span>
                {skillIds.length > 0 && <span className="disc-advanced-count">{skillIds.length}</span>}
              </button>
              {expandedBinding === 'skills' && (
                <div className="disc-advanced-chips" data-testid="qp-bindings-skills-chips">
                  {skills.map(skill => {
                    const selected = skillIds.includes(skill.id);
                    return (
                      <button
                        key={skill.id}
                        type="button"
                        className="disc-chip"
                        data-active={selected}
                        data-color="accent"
                        onClick={() => setSkillIds(prev => selected ? prev.filter(id => id !== skill.id) : [...prev, skill.id])}
                        title={skill.description || skill.name}
                      >
                        {selected && <Check size={9} />} {skill.name}
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          )}

          {profiles.length > 0 && (
            <div className="disc-advanced-section">
              <button
                type="button"
                className="disc-advanced-section-toggle"
                onClick={() => setExpandedBinding(prev => prev === 'profiles' ? null : 'profiles')}
                data-testid="qp-bindings-profiles-toggle"
              >
                <ChevronRight size={9} className="disc-chevron" data-expanded={expandedBinding === 'profiles'} />
                <UserCircle size={10} />
                <span>{t('profiles.select')}</span>
                {profileIds.length > 0 && <span className="disc-advanced-count">{profileIds.length}</span>}
              </button>
              {expandedBinding === 'profiles' && (
                <div className="disc-advanced-chips" data-testid="qp-bindings-profiles-chips">
                  <button
                    type="button"
                    className="disc-chip"
                    data-active={profileIds.length === 0}
                    data-color="purple"
                    onClick={() => setProfileIds([])}
                  >
                    {t('profiles.none')}
                  </button>
                  {profiles.map(profile => {
                    const selected = profileIds.includes(profile.id);
                    return (
                      <button
                        key={profile.id}
                        type="button"
                        className="disc-chip"
                        data-active={selected}
                        data-color="purple"
                        onClick={() => setProfileIds(prev => selected ? prev.filter(id => id !== profile.id) : [...prev, profile.id])}
                        title={profile.role || profile.persona_name || profile.name}
                        style={selected && profile.color ? { borderColor: profile.color, background: `${profile.color}15`, color: profile.color } : undefined}
                      >
                        {selected && <Check size={9} />} {profile.avatar} {profile.persona_name || profile.name}
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          )}

          {directives.length > 0 && (
            <div className="disc-advanced-section">
              <button
                type="button"
                className="disc-advanced-section-toggle"
                onClick={() => setExpandedBinding(prev => prev === 'directives' ? null : 'directives')}
                data-testid="qp-bindings-directives-toggle"
              >
                <ChevronRight size={9} className="disc-chevron" data-expanded={expandedBinding === 'directives'} />
                <FileText size={10} />
                <span>{t('directives.title')}</span>
                {directiveIds.length > 0 && <span className="disc-advanced-count">{directiveIds.length}</span>}
              </button>
              {expandedBinding === 'directives' && (
                <div className="disc-advanced-chips" data-testid="qp-bindings-directives-chips">
                  {directives.map(directive => {
                    const selected = directiveIds.includes(directive.id);
                    return (
                      <button
                        key={directive.id}
                        type="button"
                        className="disc-chip"
                        data-active={selected}
                        data-color="warning"
                        onClick={() => setDirectiveIds(prev => selected ? prev.filter(id => id !== directive.id) : [...prev, directive.id])}
                        title={directive.description || directive.name}
                      >
                        {selected && <Check size={9} />} {directive.icon} {directive.name}
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          )}
        </div>
      )}

      {/* Variables — each can now have an optional description + a
          required flag. Required is ON by default (legacy behaviour). */}
      {variables.length > 0 && (
        <div className="qp-vars mb-4">
          <label className="wf-label">{t('qp.variables')}</label>
          {variables.map((v, i) => (
            <div key={v.name} className="qp-var-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
              <div className="flex-row gap-4" style={{ alignItems: 'center' }}>
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
                <label className="flex-row gap-2" style={{ fontSize: 12, whiteSpace: 'nowrap', cursor: 'pointer' }}>
                  <input
                    type="checkbox"
                    checked={v.required ?? true}
                    onChange={e => setVariables(prev => prev.map((pv, j) => j === i ? { ...pv, required: e.target.checked } : pv))}
                  />
                  {t('qp.varRequired')}
                </label>
              </div>
              <input
                className="wf-input"
                value={v.description ?? ''}
                onChange={e => setVariables(prev => prev.map((pv, j) => j === i ? { ...pv, description: e.target.value || null } : pv))}
                placeholder={t('qp.varDescriptionPlaceholder')}
                style={{ fontSize: 12, opacity: 0.85 }}
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

      {(!name || !template) && (
        <p className="text-xs text-ghost mb-2">
          {t('qp.saveBlockedHint')} :{' '}
          {[!name && t('qp.fieldName'), !template && t('qp.fieldPrompt')].filter(Boolean).join(', ')}
        </p>
      )}
      <div className="flex-row gap-4">
        <button
          className="wf-create-btn"
          onClick={handleSave}
          disabled={saving || !name || !template}
          title={(!name || !template) ? t('qp.saveBlockedHint') : undefined}
        >
          <Save size={14} /> {saving ? '...' : t('qp.save')}
        </button>
      </div>
    </div>
  );
}

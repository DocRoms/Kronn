import { useState, useRef, useEffect } from 'react';
import { useT } from '../../lib/I18nContext';
import { workflows as workflowsApi, skills as skillsApi, profiles as profilesApi, directives as directivesApi, quickPrompts as quickPromptsApi } from '../../lib/api';
import { AGENT_COLORS, AGENT_LABELS, ALL_AGENT_TYPES, isAgentRestricted } from '../../lib/constants';
import type {
  Project, Workflow, WorkflowTrigger,
  WorkflowStep, AgentType, WorkflowSafety,
  WorkspaceConfig, StepConditionRule,
  CreateWorkflowRequest, Skill, AgentProfile, Directive,
  WorkflowSuggestion, QuickPrompt,
} from '../../types/generated';
import type { AgentsConfig } from '../../types/generated';
import {
  Plus, Loader2, Check, X, ChevronRight, ChevronDown,
  Clock, GitBranch, Zap, HelpCircle, Settings, Shield,
  AlertTriangle, UserCircle, FileText, Sparkles, Layers, Send,
} from 'lucide-react';
import '../../pages/WorkflowsPage.css';

const checkAgentRestricted = isAgentRestricted;

/** Inline help indicator — a small (?) icon with a native tooltip on hover.
 * Used to explain jargon next to labels without cluttering the form layout. */
function HelpTip({ hint }: { hint: string }) {
  return (
    <span className="wf-help-icon" title={hint} aria-label={hint} role="img">
      <HelpCircle size={11} />
    </span>
  );
}

/** Parse a cron expression back into visual builder values */
function parseCronExpr(expr: string): { every: number; unit: 'minutes' | 'hours' | 'days' | 'weeks' | 'months'; at: string; weekdays: number[]; raw?: string } {
  const parts = expr.split(' ');
  if (parts.length !== 5) return { every: 5, unit: 'minutes', at: '00:00', weekdays: [] };
  const [min, hour, dom, _mon, dow] = parts;

  // Parse a comma-separated list of integers (e.g. "1,3,5") into a sorted array.
  // Handles plain numbers only — ranges like "1-5" fall through to raw preservation.
  const parseIntList = (s: string): number[] | null => {
    if (s === '*') return [];
    if (!/^[0-9,]+$/.test(s)) return null;
    const nums = s.split(',').map(n => parseInt(n, 10)).filter(n => !isNaN(n) && n >= 0 && n <= 6);
    return nums.length > 0 ? Array.from(new Set(nums)).sort((a, b) => a - b) : null;
  };

  if (min.startsWith('*/')) return { every: parseInt(min.slice(2)) || 5, unit: 'minutes', at: '00:00', weekdays: [] };
  if (hour.startsWith('*/')) return { every: parseInt(hour.slice(2)) || 1, unit: 'hours', at: `00:${min.padStart(2, '0')}`, weekdays: [] };

  // Days-of-week variant: "m h * * 1,3,5" → specific weekdays, every=1
  if (dom === '*' && dow !== '*') {
    const parsed = parseIntList(dow);
    if (parsed !== null && parsed.length > 0 && parsed.length < 7) {
      return { every: 1, unit: 'days', at: `${hour.padStart(2, '0')}:${min.padStart(2, '0')}`, weekdays: parsed };
    }
  }

  if (dom.startsWith('*/')) return { every: parseInt(dom.slice(2)) || 1, unit: 'days', at: `${hour.padStart(2, '0')}:${min.padStart(2, '0')}`, weekdays: [] };

  // Complex expression (e.g. "0 7,10,13,16,19 * * 1-5") — preserve raw
  return { every: 1, unit: 'days', at: `${hour.padStart(2, '0')}:${min.padStart(2, '0')}`, weekdays: [], raw: expr };
}

export interface WorkflowWizardProps {
  projects: Project[];
  editWorkflow?: Workflow;
  onDone: () => void;
  onCancel: () => void;
  installedAgentTypes?: AgentType[];
  agentAccess?: AgentsConfig;
}

export function WorkflowWizard({ projects, editWorkflow, onDone, onCancel, installedAgentTypes, agentAccess }: WorkflowWizardProps) {
  const { t } = useT();
  const availableAgents = (installedAgentTypes && installedAgentTypes.length > 0
    ? installedAgentTypes
    : ALL_AGENT_TYPES
  ).map(at => ({ type: at, label: AGENT_LABELS[at] ?? at }));
  const isEdit = !!editWorkflow;
  // Detect if an existing workflow needs advanced mode (multi-step, cron, hooks, etc.)
  const needsAdvanced = isEdit && (
    (editWorkflow.steps?.length ?? 0) > 1 ||
    editWorkflow.trigger?.type !== 'Manual' ||
    editWorkflow.workspace_config ||
    editWorkflow.safety?.sandbox ||
    editWorkflow.safety?.require_approval
  );
  const [wizardMode, setWizardMode] = useState<'simple' | 'advanced'>(needsAdvanced ? 'advanced' : 'simple');
  const isSimple = wizardMode === 'simple';
  const initTrigger = editWorkflow?.trigger;
  const initCron = initTrigger?.type === 'Cron' ? parseCronExpr(initTrigger.schedule) : null;
  const initTracker = initTrigger?.type === 'Tracker' ? initTrigger : null;

  const [wizardStep, setWizardStep] = useState(0);
  const [name, setName] = useState(editWorkflow?.name ?? '');
  const [projectId, setProjectId] = useState<string>(editWorkflow?.project_id ?? '');
  const [triggerType, setTriggerType] = useState<'Cron' | 'Tracker' | 'Manual'>(initTrigger?.type ?? 'Manual');
  const [cronEvery, setCronEvery] = useState(initCron?.every ?? 5);
  const [cronUnit, setCronUnit] = useState<'minutes' | 'hours' | 'days' | 'weeks' | 'months'>(initCron?.unit ?? 'minutes');
  const [cronAt, setCronAt] = useState(initCron?.at ?? '00:00');
  // Cron day-of-week: empty array = "every day", non-empty = specific days picked
  // (values are cron DoW: 0=Sun, 1=Mon, ..., 6=Sat).
  const [cronWeekdays, setCronWeekdays] = useState<number[]>(initCron?.weekdays ?? []);
  const [cronRaw, setCronRaw] = useState(initCron?.raw ?? '');
  const cronIsRaw = !!cronRaw;
  const hasSpecificDays = cronWeekdays.length > 0 && cronWeekdays.length < 7;
  const toggleWeekday = (d: number) => {
    setCronWeekdays(prev => prev.includes(d) ? prev.filter(x => x !== d) : [...prev, d]);
  };
  const [trackerOwner, setTrackerOwner] = useState(initTracker?.source?.owner ?? '');
  const [trackerRepo, setTrackerRepo] = useState(initTracker?.source?.repo ?? '');
  const [trackerLabels, setTrackerLabels] = useState(initTracker?.labels?.join(', ') ?? '');
  const [trackerInterval, setTrackerInterval] = useState(initTracker?.interval ?? '*/5 * * * *');
  const [showVarHelp, setShowVarHelp] = useState(false);
  const [expandedStepAdvanced, setExpandedStepAdvanced] = useState<number | null>(null);

  // Safety state
  const [safety, setSafety] = useState<WorkflowSafety>(editWorkflow?.safety ?? {
    sandbox: false, max_files: null, max_lines: null, require_approval: false,
  });

  // Config page: show/hide expert options (hooks, concurrency)
  const [showExpertConfig, setShowExpertConfig] = useState(false);

  // Workspace config state
  const initHooks = editWorkflow?.workspace_config?.hooks;
  const [wsHookAfterCreate, setWsHookAfterCreate] = useState(initHooks?.after_create ?? '');
  const [wsHookBeforeRun, setWsHookBeforeRun] = useState(initHooks?.before_run ?? '');
  const [wsHookAfterRun, setWsHookAfterRun] = useState(initHooks?.after_run ?? '');
  const [wsHookBeforeRemove, setWsHookBeforeRemove] = useState(initHooks?.before_remove ?? '');

  // Concurrency
  const [concurrencyLimit, setConcurrencyLimit] = useState<string>(editWorkflow?.concurrency_limit?.toString() ?? '');

  // Build cron expression from visual inputs (or raw if complex)
  const buildCronExpr = (): string => {
    if (cronRaw) return cronRaw;
    const [hh, mm] = cronAt.split(':').map(Number);
    const h = isNaN(hh) ? 0 : hh;
    const m = isNaN(mm) ? 0 : mm;
    switch (cronUnit) {
      case 'minutes': return `*/${cronEvery} * * * *`;
      case 'hours':   return `${m} */${cronEvery} * * *`;
      case 'days':
        // If specific weekdays are picked (1-6 of them), emit a DoW-specific
        // cron pattern. 0 or 7 selected = "every day", fall back to */N.
        if (cronWeekdays.length > 0 && cronWeekdays.length < 7) {
          return `${m} ${h} * * ${[...cronWeekdays].sort((a, b) => a - b).join(',')}`;
        }
        return `${m} ${h} */${cronEvery} * *`;
      case 'weeks':   return `${m} ${h} * * 1`;
      case 'months':  return `${m} ${h} 1 */${cronEvery} *`;
      default:        return '*/5 * * * *';
    }
  };

  const cronHumanLabel = (): string => {
    const [hh, mm] = cronAt.split(':');
    const atStr = `${hh ?? '00'}:${mm ?? '00'}`;
    const unitLabels: Record<string, string> = { minutes: t('wiz.minutes'), hours: t('wiz.hours'), days: t('wiz.days'), weeks: t('wiz.weeks'), months: t('wiz.months') };
    if (cronUnit === 'minutes' || cronUnit === 'hours') {
      return `${t('wiz.every')} ${cronEvery} ${unitLabels[cronUnit]}`;
    }
    // Days unit with specific weekdays: "Lundi, Mercredi à 09:00"
    if (cronUnit === 'days' && cronWeekdays.length > 0 && cronWeekdays.length < 7) {
      const names = [...cronWeekdays]
        .sort((a, b) => a - b)
        .map(d => t(`wiz.weekday.${d}`))
        .join(', ');
      return `${names} ${t('wiz.at')} ${atStr}`;
    }
    return `${t('wiz.every')} ${cronEvery} ${unitLabels[cronUnit]} ${t('wiz.at')} ${atStr}`;
  };

  const [steps, setSteps] = useState<WorkflowStep[]>(editWorkflow?.steps ?? [{
    name: 'main',
    step_type: { type: 'Agent' },
    description: null,
    agent: 'ClaudeCode',
    prompt_template: '',
    mode: { type: 'Normal' },
  }]);
  const [saving, setSaving] = useState(false);
  const [availableSkills, setAvailableSkills] = useState<Skill[]>([]);
  const [availableProfiles, setAvailableProfiles] = useState<AgentProfile[]>([]);
  const [availableDirectives, setAvailableDirectives] = useState<Directive[]>([]);
  const [availableQuickPrompts, setAvailableQuickPrompts] = useState<QuickPrompt[]>([]);
  const promptTextareaRefs = useRef<Record<number, HTMLTextAreaElement | null>>({});

  /** Insert text at cursor position in the prompt textarea of step `stepIndex` */
  const insertVarAtCursor = (stepIndex: number, text: string) => {
    const el = promptTextareaRefs.current[stepIndex];
    if (!el) return;
    const start = el.selectionStart ?? el.value.length;
    const end = el.selectionEnd ?? start;
    const before = el.value.slice(0, start);
    const after = el.value.slice(end);
    const newValue = before + text + after;
    updateStep(stepIndex, { prompt_template: newValue });
    // Restore cursor position after React re-render
    requestAnimationFrame(() => {
      el.focus();
      const pos = start + text.length;
      el.setSelectionRange(pos, pos);
    });
  };

  // Workflow suggestions from MCP introspection
  const [suggestions, setSuggestions] = useState<WorkflowSuggestion[]>([]);
  const [showSuggestions, setShowSuggestions] = useState(false);
  const [suggestionsLoading, setSuggestionsLoading] = useState(false);

  useEffect(() => {
    skillsApi.list().then(setAvailableSkills).catch(() => {});
    profilesApi.list().then(setAvailableProfiles).catch(() => {});
    directivesApi.list().then(setAvailableDirectives).catch(() => {});
    quickPromptsApi.list().then(setAvailableQuickPrompts).catch(() => {});
  }, []);

  // Fetch suggestions when project changes
  useEffect(() => {
    if (!projectId) { setSuggestions([]); return; }
    setSuggestionsLoading(true);
    workflowsApi.suggestions(projectId)
      .then(s => { setSuggestions(s); if (s.length > 0 && !isEdit) setShowSuggestions(true); })
      .catch(() => setSuggestions([]))
      .finally(() => setSuggestionsLoading(false));
  }, [projectId, isEdit]);

  const applySuggestion = (s: WorkflowSuggestion) => {
    setName(s.title);
    setSteps(s.steps);
    setTriggerType(s.trigger.type as 'Cron' | 'Tracker' | 'Manual');
    if (s.trigger.type === 'Cron') {
      const parsed = parseCronExpr((s.trigger as { schedule: string }).schedule);
      setCronEvery(parsed.every);
      setCronUnit(parsed.unit);
      setCronAt(parsed.at);
      setCronWeekdays(parsed.weekdays);
      setCronRaw(parsed.raw ?? '');
    }
    setShowSuggestions(false);
    // Multi-step or advanced suggestions → force advanced mode
    if (s.steps.length > 1 || s.complexity === 'advanced') {
      setWizardMode('advanced');
      setWizardStep(2); // Jump to Steps
    } else {
      setWizardStep(isSimple ? 1 : 2); // Jump to Task (simple) or Steps (advanced)
    }
  };

  const addStep = () => {
    setSteps([...steps, {
      name: `step-${steps.length + 1}`,
      agent: 'ClaudeCode',
      prompt_template: '',
      mode: { type: 'Normal' },
    }]);
  };

  const updateStep = (idx: number, patch: Partial<WorkflowStep>) => {
    setSteps(steps.map((s, i) => i === idx ? { ...s, ...patch } : s));
  };

  const removeStep = (idx: number) => {
    if (steps.length > 1) setSteps(steps.filter((_, i) => i !== idx));
  };

  // --- on_result helpers ---
  const addCondition = (stepIdx: number) => {
    const step = steps[stepIdx];
    const conditions: StepConditionRule[] = [...(step.on_result ?? []), { contains: '', action: { type: 'Stop' } }];
    updateStep(stepIdx, { on_result: conditions });
  };

  const updateCondition = (stepIdx: number, condIdx: number, patch: Partial<StepConditionRule>) => {
    const step = steps[stepIdx];
    const conditions = (step.on_result ?? []).map((c, i) => i === condIdx ? { ...c, ...patch } : c);
    updateStep(stepIdx, { on_result: conditions });
  };

  const removeCondition = (stepIdx: number, condIdx: number) => {
    const step = steps[stepIdx];
    updateStep(stepIdx, { on_result: (step.on_result ?? []).filter((_, i) => i !== condIdx) });
  };

  const buildTrigger = (): WorkflowTrigger => {
    switch (triggerType) {
      case 'Cron': return { type: 'Cron', schedule: buildCronExpr() };
      case 'Tracker': return {
        type: 'Tracker',
        source: { type: 'GitHub', owner: trackerOwner, repo: trackerRepo },
        query: '',
        labels: trackerLabels.split(',').map(l => l.trim()).filter(Boolean),
        interval: trackerInterval,
      };
      case 'Manual': return { type: 'Manual' };
    }
  };

  const buildWorkspaceConfig = (): WorkspaceConfig | null => {
    const hasHooks = wsHookAfterCreate || wsHookBeforeRun || wsHookAfterRun || wsHookBeforeRemove;
    if (!hasHooks) return null;
    return {
      hooks: {
        after_create: wsHookAfterCreate || null,
        before_run: wsHookBeforeRun || null,
        after_run: wsHookAfterRun || null,
        before_remove: wsHookBeforeRemove || null,
      }
    };
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      const trigger = buildTrigger();
      const wsConfig = buildWorkspaceConfig();
      const safetyVal = (safety.sandbox || safety.require_approval || safety.max_files || safety.max_lines) ? safety : undefined;
      const concurrency = concurrencyLimit ? parseInt(concurrencyLimit) : undefined;

      if (isEdit && editWorkflow) {
        await workflowsApi.update(editWorkflow.id, {
          name,
          project_id: projectId || null,
          trigger,
          steps,
          actions: [],
          safety: safetyVal ?? editWorkflow.safety,
          workspace_config: wsConfig ?? undefined,
          concurrency_limit: concurrency ?? null,
        });
      } else {
        const req: CreateWorkflowRequest = {
          name,
          project_id: projectId || null,
          trigger,
          steps,
          actions: [],
          safety: safetyVal,
          workspace_config: wsConfig ?? undefined,
          concurrency_limit: concurrency,
        };
        await workflowsApi.create(req);
      }
      onDone();
    } catch (e) {
      console.warn('Workflow action failed:', e);
    } finally {
      setSaving(false);
    }
  };

  const WIZARD_STEPS_ADVANCED = [t('wiz.infos'), t('wiz.trigger'), t('wiz.steps'), t('wiz.config'), t('wiz.summary')];
  const WIZARD_STEPS_SIMPLE = [t('wiz.infos'), t('wiz.task'), t('wiz.summary')];
  const WIZARD_STEPS = isSimple ? WIZARD_STEPS_SIMPLE : WIZARD_STEPS_ADVANCED;
  const lastStep = WIZARD_STEPS.length - 1;

  return (
    <div className="wf-wizard-card">
      {/* Mode toggle */}
      {!isEdit && (
        <div className="flex-row gap-2 mb-6" style={{ justifyContent: 'center' }}>
          {(['simple', 'advanced'] as const).map(mode => (
            <button
              key={mode}
              className="wf-trigger-btn"
              data-selected={wizardMode === mode}
              onClick={() => { setWizardMode(mode); setWizardStep(0); }}
              title={mode === 'simple' ? t('wiz.helpModeSimple') : t('wiz.helpModeAdvanced')}
            >
              {mode === 'simple' ? t('wiz.modeSimple') : t('wiz.modeAdvanced')}
            </button>
          ))}
          <HelpTip hint={t('wiz.helpSimpleVsAdvanced')} />
        </div>
      )}

      {/* Suggestions toggle */}
      {suggestions.length > 0 && !showSuggestions && (
        <button
          className="wf-suggestions-toggle"
          onClick={() => setShowSuggestions(true)}
        >
          <Sparkles size={12} />
          {t('wiz.suggestionsCount', suggestions.length)}
        </button>
      )}

      {/* Suggestions panel */}
      {showSuggestions && suggestions.length > 0 && (
        <div className="wf-suggestions-panel">
          <div className="wf-suggestions-header">
            <Sparkles size={14} className="text-accent" />
            <span className="wf-suggestions-title">{t('wiz.suggestionsTitle')}</span>
            <button className="mcp-icon-btn" onClick={() => setShowSuggestions(false)} aria-label="Close" style={{ marginLeft: 'auto' }}>
              <X size={12} />
            </button>
          </div>
          <div className="wf-suggestions-grid">
            {suggestions.map(s => (
              <div key={s.id} className="wf-suggestion-card">
                <div className="wf-suggestion-top">
                  <span className="wf-suggestion-name">{s.title}</span>
                  <div className="flex-row gap-2">
                    {s.complexity === 'advanced' && <span className="wf-suggestion-complexity">{t('wiz.modeAdvanced')}</span>}
                    <span className={`wf-suggestion-audience wf-audience-${s.audience}`}>{s.audience}</span>
                  </div>
                </div>
                <p className="wf-suggestion-desc">{s.description}</p>
                <p className="wf-suggestion-reason">{s.reason}</p>
                <div className="wf-suggestion-mcps">
                  {s.required_mcps.map(m => <span key={m} className="wf-suggestion-mcp-tag">{m}</span>)}
                  <span className="wf-suggestion-steps-count">{s.steps.length} step{s.steps.length > 1 ? 's' : ''}</span>
                </div>
                <button
                  className="wf-suggestion-apply"
                  onClick={() => applySuggestion(s)}
                >
                  <Zap size={10} /> {s.steps.length > 1 || s.complexity === 'advanced' ? t('wiz.importDraft') : t('wiz.activate')}
                </button>
              </div>
            ))}
          </div>
          {suggestionsLoading && <div className="text-center"><Loader2 size={16} style={{ animation: 'spin 1s linear infinite' }} /></div>}
        </div>
      )}

      {/* Progress bar */}
      <div className="flex-row gap-2 mb-9">
        {WIZARD_STEPS.map((label, i) => (
          <div key={i} className="flex-1 text-center">
            <div className="wf-wizard-progress-segment" data-active={i <= wizardStep} />
            <span className="text-xs" style={{ color: i <= wizardStep ? '#c8ff00' : 'var(--kr-text-dim)' }}>
              {label}
            </span>
          </div>
        ))}
      </div>

      {/* Logical step name for conditional rendering */}
      {/* Simple: 0=infos, 1=task, 2=summary */}
      {/* Advanced: 0=infos, 1=trigger, 2=steps, 3=config, 4=summary */}

      {/* Step: Name + Project (both modes) */}
      {wizardStep === 0 && (
        <div>
          <label className="wf-label">{t('wiz.name')}</label>
          <input
            className="wf-input"
            value={name}
            onChange={e => setName(e.target.value)}
            placeholder={t('wiz.namePlaceholder')}
          />

          <label className="wf-label mt-6">{isEdit ? t('wiz.project') : t('wiz.projectOptional')}</label>
          <select className="wf-select" value={projectId} onChange={e => {
            const pid = e.target.value;
            setProjectId(pid);
            const proj = projects.find(p => p.id === pid);
            if (proj?.default_skill_ids?.length) {
              setSteps(prev => prev.map(s => ({ ...s, skill_ids: s.skill_ids?.length ? s.skill_ids : proj.default_skill_ids })));
            }
          }}>
            <option value="">{t('wiz.noProject')}</option>
            {projects.map(p => (
              <option key={p.id} value={p.id}>{p.name}</option>
            ))}
          </select>
        </div>
      )}

      {/* Step: Simple task (simple mode step 1) */}
      {isSimple && wizardStep === 1 && (
        <div>
          <label className="wf-label">
            {t('wiz.agentLabel')} <HelpTip hint={t('wiz.helpAgent')} />
          </label>
          <select
            className="wf-select mb-6"
            value={steps[0]?.agent ?? 'ClaudeCode'}
            onChange={e => updateStep(0, { agent: e.target.value as AgentType })}
          >
            {availableAgents.map(a => (
              <option key={a.type} value={a.type}>{a.label}</option>
            ))}
          </select>

          <label className="wf-label">
            {t('wiz.promptLabel')} <HelpTip hint={t('wiz.helpPromptSimple')} />
          </label>
          {!isEdit && !(steps[0]?.prompt_template) && (
            <div className="wf-starters">
              <span className="wf-starters-label">{t('wiz.starterLabel')} <HelpTip hint={t('wiz.helpStarters')} /></span>
              <div className="wf-starters-grid">
                {[
                  { icon: '📊', title: t('wiz.starter.codeReview'), prompt: t('wiz.starter.codeReviewPrompt') },
                  { icon: '📝', title: t('wiz.starter.changelog'), prompt: t('wiz.starter.changelogPrompt') },
                  { icon: '🔍', title: t('wiz.starter.techDebt'), prompt: t('wiz.starter.techDebtPrompt') },
                  { icon: '🧪', title: t('wiz.starter.testCoverage'), prompt: t('wiz.starter.testCoveragePrompt') },
                  { icon: '📋', title: t('wiz.starter.docUpdate'), prompt: t('wiz.starter.docUpdatePrompt') },
                  { icon: '🛡️', title: t('wiz.starter.securityScan'), prompt: t('wiz.starter.securityScanPrompt') },
                ].map(s => (
                  <button
                    key={s.title}
                    className="wf-starter-card"
                    onClick={() => {
                      setName(s.title);
                      updateStep(0, { prompt_template: s.prompt });
                    }}
                  >
                    <span className="wf-starter-icon">{s.icon}</span>
                    <span className="wf-starter-title">{s.title}</span>
                  </button>
                ))}
              </div>
            </div>
          )}
          <textarea
            className="wf-textarea mb-6"
            rows={6}
            value={steps[0]?.prompt_template ?? ''}
            onChange={e => updateStep(0, { prompt_template: e.target.value })}
            placeholder={t('wiz.promptSimplePlaceholder')}
          />

          {/* Simple trigger: Manual or Scheduled */}
          <label className="wf-label">
            {t('wiz.triggerWhenLabel')} <HelpTip hint={t('wiz.helpTriggerSimple')} />
          </label>
          <div className="flex-row gap-4 mb-4">
            <button
              className="wf-trigger-btn"
              data-selected={triggerType === 'Manual'}
              onClick={() => setTriggerType('Manual')}
              title={t('wiz.helpTriggerManual')}
            >
              <Zap size={12} /> {t('wiz.triggerManual')}
            </button>
            <button
              className="wf-trigger-btn"
              data-selected={triggerType === 'Cron'}
              onClick={() => setTriggerType('Cron')}
              title={t('wiz.helpTriggerCron')}
            >
              <Clock size={12} /> {t('wiz.triggerScheduled')}
            </button>
          </div>

          {triggerType === 'Cron' && (cronIsRaw ? (
            <div className="mb-2">
              <label className="text-xs text-muted mb-1" style={{ display: 'block' }}>Cron expression</label>
              <div className="flex-row gap-4" style={{ alignItems: 'center' }}>
                <input
                  className="wf-input"
                  style={{ flex: 1, fontFamily: 'var(--kr-font-mono)' }}
                  value={cronRaw}
                  onChange={e => setCronRaw(e.target.value)}
                  placeholder="0 7,10,13,16,19 * * 1-5"
                />
                <button
                  type="button"
                  className="text-xs text-muted"
                  style={{ background: 'none', border: 'none', cursor: 'pointer', textDecoration: 'underline' }}
                  onClick={() => { setCronRaw(''); setCronEvery(5); setCronUnit('minutes'); setCronAt('00:00'); setCronWeekdays([]); }}
                >
                  {t('wiz.cronSwitchSimple')}
                </button>
              </div>
            </div>
          ) : (
            <>
              <div className="flex-row gap-4 mb-2" style={{ alignItems: 'center' }}>
                <span className="text-base text-tertiary">{t('wiz.every')}</span>
                <input
                  type="number" min={1} max={60}
                  className="wf-input"
                  style={{ width: 60, textAlign: 'center' }}
                  value={cronEvery}
                  disabled={hasSpecificDays}
                  onChange={e => setCronEvery(Math.max(1, parseInt(e.target.value) || 1))}
                  title={hasSpecificDays ? t('wiz.cronEveryDisabledHint') : undefined}
                />
                <select
                  className="wf-select"
                  style={{ width: 130 }}
                  value={cronUnit}
                  onChange={e => {
                    const u = e.target.value as typeof cronUnit;
                    setCronUnit(u);
                    // Clear day-of-week selection when leaving "days" — stale
                    // weekdays would silently restrict any non-day pattern
                    // (e.g. "every 5 minutes only on Monday" makes no sense).
                    if (u !== 'days') setCronWeekdays([]);
                  }}
                >
                  <option value="minutes">{t('wiz.minutes')}</option>
                  <option value="hours">{t('wiz.hours')}</option>
                  <option value="days">{t('wiz.days')}</option>
                </select>
                {cronUnit === 'days' && (
                  <>
                    <span className="text-base text-tertiary">{t('wiz.at')}</span>
                    <input
                      type="time"
                      className="wf-input"
                      style={{ width: 100 }}
                      value={cronAt}
                      onChange={e => setCronAt(e.target.value)}
                    />
                  </>
                )}
              </div>
              {cronUnit === 'days' && (
                <div className="wf-cron-weekdays">
                  <span className="text-xs text-muted">{t('wiz.cronWeekdaysLabel')} <HelpTip hint={t('wiz.helpCronWeekdays')} /></span>
                  <div className="flex-row gap-2 mt-2 flex-wrap">
                    {[1, 2, 3, 4, 5, 6, 0].map(d => {
                      const active = cronWeekdays.includes(d);
                      return (
                        <button
                          key={d}
                          type="button"
                          className="wf-weekday-chip"
                          data-selected={active}
                          onClick={() => toggleWeekday(d)}
                          title={t(`wiz.weekday.${d}`)}
                          aria-pressed={active}
                        >
                          {t(`wiz.weekdayShort.${d}`)}
                        </button>
                      );
                    })}
                    {hasSpecificDays && (
                      <button
                        type="button"
                        className="text-xs text-muted"
                        style={{ background: 'none', border: 'none', cursor: 'pointer', textDecoration: 'underline' }}
                        onClick={() => setCronWeekdays([])}
                      >
                        {t('wiz.cronWeekdaysAll')}
                      </button>
                    )}
                  </div>
                  <p className="text-xs text-ghost mt-1" style={{ fontStyle: 'italic' }}>
                    {hasSpecificDays
                      ? t('wiz.cronWeekdaysPreview', [...cronWeekdays].sort((a, b) => a - b).map(d => t(`wiz.weekday.${d}`)).join(', '))
                      : t('wiz.cronWeekdaysEveryday')
                    }
                  </p>
                </div>
              )}
            </>
          ))}
        </div>
      )}

      {/* Step 1 (advanced): Trigger */}
      {!isSimple && wizardStep === 1 && (
        <div>
          <label className="wf-label">
            {t('wiz.triggerWhenLabel')} <HelpTip hint={t('wiz.helpTriggerAdvanced')} />
          </label>
          <div className="flex-row gap-4 mb-6">
            {(['Manual', 'Cron', 'Tracker'] as const).map(tt => {
              const tooltipKey = tt === 'Manual' ? 'wiz.helpTriggerManual'
                : tt === 'Cron' ? 'wiz.helpTriggerCron'
                : 'wiz.helpTriggerTracker';
              const labelKey = tt === 'Manual' ? 'wiz.triggerManual'
                : tt === 'Cron' ? 'wiz.triggerScheduled'
                : 'wiz.triggerTracker';
              return (
                <button
                  key={tt}
                  className="wf-trigger-btn"
                  data-selected={triggerType === tt}
                  onClick={() => setTriggerType(tt)}
                  title={t(tooltipKey)}
                >
                  {tt === 'Manual' && <Zap size={12} />}
                  {tt === 'Cron' && <Clock size={12} />}
                  {tt === 'Tracker' && <GitBranch size={12} />}
                  {t(labelKey)}
                </button>
              );
            })}
          </div>

          {triggerType === 'Cron' && (
            <>
              <label className="wf-label">{t('wiz.frequency')}</label>
              {cronIsRaw ? (
                <div className="mb-4">
                  <div className="flex-row gap-4" style={{ alignItems: 'center' }}>
                    <input
                      className="wf-input"
                      style={{ flex: 1, fontFamily: 'var(--kr-font-mono)' }}
                      value={cronRaw}
                      onChange={e => setCronRaw(e.target.value)}
                      placeholder="0 7,10,13,16,19 * * 1-5"
                    />
                    <button
                      type="button"
                      className="text-xs text-muted"
                      style={{ background: 'none', border: 'none', cursor: 'pointer', textDecoration: 'underline' }}
                      onClick={() => { setCronRaw(''); setCronEvery(5); setCronUnit('minutes'); setCronAt('00:00'); setCronWeekdays([]); }}
                    >
                      {t('wiz.cronSwitchSimple')}
                    </button>
                  </div>
                </div>
              ) : (
                <>
                  <div className="flex-row gap-4 mb-4">
                    <span className="text-base text-tertiary">{t('wiz.every')}</span>
                    <input
                      type="number" min={1} max={60}
                      className="wf-input"
                      style={{ width: 60, textAlign: 'center' }}
                      value={cronEvery}
                      disabled={hasSpecificDays}
                      onChange={e => setCronEvery(Math.max(1, parseInt(e.target.value) || 1))}
                      title={hasSpecificDays ? t('wiz.cronEveryDisabledHint') : undefined}
                    />
                    <select
                      className="wf-select"
                      style={{ width: 130 }}
                      value={cronUnit}
                      onChange={e => {
                    const u = e.target.value as typeof cronUnit;
                    setCronUnit(u);
                    // Clear day-of-week selection when leaving "days" — stale
                    // weekdays would silently restrict any non-day pattern
                    // (e.g. "every 5 minutes only on Monday" makes no sense).
                    if (u !== 'days') setCronWeekdays([]);
                  }}
                    >
                      <option value="minutes">{t('wiz.minutes')}</option>
                      <option value="hours">{t('wiz.hours')}</option>
                      <option value="days">{t('wiz.days')}</option>
                      <option value="weeks">{t('wiz.weeks')}</option>
                      <option value="months">{t('wiz.months')}</option>
                    </select>
                    {(cronUnit === 'days' || cronUnit === 'weeks' || cronUnit === 'months') && (
                      <>
                        <span className="text-base text-tertiary">{t('wiz.at')}</span>
                        <input
                          type="time"
                          className="wf-input"
                          style={{ width: 100 }}
                          value={cronAt}
                          onChange={e => setCronAt(e.target.value)}
                        />
                      </>
                    )}
                  </div>
                  {cronUnit === 'days' && (
                    <div className="wf-cron-weekdays">
                      <span className="text-xs text-muted">{t('wiz.cronWeekdaysLabel')} <HelpTip hint={t('wiz.helpCronWeekdays')} /></span>
                      <div className="flex-row gap-2 mt-2 flex-wrap">
                        {[1, 2, 3, 4, 5, 6, 0].map(d => {
                          const active = cronWeekdays.includes(d);
                          return (
                            <button
                              key={d}
                              type="button"
                              className="wf-weekday-chip"
                              data-selected={active}
                              onClick={() => toggleWeekday(d)}
                              title={t(`wiz.weekday.${d}`)}
                              aria-pressed={active}
                            >
                              {t(`wiz.weekdayShort.${d}`)}
                            </button>
                          );
                        })}
                        {hasSpecificDays && (
                          <button
                            type="button"
                            className="text-xs text-muted"
                            style={{ background: 'none', border: 'none', cursor: 'pointer', textDecoration: 'underline' }}
                            onClick={() => setCronWeekdays([])}
                          >
                            {t('wiz.cronWeekdaysAll')}
                          </button>
                        )}
                      </div>
                      <p className="text-xs text-ghost mt-1" style={{ fontStyle: 'italic' }}>
                        {hasSpecificDays
                          ? t('wiz.cronWeekdaysPreview', [...cronWeekdays].sort((a, b) => a - b).map(d => t(`wiz.weekday.${d}`)).join(', '))
                          : t('wiz.cronWeekdaysEveryday')
                        }
                      </p>
                    </div>
                  )}
                </>
              )}
              <div className="wf-cron-preview">
                <Clock size={12} className="text-accent flex-shrink-0" />
                <span className="text-sm text-tertiary">{cronIsRaw ? cronRaw : cronHumanLabel()}</span>
                <span className="text-xs text-ghost mono" style={{ marginLeft: 'auto' }}>
                  {buildCronExpr()}
                </span>
              </div>
            </>
          )}

          {triggerType === 'Tracker' && (
            <>
              <div className="flex-row gap-4">
                <div className="flex-1">
                  <label className="wf-label">Owner</label>
                  <input className="wf-input" value={trackerOwner} onChange={e => setTrackerOwner(e.target.value)} placeholder="owner" />
                </div>
                <div className="flex-1">
                  <label className="wf-label">Repo</label>
                  <input className="wf-input" value={trackerRepo} onChange={e => setTrackerRepo(e.target.value)} placeholder="repo" />
                </div>
              </div>
              <label className="wf-label mt-4">{t('wiz.labels')}</label>
              <input className="wf-input" value={trackerLabels} onChange={e => setTrackerLabels(e.target.value)} placeholder="bug-5xx, auto-fix" />
              <label className="wf-label mt-4">{t('wiz.pollInterval')}</label>
              <input className="wf-input" value={trackerInterval} onChange={e => setTrackerInterval(e.target.value)} placeholder="*/5 * * * *" />
            </>
          )}
        </div>
      )}

      {/* Step 2 (advanced): Steps (with advanced per-step config) */}
      {!isSimple && wizardStep === 2 && (
        <div>
          {/* Variable help toggle */}
          <button
            className="wf-small-help-btn mb-6"
            data-open={showVarHelp}
            onClick={() => setShowVarHelp(!showVarHelp)}
          >
            <HelpCircle size={12} />
            {t('wiz.availableVars')}
            <ChevronRight size={10} className={showVarHelp ? 'wf-chevron-rotated' : 'wf-chevron'} />
          </button>

          {showVarHelp && (
            <div className="wf-help-panel">
              <div className="wf-help-section">
                <div className="wf-help-title">{t('wiz.triggerVars')}</div>
                <div className="wf-help-grid">
                  {[
                    ['{{issue.title}}', t('wiz.issueTitle')],
                    ['{{issue.body}}', t('wiz.issueBody')],
                    ['{{issue.number}}', t('wiz.issueNumber')],
                    ['{{issue.url}}', t('wiz.issueUrl')],
                    ['{{issue.labels}}', t('wiz.issueLabels')],
                  ].map(([v, d]) => (
                    <div key={v} className="wf-help-row">
                      <code
                        className="wf-help-code"
                        onClick={() => navigator.clipboard.writeText(v!)}
                        title={t('wiz.clickToInsert')}
                      >{v}</code>
                      <span className="wf-help-desc">{d}</span>
                    </div>
                  ))}
                </div>
              </div>

              <div className="wf-help-section">
                <div className="wf-help-title">{t('wiz.stepChaining')}</div>
                <div className="wf-help-grid">
                  {[
                    ['{{previous_step.output}}', t('wiz.prevOutput')],
                    ['{{steps.<nom>.output}}', t('wiz.namedOutput')],
                  ].map(([v, d]) => (
                    <div key={v} className="wf-help-row">
                      <code
                        className="wf-help-code"
                        onClick={() => navigator.clipboard.writeText(v!)}
                        title={t('wiz.clickToInsert')}
                      >{v}</code>
                      <span className="wf-help-desc">{d}</span>
                    </div>
                  ))}
                </div>
              </div>

              <div className="wf-help-section">
                <div className="wf-help-title">{t('wiz.availableSignals')}</div>
                <div className="wf-help-grid">
                  {[
                    ['[SIGNAL: NO_RESULTS]', t('wiz.signalNoResults')],
                    ['[SIGNAL: CONTINUE]', t('wiz.signalContinue')],
                  ].map(([v, d]) => (
                    <div key={v} className="wf-help-row">
                      <code className="wf-help-code-signal">{v}</code>
                      <span className="wf-help-desc">{d}</span>
                    </div>
                  ))}
                </div>
                <p className="text-2xs text-ghost mt-2" style={{ margin: '4px 0 0' }}>
                  {`Auto-injecte quand des conditions (on_result) sont definies sur un step.`}
                </p>
              </div>

              <div className="wf-help-section">
                <div className="wf-help-title">{t('wiz.example')}</div>
                <div className="wf-help-example">
                  <div><span className="text-dim">{'// Step 1 : "analyze"'}</span></div>
                  <div>Analyse le bug <span className="text-accent">{'{{issue.title}}'}</span> dans <span className="text-accent">{'{{issue.url}}'}</span>.</div>
                  <div>Trouve la cause racine.</div>
                  <div style={{ height: 8 }} />
                  <div><span className="text-dim">{'// Step 2 : "fix"'}</span></div>
                  <div>Analyse : <span className="text-accent">{'{{previous_step.output}}'}</span></div>
                  <div>Ecris le correctif.</div>
                  <div style={{ height: 8 }} />
                  <div><span className="text-dim">{'// Step 3 : "verify"'}</span></div>
                  <div>Contexte : <span className="text-accent">{'{{steps.analyze.output}}'}</span></div>
                  <div>Fix : <span className="text-accent">{'{{steps.fix.output}}'}</span></div>
                  <div>Lance les tests et verifie.</div>
                </div>
              </div>
            </div>
          )}

          {steps.map((step, i) => {
            const isAdvOpen = expandedStepAdvanced === i;
            const hasAdvanced = (step.on_result && step.on_result.length > 0) ||
              step.agent_settings ||
              step.stall_timeout_secs || step.retry || step.delay_after_secs;

            return (
              <div key={i} className="wf-step-edit-card mb-6">
                <div className="flex-row gap-4 mb-4">
                  <span className="wf-step-number">
                    {i + 1}
                  </span>
                  <input
                    className="wf-input flex-1"
                    value={step.name}
                    onChange={e => updateStep(i, { name: e.target.value })}
                    placeholder={t('wiz.stepName')}
                  />
                  <select
                    className="wf-select"
                    style={{ width: 120 }}
                    value={step.agent}
                    onChange={e => updateStep(i, { agent: e.target.value as AgentType })}
                  >
                    {availableAgents.map(a => (
                      <option key={a.type} value={a.type}>{a.label}</option>
                    ))}
                  </select>
                  {steps.length > 1 && (
                    <button className="wf-icon-btn" onClick={() => removeStep(i)} aria-label="Remove step">
                      <X size={12} />
                    </button>
                  )}
                </div>
                {/* Step type + description */}
                <div className="flex-row gap-4 mb-3">
                  <div className="flex-row gap-2">
                    <button
                      className="wf-step-type-btn"
                      data-type="agent"
                      data-selected={!step.step_type || step.step_type.type === 'Agent'}
                      onClick={() => updateStep(i, { step_type: { type: 'Agent' } })}
                    >{t('wiz.stepTypeAgent')}</button>
                    <button
                      className="wf-step-type-btn"
                      data-type="api"
                      data-selected={step.step_type?.type === 'ApiCall'}
                      onClick={() => updateStep(i, { step_type: { type: 'ApiCall' } })}
                    >{t('wiz.stepTypeApiCall')}</button>
                    <button
                      className="wf-step-type-btn"
                      data-type="batch-qp"
                      data-selected={step.step_type?.type === 'BatchQuickPrompt'}
                      onClick={() => updateStep(i, { step_type: { type: 'BatchQuickPrompt' } })}
                      title={t('wiz.stepTypeBatchQPHint')}
                    >{t('wiz.stepTypeBatchQP')}</button>
                    <button
                      className="wf-step-type-btn"
                      data-type="notify"
                      data-selected={step.step_type?.type === 'Notify'}
                      onClick={() => updateStep(i, { step_type: { type: 'Notify' } })}
                      title={t('wiz.stepTypeNotifyHint')}
                    >{t('wiz.stepTypeNotify')}</button>
                  </div>
                  <input
                    className="wf-input flex-1 text-sm"
                    value={step.description ?? ''}
                    onChange={e => updateStep(i, { description: e.target.value || null })}
                    placeholder={t('wiz.stepDescriptionPlaceholder')}
                  />
                </div>
                {step.step_type?.type !== 'BatchQuickPrompt' && checkAgentRestricted(agentAccess, step.agent) && (
                  <div className="wf-restricted-warning">
                    <AlertTriangle size={12} />
                    <span>{t('config.restrictedStep')}</span>
                    <span className="cursor-pointer" style={{ textDecoration: 'underline', marginLeft: 4 }}
                      onClick={() => window.location.hash = '#config'}
                    >{t('config.restrictedAgentLink')}</span>
                  </div>
                )}
                {/* ── BatchQuickPrompt form ── */}
                {step.step_type?.type === 'BatchQuickPrompt' ? (() => {
                  const selectedQp = availableQuickPrompts.find(qp => qp.id === step.batch_quick_prompt_id);
                  const qpMissing = !step.batch_quick_prompt_id;
                  const itemsFromEmpty = !step.batch_items_from || !step.batch_items_from.trim();
                  const isFirstStep = i === 0;
                  return (
                  <div className="wf-batch-qp-form">
                    <div className="wf-batch-intro">
                      <Layers size={14} />
                      <div>
                        <strong>{t('wiz.batchQPTitle')}</strong>
                        <p className="text-xs text-muted">{t('wiz.batchQPHint')}</p>
                      </div>
                    </div>

                    {isFirstStep && (
                      <div className="wf-batch-info mt-2">
                        <AlertTriangle size={12} />
                        <span>{t('wiz.batchFirstStepWarning')}</span>
                      </div>
                    )}

                    {availableQuickPrompts.length === 0 ? (
                      <div className="wf-restricted-warning">
                        <AlertTriangle size={12} />
                        <span>{t('wiz.batchQPNoQPs')}</span>
                      </div>
                    ) : (
                      <>
                        {/* ─── QP picker (required) ─── */}
                        <label className="text-xs text-muted mb-1">
                          {t('wiz.batchQPPicker')} <span className="wf-required">*</span>
                        </label>
                        <select
                          className="wf-input text-sm"
                          data-invalid={qpMissing}
                          value={step.batch_quick_prompt_id ?? ''}
                          onChange={e => updateStep(i, { batch_quick_prompt_id: e.target.value || null })}
                        >
                          <option value="">— {t('wiz.batchQPPickerEmpty')} —</option>
                          {availableQuickPrompts.map(qp => (
                            <option key={qp.id} value={qp.id}>
                              {qp.icon} {qp.name}
                            </option>
                          ))}
                        </select>
                        {qpMissing ? (
                          <p className="wf-field-error">{t('wiz.batchQPRequired')}</p>
                        ) : selectedQp && (
                          <div className="wf-qp-preview">
                            <div className="wf-qp-preview-head">
                              <span className="wf-qp-preview-icon">{selectedQp.icon}</span>
                              <span className="font-semibold text-sm">{selectedQp.name}</span>
                              <span className="text-xs text-dim">→ {AGENT_LABELS[selectedQp.agent] ?? selectedQp.agent}</span>
                            </div>
                            {selectedQp.description && (
                              <p className="text-xs text-muted mt-1">{selectedQp.description}</p>
                            )}
                            {selectedQp.variables.length > 0 && (
                              <p className="text-xs text-ghost mt-1">
                                {t('wiz.batchQPVarSubst', selectedQp.variables[0].name)}
                              </p>
                            )}
                          </div>
                        )}

                        {/* ─── Items source (required) ─── */}
                        <label className="text-xs text-muted mb-1 mt-3">
                          {t('wiz.batchItemsFrom')} <span className="wf-required">*</span>
                        </label>
                        <textarea
                          className="wf-textarea"
                          data-invalid={itemsFromEmpty}
                          rows={2}
                          value={step.batch_items_from ?? ''}
                          onChange={e => updateStep(i, { batch_items_from: e.target.value || null })}
                          placeholder={i > 0
                            ? `{{steps.${steps[i - 1].name}.data.items}}`
                            : '{{steps.fetch.data.items}}'
                          }
                        />
                        {itemsFromEmpty && (
                          <p className="wf-field-error">{t('wiz.batchItemsFromRequired')}</p>
                        )}
                        {i > 0 && (
                          <div className="mt-2">
                            <p className="text-xs text-ghost mb-1">{t('wiz.batchItemsFromHelper')}</p>
                            <div className="flex-wrap flex-row gap-2">
                              {steps.slice(0, i).map(prev => (
                                <button
                                  key={prev.name}
                                  type="button"
                                  className="wf-batch-prev-chip"
                                  onClick={() => updateStep(i, { batch_items_from: `{{steps.${prev.name}.data}}` })}
                                  title={t('wiz.batchItemsFromPickStep', prev.name)}
                                >
                                  {t('wiz.batchItemsFromFromStep', prev.name)}
                                </button>
                              ))}
                            </div>
                          </div>
                        )}
                        <div className="flex-row gap-4 mt-3 flex-wrap">
                          <label className="flex-row gap-2 text-xs" style={{ alignItems: 'center' }}>
                            <input
                              type="checkbox"
                              checked={step.batch_wait_for_completion ?? true}
                              onChange={e => updateStep(i, { batch_wait_for_completion: e.target.checked })}
                            />
                            {t('wiz.batchWaitForCompletion')}
                          </label>
                          <label className="flex-row gap-2 text-xs" style={{ alignItems: 'center' }}>
                            {t('wiz.batchMaxItems')}
                            <input
                              type="number"
                              min={1}
                              max={50}
                              className="wf-input text-xs"
                              style={{ width: 60 }}
                              value={step.batch_max_items ?? 50}
                              onChange={e => updateStep(i, { batch_max_items: parseInt(e.target.value) || null })}
                            />
                          </label>
                        </div>
                        {/* Worktree toggle — needs a project (QP's or workflow's) to be useful */}
                        {(() => {
                          const hasProject = Boolean(selectedQp?.project_id || projectId);
                          const isIsolated = step.batch_workspace_mode === 'Isolated';
                          return (
                            <label
                              className="flex-row gap-2 text-xs mt-2"
                              style={{ alignItems: 'center', opacity: hasProject ? 1 : 0.5 }}
                              title={hasProject ? t('wiz.batchWorktreeHint') : t('wiz.batchWorktreeNoProject')}
                            >
                              <input
                                type="checkbox"
                                checked={isIsolated}
                                disabled={!hasProject}
                                onChange={e => updateStep(i, {
                                  batch_workspace_mode: e.target.checked ? 'Isolated' : 'Direct',
                                })}
                              />
                              <GitBranch size={10} /> {t('wiz.batchWorktree')}
                            </label>
                          );
                        })()}
                      </>
                    )}
                  </div>
                  );
                })() : step.step_type?.type === 'Notify' ? (
                  <div className="wf-notify-form">
                    <div className="wf-batch-intro">
                      <Send size={14} />
                      <div>
                        <strong>{t('wiz.notifyTitle')}</strong>
                        <p className="text-xs text-muted">{t('wiz.notifyHint')}</p>
                      </div>
                    </div>
                    <label className="text-xs text-muted mb-1">{t('wiz.notifyUrl')} <span className="wf-required">*</span></label>
                    <input
                      className="wf-input text-sm mb-2"
                      value={step.notify_config?.url ?? ''}
                      onChange={e => updateStep(i, { notify_config: { ...step.notify_config ?? { url: '', method: 'POST', headers: {}, body_template: '' }, url: e.target.value } })}
                      placeholder="https://hooks.slack.com/services/... ou {{steps.fetch.data.webhook_url}}"
                    />
                    <div className="flex-row gap-4 mb-2">
                      <div>
                        <label className="text-xs text-muted">{t('wiz.notifyMethod')}</label>
                        <select
                          className="wf-select text-sm"
                          value={step.notify_config?.method ?? 'POST'}
                          onChange={e => updateStep(i, { notify_config: { ...step.notify_config ?? { url: '', method: 'POST', headers: {}, body_template: '' }, method: e.target.value } })}
                        >
                          <option value="POST">POST</option>
                          <option value="PUT">PUT</option>
                          <option value="GET">GET</option>
                        </select>
                      </div>
                    </div>
                    <label className="text-xs text-muted mb-1">{t('wiz.notifyBody')}</label>
                    <textarea
                      className="wf-textarea text-sm mb-2"
                      rows={4}
                      value={step.notify_config?.body_template ?? ''}
                      onChange={e => updateStep(i, { notify_config: { ...step.notify_config ?? { url: '', method: 'POST', headers: {}, body_template: '' }, body_template: e.target.value } })}
                      placeholder={`{"text": "Workflow terminé : {{previous_step.summary}}"}`}
                    />
                    {i > 0 && (
                      <div className="mt-1 text-xs text-ghost flex-wrap flex-row gap-1">
                        <span>{t('wiz.clickToInsert')} :</span>
                        <code className="wf-var-hint-code" style={{ cursor: 'default' }}>{'{{previous_step.output}}'}</code>
                        <code className="wf-var-hint-code" style={{ cursor: 'default' }}>{'{{previous_step.summary}}'}</code>
                        {steps.slice(0, i).map(prev => (
                          <code key={prev.name} className="wf-var-hint-code" style={{ cursor: 'default' }}>
                            {`{{steps.${prev.name}.output}}`}
                          </code>
                        ))}
                      </div>
                    )}
                  </div>
                ) : (
                  <>
                    <textarea
                      ref={el => { promptTextareaRefs.current[i] = el; }}
                      className="wf-textarea"
                      rows={3}
                      value={step.prompt_template}
                      onChange={e => updateStep(i, { prompt_template: e.target.value })}
                      placeholder={i === 0
                        ? 'Prompt template... ex: Analyse le bug {{issue.title}}. Trouve la cause racine.'
                        : `Prompt template... ex: Voici l'analyse : {{previous_step.output}}. Ecris le correctif.`
                      }
                    />
                    {/* Hint: available variables for this step (clickable) */}
                    {i > 0 && (
                      <div className="mt-2 text-xs text-ghost flex-wrap flex-row gap-1">
                        <span>{t('wiz.clickToInsert')} :</span>
                        <code
                          className="wf-var-hint-code"
                          onClick={() => insertVarAtCursor(i, '{{previous_step.output}}')}
                          title={t('wiz.prevOutput')}
                        >{'{{previous_step.output}}'}</code>
                        {steps.slice(0, i).map(prev => (
                          <code
                            key={prev.name}
                            className="wf-var-hint-code"
                            onClick={() => insertVarAtCursor(i, `{{steps.${prev.name}.output}}`)}
                            title={`${t('wiz.namedOutput')}: ${prev.name}`}
                          >{`{{steps.${prev.name}.output}}`}</code>
                        ))}
                      </div>
                    )}
                  </>
                )}

                {/* Skills selector per step */}
                {availableSkills.length > 0 && (
                  <div className="mt-4">
                    <label className="wf-selector-label">
                      <Zap size={9} /> {t('skills.selectSkills')}
                    </label>
                    <div className="flex-wrap flex-row gap-2 mt-2">
                      {availableSkills.map(skill => {
                        const ids = step.skill_ids ?? [];
                        const selected = ids.includes(skill.id);
                        return (
                          <button
                            key={skill.id}
                            type="button"
                            onClick={() => {
                              const newIds = selected ? ids.filter(id => id !== skill.id) : [...ids, skill.id];
                              updateStep(i, { skill_ids: newIds.length > 0 ? newIds : undefined });
                            }}
                            className="wf-chip wf-chip-skill"
                            data-selected={selected}
                            title={skill.name}
                          >
                            {selected && <Check size={8} />}
                            {skill.name}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                )}

                {/* Profile selector per step (single-select) */}
                {availableProfiles.length > 0 && (
                  <div className="mt-4">
                    <label className="wf-selector-label">
                      <UserCircle size={9} /> {t('profiles.select')}
                    </label>
                    <div className="flex-wrap flex-row gap-2 mt-2">
                      <button
                        type="button"
                        onClick={() => updateStep(i, { profile_ids: [] })}
                        className="wf-chip wf-chip-profile-none"
                        data-selected={!step.profile_ids?.length}
                      >
                        {t('profiles.none')}
                      </button>
                      {availableProfiles.map(profile => {
                        const selected = step.profile_ids?.includes(profile.id) ?? false;
                        return (
                          <button
                            key={profile.id}
                            type="button"
                            onClick={() => updateStep(i, { profile_ids: selected ? (step.profile_ids ?? []).filter(id => id !== profile.id) : [...(step.profile_ids ?? []), profile.id] })}
                            className="wf-chip wf-chip-profile"
                            data-selected={selected}
                            style={selected ? {
                              fontWeight: 600,
                              border: `1px solid ${profile.color || 'rgba(139,92,246,0.4)'}`,
                              background: `${profile.color}15`,
                              color: profile.color || '#a78bfa',
                            } : undefined}
                            title={profile.role}
                          >
                            {selected && <Check size={8} />}
                            {profile.avatar} {profile.persona_name || profile.name}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                )}

                {/* Directive selector per step (multi-select) */}
                {availableDirectives.length > 0 && (
                  <div className="mt-4">
                    <label className="wf-selector-label">
                      <FileText size={9} /> {t('directives.title')}
                    </label>
                    <div className="flex-wrap flex-row gap-2 mt-2">
                      {availableDirectives.map(directive => {
                        const ids = step.directive_ids ?? [];
                        const selected = ids.includes(directive.id);
                        return (
                          <button
                            key={directive.id}
                            type="button"
                            onClick={() => {
                              const newIds = selected
                                ? ids.filter(id => id !== directive.id)
                                : [...ids, directive.id];
                              updateStep(i, { directive_ids: newIds });
                            }}
                            className="wf-chip wf-chip-directive"
                            data-selected={selected}
                            title={directive.name}
                          >
                            {selected && <Check size={8} />}
                            {directive.icon} {directive.name}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                )}

                {/* Advanced toggle */}
                <button
                  className="wf-advanced-toggle"
                  style={{ color: hasAdvanced ? '#c8ff00' : 'rgba(255,255,255,0.25)' }}
                  onClick={() => setExpandedStepAdvanced(isAdvOpen ? null : i)}
                >
                  <Settings size={10} />
                  {t('wiz.advanced')}{hasAdvanced ? ' *' : ''}
                  {isAdvOpen ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
                </button>

                {isAdvOpen && (
                  <div className="wf-advanced-panel">
                    {/* Output format */}
                    <div className="mb-5">
                      <label className="wf-label">{t('wiz.outputFormat')}</label>
                      <div className="flex-row gap-3">
                        <button
                          className="wf-step-type-btn"
                          data-selected={!step.output_format || step.output_format.type === 'FreeText'}
                          onClick={() => updateStep(i, { output_format: { type: 'FreeText' } })}
                        >{t('wiz.outputFree')}</button>
                        <button
                          className="wf-step-type-btn"
                          data-selected={step.output_format?.type === 'Structured'}
                          onClick={() => updateStep(i, { output_format: { type: 'Structured' } })}
                        >{t('wiz.outputStructured')}</button>
                      </div>
                      {step.output_format?.type === 'Structured' && (
                        <p className="text-2xs text-ghost mt-2">{t('wiz.outputStructuredHint')}</p>
                      )}
                    </div>

                    {/* Agent settings */}
                    <div className="mb-5">
                      <label className="wf-label">{t('wiz.agentSettings')}</label>
                      <div className="flex-row gap-3">
                        <div className="flex-1">
                          <label className="wf-label text-2xs">{t('wiz.model')}</label>
                          <input
                            className="wf-input"
                            value={step.agent_settings?.model ?? ''}
                            onChange={e => updateStep(i, {
                              agent_settings: { ...step.agent_settings, model: e.target.value || null }
                            })}
                            placeholder="ex: o3"
                          />
                        </div>
                        <div className="flex-1">
                          <label className="wf-label text-2xs">Reasoning effort</label>
                          <select
                            className="wf-select"
                            value={step.agent_settings?.reasoning_effort ?? ''}
                            onChange={e => updateStep(i, {
                              agent_settings: { ...step.agent_settings, reasoning_effort: e.target.value || null }
                            })}
                          >
                            <option value="">default</option>
                            <option value="low">low</option>
                            <option value="medium">medium</option>
                            <option value="high">high</option>
                          </select>
                        </div>
                        <div className="flex-1">
                          <label className="wf-label text-2xs">Max tokens</label>
                          <input
                            type="number"
                            className="wf-input"
                            value={step.agent_settings?.max_tokens ?? ''}
                            onChange={e => updateStep(i, {
                              agent_settings: { ...step.agent_settings, max_tokens: e.target.value ? parseInt(e.target.value) : null }
                            })}
                            placeholder="ex: 16000"
                          />
                        </div>
                      </div>
                    </div>

                    {/* Stall timeout */}
                    <div className="flex-row gap-6 mb-5">
                      <div>
                        <label className="wf-label">{t('wiz.stallTimeout')}</label>
                        <input
                          type="number" min={0}
                          className="wf-input"
                          style={{ width: 90 }}
                          value={step.stall_timeout_secs ?? ''}
                          onChange={e => updateStep(i, {
                            stall_timeout_secs: e.target.value ? parseInt(e.target.value) : null,
                          })}
                          placeholder="600"
                        />
                      </div>

                      <div>
                        <label className="wf-label">{t('wiz.delayAfter')}</label>
                        <input
                          type="number" min={0}
                          className="wf-input"
                          style={{ width: 90 }}
                          value={step.delay_after_secs ?? ''}
                          onChange={e => updateStep(i, {
                            delay_after_secs: e.target.value ? parseInt(e.target.value) : null,
                          })}
                          placeholder="0"
                        />
                      </div>

                      {/* Retry */}
                      <div className="flex-1">
                        <label className="wf-label">{t('wiz.retry')}</label>
                        <div className="flex-row gap-3">
                          <input
                            type="number" min={0} max={10}
                            className="wf-input"
                            style={{ width: 60 }}
                            value={step.retry?.max_retries ?? ''}
                            onChange={e => {
                              const val = e.target.value ? parseInt(e.target.value) : 0;
                              updateStep(i, {
                                retry: val > 0 ? { max_retries: val, backoff: step.retry?.backoff ?? 'exponential' } : null,
                              });
                            }}
                            placeholder="0"
                          />
                          <select
                            className="wf-select"
                            style={{ width: 120 }}
                            value={step.retry?.backoff ?? 'exponential'}
                            onChange={e => {
                              if (step.retry) {
                                updateStep(i, { retry: { ...step.retry, backoff: e.target.value } });
                              }
                            }}
                            disabled={!step.retry}
                          >
                            <option value="exponential">exponential</option>
                            <option value="fixed">fixed</option>
                          </select>
                        </div>
                      </div>
                    </div>

                    {/* on_result conditions */}
                    <div>
                      <label className="wf-label">{t('wiz.conditions')}</label>
                      {(step.on_result ?? []).map((cond, j) => (
                        <div key={j} className="flex-row gap-3 mb-2">
                          <span className="text-xs text-dim" style={{ whiteSpace: 'nowrap' }}>{t('wiz.ifContains')}</span>
                          <input
                            className="wf-input flex-1 text-sm"
                            style={{ borderColor: !cond.contains ? 'rgba(255,77,106,0.4)' : undefined }}
                            value={cond.contains}
                            onChange={e => updateCondition(i, j, { contains: e.target.value })}
                            placeholder="NO_RESULTS (obligatoire)"
                          />
                          <span className="text-xs text-dim">&rarr;</span>
                          <select
                            className="wf-select text-sm"
                            style={{ width: 100 }}
                            value={cond.action.type}
                            onChange={e => {
                              const type = e.target.value as 'Stop' | 'Skip' | 'Goto';
                              const action = type === 'Goto' ? { type: 'Goto' as const, step_name: '' } : { type };
                              updateCondition(i, j, { action: action as StepConditionRule['action'] });
                            }}
                          >
                            <option value="Stop">Stop</option>
                            <option value="Skip">Skip</option>
                            <option value="Goto">Goto</option>
                          </select>
                          {cond.action.type === 'Goto' && (
                            <input
                              className="wf-input text-sm"
                              style={{ width: 80 }}
                              value={cond.action.type === 'Goto' ? cond.action.step_name : ''}
                              onChange={e => updateCondition(i, j, { action: { type: 'Goto', step_name: e.target.value } })}
                              placeholder="step name"
                            />
                          )}
                          <button className="wf-icon-btn" onClick={() => removeCondition(i, j)} aria-label="Remove condition">
                            <X size={10} />
                          </button>
                        </div>
                      ))}
                      {(step.on_result ?? []).length === 0 && (
                        <div className="flex-row gap-2 mt-2 flex-wrap">
                          <button className="wf-add-step-btn wf-add-step-btn-inline" onClick={() => addCondition(i)}>
                            <Plus size={10} /> Condition custom
                          </button>
                          <span className="text-2xs text-ghost" style={{ alignSelf: 'center' }}>ou :</span>
                          <button
                            className="wf-add-step-btn wf-add-step-btn-preset"
                            onClick={() => updateStep(i, { on_result: [{ contains: 'NO_RESULTS', action: { type: 'Stop' } }] })}
                          >{t('wiz.noResultsStop')}</button>
                        </div>
                      )}
                      <p className="text-2xs text-ghost" style={{ margin: '4px 0 0' }}>
                        L'agent recevra l'instruction de terminer par <code className="text-accent" style={{ opacity: 0.4 }}>[SIGNAL: mot-cle]</code> en derniere ligne.
                      </p>
                    </div>
                  </div>
                )}
              </div>
            );
          })}
          <button className="wf-add-step-btn" onClick={addStep}>
            <Plus size={12} /> {t('wiz.addStep')}
          </button>
        </div>
      )}

      {/* Step 3 (advanced): Config (Safety + Workspace + Concurrency) */}
      {!isSimple && wizardStep === 3 && (
        <div>
          {/* Safety */}
          <div className="mb-8">
            <div className="flex-row gap-3 mb-4">
              <Shield size={14} className="text-muted" />
              <span className="text-md font-semibold text-secondary">{t('wiz.security')}</span>
            </div>

            <div className="flex-row gap-6 mb-4">
              <label className="wf-checkbox-label">
                <input type="checkbox" checked={safety.sandbox} onChange={e => setSafety({ ...safety, sandbox: e.target.checked })} />
                <span>{t('wiz.sandbox')}</span>
              </label>
              <label className="wf-checkbox-label">
                <input type="checkbox" checked={safety.require_approval} onChange={e => setSafety({ ...safety, require_approval: e.target.checked })} />
                <span>{t('wiz.requireApproval')}</span>
              </label>
            </div>

            <div className="flex-row gap-4">
              <div>
                <label className="wf-label">{t('wiz.maxFiles')}</label>
                <input
                  type="number" min={0}
                  className="wf-input"
                  style={{ width: 90 }}
                  value={safety.max_files ?? ''}
                  onChange={e => setSafety({ ...safety, max_files: e.target.value ? parseInt(e.target.value) : null })}
                  placeholder="illimite"
                />
              </div>
              <div>
                <label className="wf-label">{t('wiz.maxLines')}</label>
                <input
                  type="number" min={0}
                  className="wf-input"
                  style={{ width: 90 }}
                  value={safety.max_lines ?? ''}
                  onChange={e => setSafety({ ...safety, max_lines: e.target.value ? parseInt(e.target.value) : null })}
                  placeholder="illimite"
                />
              </div>
            </div>
          </div>

          {/* Expert options toggle */}
          <button
            className="wf-advanced-toggle"
            style={{ color: showExpertConfig ? '#c8ff00' : 'rgba(255,255,255,0.25)' }}
            onClick={() => setShowExpertConfig(!showExpertConfig)}
          >
            <Settings size={10} />
            {t('wiz.advanced')}
            {showExpertConfig ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
          </button>

          {showExpertConfig && (
            <div className="wf-advanced-panel">
              {/* Concurrency */}
              <div className="mb-8">
                <label className="wf-label">{t('wiz.concurrency')}</label>
                <input
                  type="number" min={1} max={20}
                  className="wf-input"
                  style={{ width: 90 }}
                  value={concurrencyLimit}
                  onChange={e => setConcurrencyLimit(e.target.value)}
                  placeholder="illimite"
                />
              </div>

              {/* Workspace hooks */}
              <div>
                <div className="flex-row gap-3 mb-4">
                  <GitBranch size={14} className="text-muted" />
                  <span className="text-md font-semibold text-secondary">{t('wiz.hooks')}</span>
                </div>
                <p className="text-xs text-faint" style={{ margin: '0 0 8px' }}>
                  {t('wiz.hooksHint')}
                </p>

                {([
                  ['after_create', t('wiz.hookAfterCreate'), wsHookAfterCreate, setWsHookAfterCreate, 'npm install'],
                  ['before_run', t('wiz.hookBeforeRun'), wsHookBeforeRun, setWsHookBeforeRun, 'git pull origin main'],
                  ['after_run', t('wiz.hookAfterRun'), wsHookAfterRun, setWsHookAfterRun, 'npm run lint'],
                  ['before_remove', t('wiz.hookBeforeRemove'), wsHookBeforeRemove, setWsHookBeforeRemove, 'git stash'],
                ] as [string, string, string, (v: string) => void, string][]).map(([key, label, value, setter, placeholder]) => (
                  <div key={key} className="mb-3">
                    <label className="wf-label text-xs">{label} ({key})</label>
                    <input
                      className="wf-input"
                      value={value}
                      onChange={e => setter(e.target.value)}
                      placeholder={placeholder}
                    />
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      )}

      {/* Step: Summary (last step in both modes) */}
      {wizardStep === lastStep && (
        <div>
          <div className="wf-summary-row"><span className="wf-summary-label">Nom</span> {name}</div>
          <div className="wf-summary-row"><span className="wf-summary-label">Projet</span> {projects.find(p => p.id === projectId)?.name ?? 'Aucun'}</div>
          <div className="wf-summary-row">
            <span className="wf-summary-label">Trigger</span>
            {triggerType === 'Cron' ? `${cronHumanLabel()} (${buildCronExpr()})` : triggerType === 'Tracker' ? `Tracker: ${trackerOwner}/${trackerRepo}` : 'Manuel'}
          </div>
          {concurrencyLimit && (
            <div className="wf-summary-row"><span className="wf-summary-label">Concurrence</span> max {concurrencyLimit} runs</div>
          )}
          <div className="wf-summary-row"><span className="wf-summary-label">Steps</span> {steps.length}</div>
          {steps.map((s, i) => {
            const typeKind = s.step_type?.type ?? 'Agent';
            const typeLabel = typeKind === 'ApiCall' ? 'API' : typeKind === 'BatchQuickPrompt' ? 'BATCH' : 'AGENT';
            const typeData = typeKind === 'ApiCall' ? 'api' : typeKind === 'BatchQuickPrompt' ? 'batch-qp' : 'agent';
            const isBatch = typeKind === 'BatchQuickPrompt';
            const qpName = isBatch && s.batch_quick_prompt_id
              ? availableQuickPrompts.find(qp => qp.id === s.batch_quick_prompt_id)?.name
              : null;
            return (
            <div key={i} className="wf-summary-row" style={{ paddingLeft: 20 }}>
              {i + 1}. <span className="wf-summary-step-type" data-type={typeData}>{typeLabel}</span>
              <span className="font-semibold" style={{ color: isBatch ? '#888' : (AGENT_COLORS[s.agent] ?? '#888') }}>{s.name}</span>
              {isBatch
                ? <span className="text-dim text-xs"> ({qpName ?? t('wiz.batchQPPickerEmpty')})</span>
                : <> ({AGENT_LABELS[s.agent] ?? s.agent})</>
              }
              {s.description && <span className="text-faint text-xs" style={{ fontStyle: 'italic' }}> &mdash; {s.description}</span>}
              {s.on_result && s.on_result.length > 0 && <span className="text-dim text-xs"> [{s.on_result.length} condition{s.on_result.length > 1 ? 's' : ''}]</span>}
              {s.retry && <span className="text-dim text-xs"> [retry x{s.retry.max_retries}]</span>}
              {s.stall_timeout_secs && <span className="text-dim text-xs"> [timeout {s.stall_timeout_secs}s]</span>}
              {s.delay_after_secs && <span className="text-dim text-xs"> [delai {s.delay_after_secs}s]</span>}
            </div>
            );
          })}
          {(safety.sandbox || safety.require_approval || safety.max_files || safety.max_lines) && (
            <div className="wf-summary-row">
              <span className="wf-summary-label">Securite</span>
              {[
                safety.sandbox && 'sandbox',
                safety.require_approval && 'approbation',
                safety.max_files && `max ${safety.max_files} fichiers`,
                safety.max_lines && `max ${safety.max_lines} lignes`,
              ].filter(Boolean).join(', ')}
            </div>
          )}
          {(wsHookAfterCreate || wsHookBeforeRun || wsHookAfterRun || wsHookBeforeRemove) && (
            <div className="wf-summary-row">
              <span className="wf-summary-label">Hooks</span>
              {[
                wsHookAfterCreate && 'after_create',
                wsHookBeforeRun && 'before_run',
                wsHookAfterRun && 'after_run',
                wsHookBeforeRemove && 'before_remove',
              ].filter(Boolean).join(', ')}
            </div>
          )}
        </div>
      )}

      {/* Validation errors */}
      {wizardStep === lastStep && (() => {
        const errors: string[] = [];
        if (!name) errors.push(t('wiz.errorNoName'));
        steps.forEach((s, i) => {
          const label = s.name || `step-${i + 1}`;
          if (s.step_type?.type === 'BatchQuickPrompt') {
            // Batch steps have no prompt_template of their own — the prompt
            // comes from the referenced Quick Prompt. We validate the batch
            // fields instead.
            if (!s.batch_quick_prompt_id) {
              errors.push(t('wiz.errorBatchNoQP').replace('{0}', label));
            }
            if (!s.batch_items_from || !s.batch_items_from.trim()) {
              errors.push(t('wiz.errorBatchNoItemsFrom').replace('{0}', label));
            }
          } else if (!s.prompt_template) {
            errors.push(t('wiz.errorNoPrompt').replace('{0}', label));
          }
          (s.on_result ?? []).forEach((r, j) => {
            if (!r.contains) errors.push(t('wiz.errorNoCondition').replace('{0}', label).replace('{1}', String(j + 1)));
          });
        });
        return errors.length > 0 ? (
          <div className="wf-validation-errors">
            {errors.map((err, i) => (
              <div key={i} className="wf-validation-error">&bull; {err}</div>
            ))}
          </div>
        ) : null;
      })()}

      {/* Navigation */}
      <div className="flex-between mt-9">
        <button className="wf-cancel-btn" onClick={wizardStep === 0 ? onCancel : () => setWizardStep(wizardStep - 1)}>
          {wizardStep === 0 ? t('common.cancel') : t('wiz.previous')}
        </button>
        {wizardStep < lastStep ? (
          <button
            className="wf-next-btn"
            onClick={() => setWizardStep(wizardStep + 1)}
            disabled={wizardStep === 0 && !name}
          >
            {t('wiz.next')} <ChevronRight size={12} />
          </button>
        ) : (
          <button
            className="wf-next-btn"
            onClick={handleSave}
            disabled={saving || !name || steps.some(s => {
              // BatchQuickPrompt steps validate their own fields instead of prompt_template
              if (s.step_type?.type === 'BatchQuickPrompt') {
                if (!s.batch_quick_prompt_id) return true;
                if (!s.batch_items_from || !s.batch_items_from.trim()) return true;
              } else if (!s.prompt_template) {
                return true;
              }
              return (s.on_result ?? []).some(r => !r.contains);
            })}
          >
            {saving ? <Loader2 size={12} /> : <Check size={12} />}
            {isEdit ? t('wiz.save') : t('wiz.create')}
          </button>
        )}
      </div>
    </div>
  );
}

import { useState, useRef, useEffect } from 'react';
import { useT } from '../../lib/I18nContext';
import { workflows as workflowsApi, skills as skillsApi, profiles as profilesApi, directives as directivesApi, quickPrompts as quickPromptsApi, quickApis as quickApisApi, mcps as mcpsApi } from '../../lib/api';
import { ApiCallStepCard, type ApiPluginOption } from './ApiCallStepCard';
import { STARTER_TEMPLATES, cloneTemplateSteps } from '../../lib/workflow-templates/chartbeat-top5';
import { buildV07Presets } from '../../lib/workflow-templates/v07-presets';
import { parseRepoUrl, buildOldestIssueRequest, inferTrackerSlugFromRepoUrl } from '../../lib/constants';
import { AGENT_COLORS, AGENT_LABELS, ALL_AGENT_TYPES, isAgentRestricted } from '../../lib/constants';
import type {
  Project, Workflow, WorkflowTrigger,
  WorkflowStep, AgentType, WorkflowSafety,
  WorkspaceConfig, StepConditionRule,
  CreateWorkflowRequest, Skill, AgentProfile, Directive,
  WorkflowSuggestion, QuickPrompt, QuickApi, WorkflowGuards,
  PromptVariable,
} from '../../types/generated';
import { ExecutionLimitsCard } from './ExecutionLimitsCard';
import type { AgentsConfig } from '../../types/generated';
import {
  Plus, Loader2, Check, X, ChevronRight, ChevronDown, ChevronUp,
  Clock, GitBranch, Zap, HelpCircle, Settings, Shield,
  AlertTriangle, UserCircle, FileText, Sparkles, Layers, Send,
  Info, Hand, RotateCcw, Terminal, Bot, Plug, Braces,
} from 'lucide-react';
import { scanUndeclaredVars } from '../../lib/scanUndeclaredVars';
import { userError } from '../../lib/userError';
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
  /** Backend "agent output language" (Settings → Output language). Distinct from
   *  the UI locale (`useT()`): UI labels follow the user's interface language,
   *  but agents reply in this language. Used by the ApiCall AI helper to pick
   *  both the system-prompt translation and the discussion's `language` param. */
  configLanguage?: string;
  /** 0.8.2 — Deep-link from the audit-validation CTA: pre-select a preset
   *  (e.g. `ticket-to-pr` for AutoPilot) and a project so the wizard opens
   *  ready-to-save. Both are best-effort: if the preset id is unknown or
   *  the project no longer exists, the wizard falls back to its blank state. */
  initialPresetId?: string;
  initialProjectId?: string;
}

export function WorkflowWizard({ projects, editWorkflow, onDone, onCancel, installedAgentTypes, agentAccess, configLanguage, initialPresetId, initialProjectId }: WorkflowWizardProps) {
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

  // 0.7.0 — Execution limits (timeout / max LLM calls / loop detection)
  const [guards, setGuards] = useState<WorkflowGuards | null>(editWorkflow?.guards ?? null);

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
    // Structured by default — see addStep() for the rationale.
    output_format: { type: 'Structured' },
  }]);
  // 0.7.0 Phase 7 — rollback / compensation chain. Empty by default.
  // Wizard supports adding Notify-only rollback steps (the most common
  // case: "tell ops on failure"). Agent / ApiCall rollback steps can
  // be added via the API directly — keeping the wizard focused.
  const [onFailureSteps, setOnFailureSteps] = useState<WorkflowStep[]>(editWorkflow?.on_failure ?? []);
  // 0.7.0 Phase 5 — list of binaries `StepType::Exec` may invoke. Empty
  // by default = Exec disabled. Editable in the Config tab; the step
  // form's command dropdown is filled from this list.
  const [execAllowlist, setExecAllowlist] = useState<string[]>(editWorkflow?.exec_allowlist ?? []);
  // 0.6.0 UX pass — workflow-level launch variables (mirrors QP variables).
  // When the user clicks "Lancer" with trigger=Manual + non-empty list,
  // a form asks for one value per variable before the run starts.
  const [wfVariables, setWfVariables] = useState<PromptVariable[]>(editWorkflow?.variables ?? []);

  // 0.8.2 — Deep-link from the audit-validation CTA: if `initialPresetId`
  // is supplied (and we're not editing an existing workflow), apply the
  // matching preset on mount so the user lands on a ready-to-save wizard
  // instead of an empty form. We also:
  //   1. Force `wizardMode='advanced'` + jump to the Steps page (step 2):
  //      simple mode hides multi-step pipelines, defeating the purpose of
  //      a 9-step AutoPilot deep-link.
  //   2. Suppress the auto-shown "Suggestions for this project" panel —
  //      the user came here for THIS preset, not for orthogonal MCP-aware
  //      suggestions.
  //   3. Transform the preset's first step from JsonData fixture to a real
  //      ApiCall when a tracker MCP (GitHub / GitLab / Jira) is wired,
  //      pointing at the oldest open issue so the user can test
  //      immediately. Mirrors the same heuristic as `isTrackerMcp` in
  //      `lib/constants.ts`.
  const presetAppliedRef = useRef(false);
  const [pluginsLoaded, setPluginsLoaded] = useState(false);
  useEffect(() => {
    if (presetAppliedRef.current || editWorkflow || !initialPresetId) return;
    // Wait for the plugins fetch to settle (one tick after mcpsApi.overview
    // resolves) so the tracker-MCP lookup is deterministic. Without this
    // guard, applying on mount races the plugins promise and the fetch_issue
    // transform silently no-ops on first render.
    if (!pluginsLoaded) return;

    const preset = buildV07Presets(t).find(p => p.id === initialPresetId);
    if (!preset) return;
    presetAppliedRef.current = true;

    // Pre-fill fetch_issue with the project's tracker MCP if available.
    // Precedence (most specific → most generic):
    //   1. `repo_url` hint  — github.com → mcp-github, gitlab.com → mcp-gitlab.
    //      This is the strongest signal: the user's project ACTUALLY lives
    //      on this host, so we pick the matching tracker even if Jira is
    //      also wired globally.
    //   2. Project-scoped plugin attachment (`!is_global` and project_ids
    //      includes this project).
    //   3. Global plugin (matches any project).
    // Without (1), a globally-wired Jira shadows a project-specific
    // GitHub config because both match the "actively wired" filter.
    const TRACKER_PLUGINS = ['mcp-github', 'mcp-gitlab', 'mcp-jira', 'mcp-atlassian'];
    const linkedProjectRepoUrl = initialProjectId
      ? projects.find(p => p.id === initialProjectId)?.repo_url ?? null
      : null;
    const repoHintedSlug = inferTrackerSlugFromRepoUrl(linkedProjectRepoUrl);

    // Both `ticket-to-pr` and 0.8.3's `feasibility-autopilot` share
    // the JsonData → ApiCall upgrade path: the first step seeds
    // ticket fixture data, and when a tracker plugin (Jira/GitHub/
    // GitLab) is wired for the project we swap it for a real fetch.
    // The transform body below assumes the step is named `fetch_issue`
    // in both presets — keep that naming consistent.
    const candidatePlugins = (preset.id === 'ticket-to-pr' || preset.id === 'feasibility-autopilot')
      ? availableApiPlugins.filter(p =>
          TRACKER_PLUGINS.includes(p.server.id)
          && (!initialProjectId
              || p.config.is_global
              || p.config.project_ids.includes(initialProjectId)),
        )
      : [];

    // Rank: (a) repo-hint match wins outright; otherwise prefer
    // project-scoped over global; ties broken by TRACKER_PLUGINS order.
    const trackerPlugin = candidatePlugins.length === 0 ? undefined :
      [...candidatePlugins].sort((a, b) => {
        const aHint = a.server.id === repoHintedSlug ? -100 : 0;
        const bHint = b.server.id === repoHintedSlug ? -100 : 0;
        if (aHint !== bHint) return aHint - bHint;
        const aScope = (initialProjectId && a.config.project_ids.includes(initialProjectId)) ? -10 : 0;
        const bScope = (initialProjectId && b.config.project_ids.includes(initialProjectId)) ? -10 : 0;
        if (aScope !== bScope) return aScope - bScope;
        return TRACKER_PLUGINS.indexOf(a.server.id) - TRACKER_PLUGINS.indexOf(b.server.id);
      })[0];

    const transformedSteps = preset.steps.map((s, idx) => {
      if (idx !== 0 || !trackerPlugin || s.name !== 'fetch_issue') return s;
      const slug = trackerPlugin.server.id;
      const linkedProject = initialProjectId
        ? projects.find(p => p.id === initialProjectId)
        : undefined;
      const parsed = parseRepoUrl(linkedProject?.repo_url);
      const req = buildOldestIssueRequest(slug, parsed);
      if (!req) return s;
      return {
        ...s,
        step_type: { type: 'ApiCall' } as WorkflowStep['step_type'],
        api_plugin_slug: slug,
        api_config_id: trackerPlugin.config.id,
        api_endpoint_path: req.endpoint,
        api_method: 'GET',
        api_query: req.query,
        api_path_params: req.path_params,
        api_extract: {
          path: req.extract_path,
          fallback: null,
          fail_on_empty: false,
        },
        // Drop the JsonData fixture body — leaving it around is harmless
        // (the executor ignores it for ApiCall steps) but confuses the UI.
        json_data_payload: null,
      };
    });

    setSteps(transformedSteps);
    if (preset.onFailure) setOnFailureSteps(preset.onFailure);
    if (preset.execAllowlist) setExecAllowlist(preset.execAllowlist);
    if (preset.variables) setWfVariables(preset.variables);
    if (initialProjectId && projects.some(p => p.id === initialProjectId)) {
      setProjectId(initialProjectId);
    }
    if (!name) setName(t(preset.titleKey));

    // Land the user on the Steps page in Advanced mode so the full
    // pipeline is visible.
    setWizardMode('advanced');
    setWizardStep(2);
    setShowSuggestions(false);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialPresetId, initialProjectId, pluginsLoaded]);
  // 0.7.0 Phase 5b — ref so the Exec form's "Configure now" button can
  // jump to the Config tab AND focus the allowlist input. Without focus
  // the user lands on a tab full of forms and has to hunt for "Allowlist
  // Exec" — defeats the purpose of the actionable warning.
  const execAllowlistInputRef = useRef<HTMLInputElement>(null);
  const goToAllowlistConfig = () => {
    setWizardStep(3); // Config tab in advanced mode
    // Wait one paint for the Config tab to mount before focusing.
    requestAnimationFrame(() => {
      execAllowlistInputRef.current?.focus();
      execAllowlistInputRef.current?.scrollIntoView({ block: 'center', behavior: 'smooth' });
    });
  };
  const [saving, setSaving] = useState(false);
  // Race-free guard, cf QuickPromptForm. Without this, a fast double-
  // click on Create/Save would `workflowsApi.create` (or `update`) twice.
  const savingRef = useRef(false);
  // 0.7+ — Save error surfacé en bandeau rouge sous le bouton Create.
  // Avant : seul `console.warn` capturait les rejets backend, l'utilisateur
  // cliquait sans rien voir = bug "dead button". Le validator backend rend
  // souvent des messages déjà actionnables (ex: "L'étape X est en
  // FreeText, passe-la en Structured"), inutile de les masquer.
  const [saveError, setSaveError] = useState<string | null>(null);
  const [availableSkills, setAvailableSkills] = useState<Skill[]>([]);
  const [availableProfiles, setAvailableProfiles] = useState<AgentProfile[]>([]);
  const [availableDirectives, setAvailableDirectives] = useState<Directive[]>([]);
  const [availableQuickPrompts, setAvailableQuickPrompts] = useState<QuickPrompt[]>([]);
  const [availableQuickApis, setAvailableQuickApis] = useState<QuickApi[]>([]);
  // API plugins available on the project — filtered to those with `api_spec != null`
  // and a matching config. Consumed by ApiCallStepCard's plugin picker.
  const [availableApiPlugins, setAvailableApiPlugins] = useState<ApiPluginOption[]>([]);
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

  /** B3 (0.7.0 UX pass) — append a multi-line instruction block at the
   *  end of a prompt (rather than at cursor). Used by the "écrire STATE"
   *  chip: the instruction belongs at the *end* of the agent's prompt
   *  ("À la fin, écris ---STATE:...---"), not wherever the user happens
   *  to be typing. Adds a leading blank line so the instruction stands
   *  out from whatever the user already wrote. */
  const appendPromptBlock = (stepIndex: number, block: string) => {
    const cur = steps[stepIndex]?.prompt_template ?? '';
    const sep = cur.endsWith('\n\n') ? '' : (cur.endsWith('\n') || cur === '') ? '\n' : '\n\n';
    updateStep(stepIndex, { prompt_template: cur + sep + block });
    // Focus the textarea at the end so the user sees what was inserted.
    requestAnimationFrame(() => {
      const el = promptTextareaRefs.current[stepIndex];
      if (!el) return;
      el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
      el.scrollTop = el.scrollHeight;
    });
  };

  // Workflow suggestions from MCP introspection
  const [suggestions, setSuggestions] = useState<WorkflowSuggestion[]>([]);
  const [showSuggestions, setShowSuggestions] = useState(false);
  const [suggestionsLoading, setSuggestionsLoading] = useState(false);

  useEffect(() => {
    const refetchProfiles = () => profilesApi.list()
      .then(setAvailableProfiles)
      .catch(e => console.warn('Failed to load profiles:', e));
    skillsApi.list().then(setAvailableSkills).catch(e => console.warn('Failed to load skills:', e));
    refetchProfiles();
    directivesApi.list().then(setAvailableDirectives).catch(e => console.warn('Failed to load directives:', e));
    quickPromptsApi.list().then(setAvailableQuickPrompts).catch(e => console.warn('Failed to load quick prompts:', e));
    quickApisApi.list().then(setAvailableQuickApis).catch(e => console.warn('Failed to load quick apis:', e));
    // Load API plugins once at mount — the list is refreshed if the user
    // comes back to the wizard, cheap call + rare delta.
    mcpsApi.overview()
      .then(overview => {
        const options: ApiPluginOption[] = [];
        for (const config of overview.configs) {
          const server = overview.servers.find(s => s.id === config.server_id);
          if (!server || !server.api_spec) continue;
          // Project scope: the wizard's `projectId` may change, so we
          // keep all configs and filter per-step-render if needed. For
          // MVP, include global configs + configs bound to any project.
          options.push({ server, config });
        }
        setAvailableApiPlugins(options);
      })
      .catch(e => console.warn('Failed to load API plugins:', e))
      .finally(() => setPluginsLoaded(true));
    window.addEventListener('kronn:profiles-changed', refetchProfiles);
    return () => window.removeEventListener('kronn:profiles-changed', refetchProfiles);
  }, []);

  // Fetch suggestions when project changes
  useEffect(() => {
    if (!projectId) { setSuggestions([]); return; }
    setSuggestionsLoading(true);
    workflowsApi.suggestions(projectId)
      .then(s => {
        setSuggestions(s);
        // Auto-show suggestions UNLESS we're applying a deep-linked preset
        // — in that case the user came here for a specific preset, not for
        // orthogonal MCP-aware suggestions. Showing both clutters the view.
        if (s.length > 0 && !isEdit && !initialPresetId) setShowSuggestions(true);
      })
      .catch(() => setSuggestions([]))
      .finally(() => setSuggestionsLoading(false));
  }, [projectId, isEdit, initialPresetId]);

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

  /** Load a starter template (désagentification aha moment). Picks the
   *  first matching config_id from `availableApiPlugins` for the template's
   *  primary plugin — user can still re-select in the wizard after. */
  const applyStarterTemplate = (templateId: string) => {
    const template = STARTER_TEMPLATES.find(t => t.id === templateId);
    if (!template) return;
    const match = availableApiPlugins.find(p => p.server.id === template.primary_plugin_slug);
    const steps = cloneTemplateSteps(template, match?.config.id ?? null);
    setName(template.title_fr);
    setSteps(steps);
    setTriggerType('Manual');
    setShowSuggestions(false);
    setWizardMode('advanced');
    setWizardStep(2); // Jump straight to Steps so the user sees the chain.
  };

  /** Build a fresh blank step. Centralised so `addStep` (append) and
   *  `insertStep` (insert-at-position) share the same defaults. */
  const blankStep = (existingCount: number): WorkflowStep => ({
    name: `step-${existingCount + 1}`,
    agent: 'ClaudeCode',
    prompt_template: '',
    mode: { type: 'Normal' },
    // Default to Structured so chained steps can read `.data` / `.summary`
    // from the very first save. Users can switch to Free text on terminal
    // steps that don't feed anything downstream.
    output_format: { type: 'Structured' },
  });

  const addStep = () => setSteps([...steps, blankStep(steps.length)]);

  /** 0.6.0 UX pass — insert a blank step at `at`. The user can then drop
   *  a step before the first one (`at = 0`), between two existing steps,
   *  or after the last (`at = steps.length`, equivalent to addStep).
   *  Subsequent step `Goto` references stay valid because they're keyed
   *  by step name, not index. */
  const insertStep = (at: number) => {
    const fresh = blankStep(steps.length);
    setSteps([...steps.slice(0, at), fresh, ...steps.slice(at)]);
  };

  /** 0.6.0 UX pass — move a step up or down by 1 position. No-op at the
   *  edges. Same `Goto`-by-name immunity as insertStep. */
  const moveStep = (idx: number, direction: -1 | 1) => {
    const target = idx + direction;
    if (target < 0 || target >= steps.length) return;
    setSteps(prev => {
      const copy = [...prev];
      [copy[idx], copy[target]] = [copy[target], copy[idx]];
      return copy;
    });
  };

  const updateStep = (idx: number, patch: Partial<WorkflowStep>) => {
    setSteps(steps.map((s, i) => i === idx ? { ...s, ...patch } : s));
  };

  /** Switch a step to a new step_type, clearing fields that don't apply
   *  to the new type. Pre-fix the step-type buttons just merged the new
   *  step_type into the existing step, leaving Agent fields (prompt
   *  template, profile, skills, model tier) sitting next to the new
   *  type's fields — the wizard rendered both at once, which Tya's
   *  audit on 2026-05-09 flagged as "biggest UX hole". The user is
   *  also warned via `confirm()` if they're about to lose configured
   *  Agent state (prompt template length > 0 OR a Quick Prompt is
   *  bound). For everything else (empty steps, switching JSON ↔ API),
   *  the swap is silent. Returns true if the swap happened. */
  const swapStepType = (idx: number, newType: string): boolean => {
    const current = steps[idx];
    if (!current) return false;
    const currentType = current.step_type?.type ?? 'Agent';
    if (currentType === newType) return false;

    // Detect "user has invested in this step's content" — show a confirm.
    const hasAgentContent =
      currentType === 'Agent' &&
      ((current.prompt_template?.trim().length ?? 0) > 0
        || !!current.quick_prompt_id
        || (current.skill_ids?.length ?? 0) > 0
        || (current.profile_ids?.length ?? 0) > 0);
    const hasApiContent =
      (currentType === 'ApiCall' || currentType === 'BatchApiCall') &&
      (!!current.api_plugin_slug || !!current.api_endpoint_path);
    // JsonData / Notify / Gate / Exec all live on the StepType variant
    // itself rather than on flat WorkflowStep fields, so swapping out
    // the step_type effectively wipes their content. We treat the type
    // change as "always destructive" for those, but only prompt when
    // the variant has been customized — for now we conservatively prompt
    // whenever the user is leaving JsonData for a non-JsonData type.
    const hasJsonContent = currentType === 'JsonData';

    if (hasAgentContent || hasApiContent || hasJsonContent) {
      // `confirm` may not exist in test environments (happy-dom doesn't
      // ship it). When unavailable we fall through to the swap — the
      // E2E layer covers the user-facing dialog separately.
      if (typeof confirm === 'function') {
        const ok = confirm(t('wiz.stepSwapConfirm', currentType, newType));
        if (!ok) return false;
      }
    }

    setSteps(prev => prev.map((s, i) => {
      if (i !== idx) return s;
      // Preserve only the fields that are universal across step types:
      // name, description, conditions/branching, on_failure, retries,
      // stall timeout. Drop everything else so the new type starts clean.
      const universal: Partial<WorkflowStep> = {
        name: s.name,
        description: s.description,
        on_result: s.on_result,
        retry: s.retry,
        stall_timeout_secs: s.stall_timeout_secs,
        delay_after_secs: s.delay_after_secs,
        mode: s.mode,
        step_type: { type: newType } as WorkflowStep['step_type'],
      };
      // Keep `agent` field present (the type allows AgentType only) — the
      // backend ignores it for non-Agent steps but the model field is
      // non-nullable. Mirror the default the form uses on new steps.
      return { ...universal, agent: s.agent } as WorkflowStep;
    }));
    return true;
  };

  // Switch a step to BatchQuickPrompt and pre-configure the upstream step
  // to actually feed it: Structured output (so `batch_items_from` can read
  // `{{steps.X.data}}`) + reasoning_effort=low (so the producer doesn't
  // burn tokens on narration the batch never reads).
  // Existing user overrides on `reasoning_effort` are preserved.
  const selectBatchQpStepType = (idx: number) => {
    // Use the shared `swapStepType` first to clear non-shared Agent/ApiCall
    // fields cleanly (and prompt for confirm if user content would be lost).
    // If the swap was rejected (user clicked Cancel), bail without patching
    // the predecessor.
    const swapped = swapStepType(idx, 'BatchQuickPrompt');
    if (!swapped) return;
    // Now patch the immediate predecessor to feed the batch — Structured
    // output (so `batch_items_from` can read `{{steps.X.data}}`) +
    // reasoning_effort=low (so the producer doesn't burn tokens on
    // narration the batch never reads). Existing user overrides preserved.
    setSteps(prev => prev.map((s, i) => {
      if (i === idx - 1 && s.step_type?.type !== 'BatchQuickPrompt' && s.step_type?.type !== 'Notify') {
        const needsStructured = !s.output_format || s.output_format.type !== 'Structured';
        const needsLowEffort = !s.agent_settings?.reasoning_effort;
        if (!needsStructured && !needsLowEffort) return s;
        return {
          ...s,
          output_format: needsStructured ? { type: 'Structured' } : s.output_format,
          agent_settings: needsLowEffort
            ? { ...s.agent_settings, reasoning_effort: 'low' }
            : s.agent_settings,
        };
      }
      return s;
    }));
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
    if (savingRef.current) return;
    savingRef.current = true;
    setSaving(true);
    setSaveError(null);
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
          guards,
          on_failure: onFailureSteps,
          exec_allowlist: execAllowlist,
          variables: wfVariables,
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
          guards: guards ?? undefined,
          on_failure: onFailureSteps,
          exec_allowlist: execAllowlist,
          variables: wfVariables,
        };
        await workflowsApi.create(req);
      }
      onDone();
    } catch (e) {
      console.warn('Workflow action failed:', e);
      setSaveError(userError(e, t('wiz.saveError')));
    } finally {
      savingRef.current = false;
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

      {/* Starter templates — désagentification aha moment. Only shown on
          fresh creation (not edit) and only if at least one template's
          primary plugin is configured on the project (otherwise the clone
          would leave `api_config_id: null` and the step can't run). */}
      {!isEdit && STARTER_TEMPLATES.some(t => availableApiPlugins.some(p => p.server.id === t.primary_plugin_slug)) && (
        <div className="wf-starter-templates">
          {STARTER_TEMPLATES.filter(t => availableApiPlugins.some(p => p.server.id === t.primary_plugin_slug)).map(tpl => (
            <button
              key={tpl.id}
              type="button"
              className="wf-starter-template-btn"
              onClick={() => applyStarterTemplate(tpl.id)}
              title={tpl.description_fr}
            >
              <Sparkles size={12} /> {tpl.title_fr}
            </button>
          ))}
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
            <span className="text-xs" style={{ color: i <= wizardStep ? 'var(--kr-accent-ink)' : 'var(--kr-text-dim)' }}>
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
          <label className="wf-label">
            {t('wiz.name')} <span className="wf-required-marker" aria-hidden="true">*</span>
          </label>
          <input
            className={`wf-input${!name ? ' wf-input-required-empty' : ''}`}
            value={name}
            onChange={e => setName(e.target.value)}
            placeholder={t('wiz.namePlaceholder')}
            aria-required="true"
            aria-invalid={!name}
            aria-label={t('wiz.name')}
          />
          {!name && (
            <p className="wf-field-hint wf-field-hint-required">
              {t('wiz.nameRequired')}
            </p>
          )}

          <label className="wf-label mt-6">{isEdit ? t('wiz.project') : t('wiz.projectOptional')}</label>
          <select className="wf-select" value={projectId} aria-label={t('wiz.project')} onChange={e => {
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
            aria-label={t('wiz.agentLabel')}
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
            aria-label={t('wiz.promptLabel')}
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
          {/* B4 (0.7.0 UX pass) — Bandeau "Démarrer depuis un pattern".
              Visible uniquement à la création (pas en édition) ET tant
              que la config est restée vierge (1 step nommé "main" avec
              prompt vide). Dès que l'user customise, le bandeau s'efface
              tout seul — pas besoin d'un bouton dismiss explicite.
              Click sur une carte = applique steps + on_failure +
              exec_allowlist du préset. Pédagogique : le user voit
              comment c'est construit (pas de magie cachée). */}
          {!isEdit && steps.length === 1 && steps[0].name === 'main' && steps[0].prompt_template === '' && (
            <div className="wf-presets-bandeau mb-6">
              <div className="flex-row gap-3 mb-3 items-center">
                <Sparkles size={14} className="text-accent" />
                <span className="text-sm font-semibold text-secondary">{t('wiz.presetsTitle')}</span>
                <span className="text-xs text-ghost">{t('wiz.presetsHint')}</span>
              </div>
              <div className="wf-presets-grid">
                {buildV07Presets(t).map(preset => (
                  <button
                    key={preset.id}
                    type="button"
                    className="wf-preset-card"
                    onClick={() => {
                      setSteps(preset.steps);
                      if (preset.onFailure) setOnFailureSteps(preset.onFailure);
                      if (preset.execAllowlist) setExecAllowlist(preset.execAllowlist);
                      if (preset.variables) setWfVariables(preset.variables);
                    }}
                  >
                    <div className="wf-preset-icon">{preset.icon}</div>
                    <div className="wf-preset-title">{t(preset.titleKey)}</div>
                    <div className="wf-preset-desc">{t(preset.descKey)}</div>
                    <div className="wf-preset-primitives">
                      {preset.primitives.map(p => (
                        <span key={p} className="wf-preset-chip">{p}</span>
                      ))}
                    </div>
                  </button>
                ))}
              </div>
              <p className="text-2xs text-ghost mt-3">{t('wiz.presetsBlankHint')}</p>
            </div>
          )}

          {/* Worktree isolation hint — visible whenever a project is bound.
              Each WorkflowRun creates its own git worktree at
              `.kronn/worktrees/<name>-<run_id>` and ALL steps share that
              `work_dir`. Without this note the user wonders whether
              consecutive Agent → Exec → Agent steps see the same files. */}
          {projectId && (
            <div className="wf-worktree-hint mb-5">
              <span title={t('wiz.worktreeIconTooltip')} className="flex-row" style={{ cursor: 'help' }}>
                <GitBranch size={11} />
              </span>
              <span>{t('wiz.worktreeHint')}</span>
            </div>
          )}

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
                  {([
                    ['{{issue.title}}', t('wiz.issueTitle')],
                    ['{{issue.body}}', t('wiz.issueBody')],
                    ['{{issue.number}}', t('wiz.issueNumber')],
                    ['{{issue.url}}', t('wiz.issueUrl')],
                    ['{{issue.labels}}', t('wiz.issueLabels')],
                  ] as Array<[string, string]>).map(([v, d]) => (
                    <div key={v} className="wf-help-row">
                      <code
                        className="wf-help-code"
                        onClick={() => navigator.clipboard.writeText(v)}
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
                  {([
                    ['{{previous_step.output}}', t('wiz.prevOutput')],
                    ['{{steps.<nom>.output}}', t('wiz.namedOutput')],
                    // 0.7.0 Phase 6 — iteration counter & durable state.
                    ['{{iter.<step>}}', t('wiz.iterHint')],
                    ['{{state.<key>}}', t('wiz.stateHint')],
                  ] as Array<[string, string]>).map(([v, d]) => (
                    <div key={v} className="wf-help-row">
                      <code
                        className="wf-help-code"
                        onClick={() => navigator.clipboard.writeText(v)}
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
              <div key={i}>
                {/* 0.6.0 UX pass — Insert-here divider.
                    Une fine ligne avec un bouton "+" qui apparaît au hover
                    permettant d'insérer un step à cette position. Couvre :
                      - Avant le 1er step (i === 0)
                      - Entre 2 steps existants (i > 0)
                    Le bouton final ("Add at end") reste sous la liste,
                    inchangé. Pattern similaire au "+ Add cell" de Notion. */}
                <div className="wf-step-insert-divider">
                  <button
                    type="button"
                    className="wf-step-insert-btn"
                    onClick={() => insertStep(i)}
                    title={t('wiz.insertStepHere')}
                    aria-label={t('wiz.insertStepHere')}
                  >
                    <Plus size={11} />
                    <span>{t('wiz.insertStepHere')}</span>
                  </button>
                </div>
              <div className="wf-step-edit-card mb-6">
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
                  {(!step.step_type || step.step_type.type === 'Agent') && (
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
                  )}
                  {/* 0.6.0 UX pass — reorder buttons. Marie + Antony :
                      "je peux pas déplacer mes steps". On expose 2 chevrons
                      à côté du remove. Désactivés aux bornes. Le Goto par
                      nom rend la réordonnance safe (les conditions ne
                      pètent pas en swappant 2 steps). */}
                  {steps.length > 1 && (
                    <>
                      <button
                        className="wf-icon-btn"
                        onClick={() => moveStep(i, -1)}
                        disabled={i === 0}
                        aria-label={t('wiz.moveStepUp')}
                        title={t('wiz.moveStepUp')}
                      ><ChevronUp size={12} /></button>
                      <button
                        className="wf-icon-btn"
                        onClick={() => moveStep(i, 1)}
                        disabled={i === steps.length - 1}
                        aria-label={t('wiz.moveStepDown')}
                        title={t('wiz.moveStepDown')}
                      ><ChevronDown size={12} /></button>
                      <button className="wf-icon-btn" onClick={() => removeStep(i)} aria-label="Remove step">
                        <X size={12} />
                      </button>
                    </>
                  )}
                </div>
                {/* Step type — equal-width buttons with icon + short label.
                    0.6.0 UX pass : Marie flaggait des largeurs incohérentes
                    et des labels longs ("Agent (raisonnement)", "Récupérer
                    des données"). On garde un mot court par bouton, l'icône
                    porte la sémantique visuelle, le `title` (tooltip) garde
                    l'explication détaillée. La caption sous la rangée affiche
                    en clair le rôle du type sélectionné — plus besoin de
                    hover pour comprendre. */}
                <div className="wf-step-type-row mb-3">
                  <button
                    className="wf-step-type-btn"
                    data-type="agent"
                    data-selected={!step.step_type || step.step_type.type === 'Agent'}
                    onClick={() => swapStepType(i, 'Agent')}
                    title={t('wiz.stepTypeAgentHint')}
                  ><Bot size={11} /> {t('wiz.stepTypeAgent')}</button>
                  <button
                    className="wf-step-type-btn"
                    data-type="api"
                    data-selected={step.step_type?.type === 'ApiCall'}
                    onClick={() => swapStepType(i, 'ApiCall')}
                    title={t('wiz.stepTypeApiCallHint')}
                  ><Plug size={11} /> {t('wiz.stepTypeApiCall')}</button>
                  <button
                    className="wf-step-type-btn"
                    data-type="batch-qp"
                    data-selected={step.step_type?.type === 'BatchQuickPrompt'}
                    onClick={() => selectBatchQpStepType(i)}
                    title={t('wiz.stepTypeBatchQPHint')}
                  ><Layers size={11} /> {t('wiz.stepTypeBatchQP')}</button>
                  <button
                    className="wf-step-type-btn"
                    data-type="notify"
                    data-selected={step.step_type?.type === 'Notify'}
                    onClick={() => swapStepType(i, 'Notify')}
                    title={t('wiz.stepTypeNotifyHint')}
                  ><Send size={11} /> {t('wiz.stepTypeNotify')}</button>
                  <button
                    className="wf-step-type-btn"
                    data-type="gate"
                    data-selected={step.step_type?.type === 'Gate'}
                    onClick={() => swapStepType(i, 'Gate')}
                    title={t('wiz.stepTypeGateHint')}
                  ><Hand size={11} /> {t('wiz.stepTypeGate')}</button>
                  <button
                    className="wf-step-type-btn"
                    data-type="exec"
                    data-selected={step.step_type?.type === 'Exec'}
                    onClick={() => swapStepType(i, 'Exec')}
                    title={t('wiz.stepTypeExecHint')}
                  ><Terminal size={11} /> {t('wiz.stepTypeExec')}</button>
                  <button
                    className="wf-step-type-btn"
                    data-type="batch-api"
                    data-selected={step.step_type?.type === 'BatchApiCall'}
                    onClick={() => swapStepType(i, 'BatchApiCall')}
                    title={t('wiz.stepTypeBatchApiHint')}
                  ><Layers size={11} /> {t('wiz.stepTypeBatchApi')}</button>
                  <button
                    className="wf-step-type-btn"
                    data-type="json-data"
                    data-selected={step.step_type?.type === 'JsonData'}
                    onClick={() => swapStepType(i, 'JsonData')}
                    title={t('wiz.stepTypeJsonDataHint')}
                  ><Braces size={11} /> {t('wiz.stepTypeJsonData')}</button>
                </div>
                {/* Caption sous la rangée — résume en 1 phrase ce que fait
                    le type sélectionné. Évite au user de devoir hover pour
                    comprendre la diff Agent / API / Batch / Notify / Gate / Exec. */}
                <div className="wf-step-type-caption mb-3">
                  {(() => {
                    const k = step.step_type?.type ?? 'Agent';
                    const captionKey =
                      k === 'ApiCall' ? 'wiz.stepTypeApiCallHint' :
                      k === 'BatchQuickPrompt' ? 'wiz.stepTypeBatchQPHint' :
                      k === 'Notify' ? 'wiz.stepTypeNotifyHint' :
                      k === 'Gate' ? 'wiz.stepTypeGateHint' :
                      k === 'Exec' ? 'wiz.stepTypeExecHint' :
                      k === 'BatchApiCall' ? 'wiz.stepTypeBatchApiHint' :
                      'wiz.stepTypeAgentHint';
                    return t(captionKey);
                  })()}
                </div>
                <div className="flex-row gap-4 mb-3">
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

                    {!isFirstStep && (() => {
                      // Surface the auto-config applied to the upstream step
                      // so the user understands why its output_format /
                      // reasoning_effort changed when they selected BatchQP.
                      const prev = steps[i - 1];
                      const prevIsProducer = prev.step_type?.type !== 'BatchQuickPrompt' && prev.step_type?.type !== 'Notify';
                      const prevStructured = prev.output_format?.type === 'Structured';
                      const prevLowEffort = prev.agent_settings?.reasoning_effort === 'low';
                      if (!prevIsProducer || (!prevStructured && !prevLowEffort)) return null;
                      return (
                        <div className="wf-batch-info mt-2">
                          <Info size={12} />
                          <span>
                            {t('wiz.batchAutoPrevNotice').replace('{name}', prev.name)}
                            {prevStructured && prevLowEffort ? ' (Structured + reasoning_effort=low)'
                              : prevStructured ? ' (Structured)'
                              : ' (reasoning_effort=low)'}
                          </span>
                        </div>
                      );
                    })()}

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
                        {/* QP Chain (Phase 2) — run N more QPs sequentially in each child */}
                        {(() => {
                          const chain = step.batch_chain_prompt_ids ?? [];
                          // Chain candidates: at most 1 variable. The backend
                          // substitutes the batch item value (e.g. "EW-1234")
                          // into that first var when the chain QP fires —
                          // same mechanism as the primary QP. QPs with 2+ vars
                          // are excluded because we only have one value per
                          // batch item to substitute.
                          const chainCandidates = availableQuickPrompts.filter(
                            qp => qp.variables.length <= 1 && qp.id !== step.batch_quick_prompt_id && !chain.includes(qp.id),
                          );
                          return (
                            <div className="mt-3">
                              <label className="text-xs text-muted">{t('wiz.batchChain')}</label>
                              <p className="text-xs text-ghost mb-2">{t('wiz.batchChainHint')}</p>
                              {chain.length > 0 && (
                                <div
                                  className="flex-row flex-wrap gap-2 mb-2"
                                  role="list"
                                  aria-label={t('wiz.batchChain')}
                                >
                                  {chain.map((qpId, chainIdx) => {
                                    const qp = availableQuickPrompts.find(q => q.id === qpId);
                                    const label = qp ? `${qp.icon} ${qp.name}` : `⚠️ ${qpId}`;
                                    const reorder = (from: number, to: number) => {
                                      if (to < 0 || to >= chain.length || to === from) return;
                                      const next = [...chain];
                                      const [moved] = next.splice(from, 1);
                                      next.splice(to, 0, moved);
                                      updateStep(i, { batch_chain_prompt_ids: next });
                                    };
                                    return (
                                      // Native HTML5 DnD instead of pulling in
                                      // `@dnd-kit` — the list is short (a handful
                                      // of pills) and we already needed keyboard
                                      // ↑/↓ buttons for a11y, so the DnD wrapper
                                      // is just a quality-of-life shortcut on top.
                                      <span
                                        key={`${qpId}-${chainIdx}`}
                                        className="wf-chain-pill"
                                        role="listitem"
                                        draggable
                                        data-dragging-idx={chainIdx}
                                        onDragStart={e => {
                                          e.dataTransfer.setData('text/plain', String(chainIdx));
                                          e.dataTransfer.effectAllowed = 'move';
                                        }}
                                        onDragOver={e => {
                                          // Required to make a drop target — without
                                          // `preventDefault` the browser refuses the
                                          // drop event.
                                          e.preventDefault();
                                          e.dataTransfer.dropEffect = 'move';
                                        }}
                                        onDrop={e => {
                                          e.preventDefault();
                                          const from = Number(e.dataTransfer.getData('text/plain'));
                                          if (Number.isFinite(from)) reorder(from, chainIdx);
                                        }}
                                      >
                                        <span className="wf-chain-pos">{chainIdx + 1}.</span>
                                        <span>{label}</span>
                                        <button
                                          type="button"
                                          className="wf-chain-pill-arrow"
                                          title={t('wiz.batchChainMoveUp')}
                                          aria-label={t('wiz.batchChainMoveUp')}
                                          disabled={chainIdx === 0}
                                          onClick={() => reorder(chainIdx, chainIdx - 1)}
                                        >
                                          <ChevronUp size={10} />
                                        </button>
                                        <button
                                          type="button"
                                          className="wf-chain-pill-arrow"
                                          title={t('wiz.batchChainMoveDown')}
                                          aria-label={t('wiz.batchChainMoveDown')}
                                          disabled={chainIdx === chain.length - 1}
                                          onClick={() => reorder(chainIdx, chainIdx + 1)}
                                        >
                                          <ChevronDown size={10} />
                                        </button>
                                        <button
                                          type="button"
                                          className="wf-chain-pill-remove"
                                          title={t('wiz.batchChainRemove')}
                                          aria-label={t('wiz.batchChainRemove')}
                                          onClick={() => {
                                            const next = [...chain];
                                            next.splice(chainIdx, 1);
                                            updateStep(i, { batch_chain_prompt_ids: next });
                                          }}
                                        >
                                          <X size={10} />
                                        </button>
                                      </span>
                                    );
                                  })}
                                </div>
                              )}
                              {chainCandidates.length > 0 ? (
                                <select
                                  className="wf-select text-sm"
                                  value=""
                                  onChange={e => {
                                    if (!e.target.value) return;
                                    updateStep(i, { batch_chain_prompt_ids: [...chain, e.target.value] });
                                  }}
                                >
                                  <option value="">{t('wiz.batchChainAdd')}</option>
                                  {chainCandidates.map(qp => (
                                    <option key={qp.id} value={qp.id}>
                                      {qp.icon} {qp.name}
                                    </option>
                                  ))}
                                </select>
                              ) : (
                                <p className="text-xs text-ghost">{t('wiz.batchChainEmpty')}</p>
                              )}
                            </div>
                          );
                        })()}
                      </>
                    )}
                  </div>
                  );
                })() : step.step_type?.type === 'ApiCall' ? (
                  <ApiCallStepCard
                    step={step}
                    onChange={updates => updateStep(i, updates)}
                    availableApiPlugins={availableApiPlugins}
                    projectId={projectId || null}
                    nextStepType={steps[i + 1]?.step_type}
                    installedAgents={installedAgentTypes}
                    configLanguage={configLanguage}
                    availableQuickApis={availableQuickApis}
                    t={t}
                  />
                ) : step.step_type?.type === 'Notify' ? (
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
                ) : step.step_type?.type === 'Gate' ? (
                  <div className="wf-gate-form">
                    <div className="wf-batch-intro">
                      <Hand size={14} />
                      <div>
                        <strong>{t('wiz.gateTitle')}</strong>
                        <p className="text-xs text-muted">{t('wiz.gateHint')}</p>
                      </div>
                    </div>
                    <label className="text-xs text-muted mb-1">{t('wiz.gateMessage')}</label>
                    <textarea
                      className="wf-textarea text-sm mb-2"
                      rows={4}
                      value={step.gate_message ?? ''}
                      onChange={e => updateStep(i, { gate_message: e.target.value || null })}
                      placeholder={t('wiz.gateMessagePlaceholder')}
                    />
                    {i > 0 && (
                      <div className="mt-1 mb-2 text-xs text-ghost flex-wrap flex-row gap-1">
                        <span>{t('wiz.clickToInsert')} :</span>
                        <code className="wf-var-hint-code" style={{ cursor: 'default' }}>{'{{previous_step.output}}'}</code>
                        <code className="wf-var-hint-code" style={{ cursor: 'default' }}>{'{{previous_step.summary}}'}</code>
                        {steps.slice(0, i).map(prev => (
                          <code key={prev.name} className="wf-var-hint-code" style={{ cursor: 'default' }}>
                            {`{{steps.${prev.name}.summary}}`}
                          </code>
                        ))}
                      </div>
                    )}
                    <label className="text-xs text-muted mb-1">{t('wiz.gateRequestChangesTarget')}</label>
                    <select
                      className="wf-select text-sm mb-2"
                      value={step.gate_request_changes_target ?? ''}
                      onChange={e => updateStep(i, { gate_request_changes_target: e.target.value || null })}
                    >
                      <option value="">{t('wiz.gateRequestChangesDefault')}</option>
                      {steps.map((s, j) => j !== i && (
                        <option key={s.name} value={s.name}>{s.name}</option>
                      ))}
                    </select>
                    {/* P1-1 (0.7.0 UX pass) — optional webhook to ping
                        ops on Gate fire. Cyndie's blocker for team
                        deployment. Templated so {{state.slack_url}}
                        type vars work. Empty = no notification. */}
                    <label className="text-xs text-muted mb-1">{t('wiz.gateNotifyUrl')}</label>
                    <input
                      className="wf-input text-sm"
                      value={step.gate_notify_url ?? ''}
                      onChange={e => updateStep(i, { gate_notify_url: e.target.value || null })}
                      placeholder={t('wiz.gateNotifyUrlPlaceholder')}
                    />
                    <p className="text-2xs text-ghost mt-1">{t('wiz.gateNotifyUrlHint')}</p>
                  </div>
                ) : step.step_type?.type === 'Exec' ? (
                  <div className="wf-exec-form">
                    <div className="wf-batch-intro">
                      <Terminal size={14} />
                      <div>
                        <strong>{t('wiz.execTitle')}</strong>
                        <p className="text-xs text-muted">{t('wiz.execHint')}</p>
                      </div>
                    </div>
                    {/* Worktree affordance — surface here because "is this exec
                        seeing what the previous Agent step wrote?" is the most
                        common UX worry on the Exec step. 0.8.2 TD #232 — show
                        for ALL exec steps (not just `i > 0`): the first step
                        case needs a different message ("fresh worktree, no
                        prior changes") so users on a single-step workflow
                        also see where their command runs. When no project is
                        bound at all, warn that the command runs in Kronn's
                        CWD (no worktree, no isolation) — common foot-gun. */}
                    {projectId ? (
                      <div className="wf-worktree-hint mb-3">
                        <span title={t('wiz.worktreeIconTooltip')} className="flex-row" style={{ cursor: 'help' }}>
                          <GitBranch size={11} />
                        </span>
                        <span>{i > 0 ? t('wiz.execWorktreeHint') : t('wiz.execWorktreeHintFirst')}</span>
                      </div>
                    ) : (
                      <div className="wf-restricted-warning mb-3">
                        <AlertTriangle size={12} />
                        <span className="flex-1">{t('wiz.execNoProjectWarn')}</span>
                      </div>
                    )}
                    {execAllowlist.length === 0 ? (
                      <div className="wf-restricted-warning">
                        <AlertTriangle size={12} />
                        <span className="flex-1">{t('wiz.execAllowlistEmpty')}</span>
                        <button
                          type="button"
                          className="wf-allowlist-cta"
                          onClick={goToAllowlistConfig}
                        >
                          {t('wiz.execAllowlistConfigureNow')}
                        </button>
                      </div>
                    ) : (
                      <>
                        {/* 0.8.2 — Setup phase (worktree dep install). Hidden
                            behind a checkbox to keep the simple case clean.
                            Toggling ON enables the setup-command picker + a
                            preset dropdown for the common install commands.
                            See backend `exec_setup_command` / `exec_setup_args`
                            and `execute_exec_step` for the runtime contract. */}
                        <div className="mb-3" style={{ borderLeft: '2px solid var(--kr-border-soft)', paddingLeft: 8 }}>
                          <label className="text-xs flex-row gap-1" style={{ cursor: 'pointer', alignItems: 'flex-start' }}>
                            <input
                              type="checkbox"
                              checked={!!step.exec_setup_command}
                              onChange={e => {
                                if (e.target.checked) {
                                  // Default to composer install — the most
                                  // common case for the AutoPilot preset on
                                  // PHP projects. User can swap via the
                                  // preset dropdown below.
                                  updateStep(i, {
                                    exec_setup_command: 'composer',
                                    exec_setup_args: ['install', '--no-interaction', '--prefer-dist'],
                                  });
                                } else {
                                  updateStep(i, { exec_setup_command: null, exec_setup_args: [] });
                                }
                              }}
                              style={{ marginTop: 2 }}
                            />
                            <span style={{ flex: 1 }}>
                              <strong>{t('wiz.execSetupToggle')}</strong>
                              <br />
                              <span className="text-ghost">{t('wiz.execSetupHint')}</span>
                            </span>
                          </label>
                          {step.exec_setup_command && (
                            <div className="mt-2 ml-5">
                              <label className="text-xs text-muted mb-1">{t('wiz.execSetupPreset')}</label>
                              <select
                                className="wf-select text-sm mb-2"
                                value=""
                                onChange={e => {
                                  const v = e.target.value;
                                  if (!v) return;
                                  const presets: Record<string, { cmd: string; args: string[] }> = {
                                    composer:  { cmd: 'composer', args: ['install', '--no-interaction', '--prefer-dist'] },
                                    'npm-ci':  { cmd: 'npm',      args: ['ci'] },
                                    pnpm:      { cmd: 'pnpm',     args: ['install', '--frozen-lockfile'] },
                                    yarn:      { cmd: 'yarn',     args: ['install', '--frozen-lockfile'] },
                                    poetry:    { cmd: 'poetry',   args: ['install', '--no-interaction'] },
                                    pip:       { cmd: 'pip',      args: ['install', '-r', 'requirements.txt'] },
                                  };
                                  const p = presets[v];
                                  if (p) updateStep(i, { exec_setup_command: p.cmd, exec_setup_args: p.args });
                                }}
                              >
                                <option value="">— {t('wiz.execSetupPresetPick')} —</option>
                                <option value="composer">composer install (PHP)</option>
                                <option value="npm-ci">npm ci (Node + package-lock.json)</option>
                                <option value="pnpm">pnpm install (Node + pnpm-lock.yaml)</option>
                                <option value="yarn">yarn install (Node + yarn.lock)</option>
                                <option value="poetry">poetry install (Python + poetry.lock)</option>
                                <option value="pip">pip install -r requirements.txt (Python)</option>
                              </select>
                              <label className="text-xs text-muted mb-1">{t('wiz.execSetupCommand')}</label>
                              <select
                                className="wf-select text-sm mb-2"
                                value={step.exec_setup_command ?? ''}
                                onChange={e => updateStep(i, { exec_setup_command: e.target.value || null })}
                              >
                                <option value="">—</option>
                                {execAllowlist.map(bin => (
                                  <option key={bin} value={bin}>{bin}</option>
                                ))}
                              </select>
                              <label className="text-xs text-muted mb-1">{t('wiz.execSetupArgs')}</label>
                              <textarea
                                className="wf-textarea text-sm"
                                rows={2}
                                value={(step.exec_setup_args ?? []).join('\n')}
                                onChange={e => updateStep(i, { exec_setup_args: e.target.value.split('\n').filter(s => s.length > 0) })}
                                placeholder="install&#10;--no-interaction&#10;--prefer-dist"
                              />
                            </div>
                          )}
                        </div>
                        <label className="text-xs text-muted mb-1">{t('wiz.execCommand')} <span className="wf-required">*</span></label>
                        <select
                          className="wf-select text-sm mb-2"
                          value={step.exec_command ?? ''}
                          onChange={e => updateStep(i, { exec_command: e.target.value || null })}
                        >
                          <option value="">— {t('wiz.execCommandSelect')} —</option>
                          {execAllowlist.map(bin => (
                            <option key={bin} value={bin}>{bin}</option>
                          ))}
                        </select>
                        <label className="text-xs text-muted mb-1">{t('wiz.execArgs')}</label>
                        <textarea
                          className="wf-textarea text-sm mb-2"
                          rows={3}
                          value={(step.exec_args ?? []).join('\n')}
                          onChange={e => updateStep(i, { exec_args: e.target.value.split('\n').filter(s => s.length > 0) })}
                          placeholder={t('wiz.execArgsPlaceholder')}
                        />
                        <div className="mt-1 mb-2 text-xs text-ghost flex-wrap flex-row gap-1">
                          <span>{t('wiz.clickToInsert')} :</span>
                          {steps.slice(0, i).map(prev => (
                            <code key={prev.name} className="wf-var-hint-code" style={{ cursor: 'default' }}>
                              {`{{steps.${prev.name}.summary}}`}
                            </code>
                          ))}
                        </div>
                        <label className="text-xs text-muted mb-1">{t('wiz.execTimeoutSecs')}</label>
                        <input
                          type="number"
                          min={1}
                          max={1800}
                          className="wf-input text-sm"
                          style={{ width: 100 }}
                          value={step.exec_timeout_secs ?? ''}
                          onChange={e => updateStep(i, { exec_timeout_secs: e.target.value ? parseInt(e.target.value) : null })}
                          placeholder="300"
                        />
                      </>
                    )}
                  </div>
                ) : step.step_type?.type === 'BatchApiCall' ? (
                  <div className="wf-batch-api-form">
                    <div className="wf-batch-intro">
                      <Layers size={14} />
                      <div>
                        <strong>{t('wiz.batchApiTitle')}</strong>
                        <p className="text-xs text-muted">{t('wiz.batchApiHint')}</p>
                      </div>
                    </div>
                    {/* QuickApi reference — when set, the runtime loads the
                        saved Quick API and uses its config (per-field overrides
                        from the inline fields below still apply). Lets the
                        user define the call once, reuse across N workflows. */}
                    {availableQuickApis.length > 0 && (
                      <div className="mb-3">
                        <label className="text-xs text-muted mb-1">{t('wiz.batchApiQaPicker')}</label>
                        <select
                          className="wf-select text-sm"
                          value={step.quick_api_id ?? ''}
                          onChange={e => updateStep(i, { quick_api_id: e.target.value || null })}
                        >
                          <option value="">{t('wiz.batchApiQaPickerInline')}</option>
                          {availableQuickApis.map(qa => (
                            <option key={qa.id} value={qa.id}>
                              {qa.icon} {qa.name} — {qa.api_method ?? 'GET'} {qa.api_endpoint_path}
                            </option>
                          ))}
                        </select>
                        <p className="text-2xs text-ghost mt-1">{t('wiz.batchApiQaPickerHint')}</p>
                      </div>
                    )}
                    {/* Items source — same shape as BatchQP. Accepts JSON array
                        (preferred) or `{{steps.X.data}}` envelope. Per-item
                        templating exposes `{{batch.item.<field>}}`. */}
                    <label className="text-xs text-muted mb-1">{t('wiz.batchApiItemsFrom')} <span className="wf-required">*</span></label>
                    <input
                      className="wf-input text-sm mb-1"
                      value={step.batch_items_from ?? ''}
                      onChange={e => updateStep(i, { batch_items_from: e.target.value || null })}
                      placeholder={t('wiz.batchApiItemsFromPlaceholder')}
                    />
                    {i > 0 && (
                      <div className="mt-1 mb-3 text-xs text-ghost flex-wrap flex-row gap-1">
                        <span>{t('wiz.clickToInsert')} :</span>
                        {steps.slice(0, i).map(prev => (
                          <code key={prev.name} className="wf-var-hint-code" style={{ cursor: 'default' }}>
                            {`{{steps.${prev.name}.data}}`}
                          </code>
                        ))}
                      </div>
                    )}
                    <div className="flex-row gap-4 mb-3">
                      <div>
                        <label className="text-xs text-muted">{t('wiz.batchApiConcurrentLimit')}</label>
                        <input
                          type="number" min={1} max={20}
                          className="wf-input text-sm"
                          style={{ width: 80 }}
                          value={step.batch_concurrent_limit ?? ''}
                          onChange={e => updateStep(i, { batch_concurrent_limit: e.target.value ? parseInt(e.target.value) : null })}
                          placeholder="5"
                        />
                        <p className="text-2xs text-ghost mt-1">{t('wiz.batchApiConcurrentLimitHint')}</p>
                      </div>
                      <div>
                        <label className="text-xs text-muted">{t('wiz.batchApiMaxItems')}</label>
                        <input
                          type="number" min={1} max={500}
                          className="wf-input text-sm"
                          style={{ width: 80 }}
                          value={step.batch_max_items ?? ''}
                          onChange={e => updateStep(i, { batch_max_items: e.target.value ? parseInt(e.target.value) : null })}
                          placeholder="50"
                        />
                        <p className="text-2xs text-ghost mt-1">{t('wiz.batchApiMaxItemsHint')}</p>
                      </div>
                    </div>
                    {/* Reuse ApiCallStepCard for the request config. The
                        same plugin/endpoint/method/body/extract fields apply
                        — every child fan-out fires a clone of this request
                        with `{{batch.item.X}}` resolved per-item. */}
                    <ApiCallStepCard
                      step={step}
                      onChange={updates => updateStep(i, updates)}
                      availableApiPlugins={availableApiPlugins}
                      projectId={projectId || null}
                      installedAgents={installedAgentTypes}
                      configLanguage={configLanguage}
                      t={t}
                    />
                  </div>
                ) : step.step_type?.type === 'JsonData' ? (() => {
                  // Source de données déterministe — payload littéral, 0 token,
                  // 0 réseau. Valide le JSON live pour signaler les erreurs avant
                  // le save. Pas de templating runtime : la valeur est retournée
                  // telle quelle, ce qui permet à un BatchQuickPrompt aval de
                  // consommer `{{steps.<name>.data}}` exactement comme s'il
                  // venait d'une vraie API.
                  const raw = step.json_data_payload === null || step.json_data_payload === undefined
                    ? ''
                    : JSON.stringify(step.json_data_payload, null, 2);
                  let parseError: string | null = null;
                  let summary: string | null = null;
                  if (raw.trim()) {
                    try {
                      const parsed = JSON.parse(raw);
                      if (Array.isArray(parsed)) {
                        summary = t('wiz.jsonDataSummaryArray', parsed.length);
                      } else if (parsed && typeof parsed === 'object') {
                        summary = t('wiz.jsonDataSummaryObject', Object.keys(parsed).length);
                      } else {
                        summary = t('wiz.jsonDataSummaryScalar');
                      }
                    } catch (e) {
                      parseError = (e as Error).message;
                    }
                  }
                  return (
                    <div className="wf-json-data-form">
                      <div className="wf-batch-intro">
                        <Braces size={14} />
                        <div>
                          <strong>{t('wiz.jsonDataTitle')}</strong>
                          <p className="text-xs text-muted">{t('wiz.jsonDataHint')}</p>
                        </div>
                      </div>
                      <label className="text-xs text-muted mb-1">
                        {t('wiz.jsonDataPayload')} <span className="wf-required">*</span>
                      </label>
                      <textarea
                        className="wf-textarea text-sm"
                        rows={10}
                        style={{ fontFamily: 'var(--kr-font-mono)' }}
                        value={raw}
                        data-invalid={parseError !== null || !raw.trim()}
                        onChange={e => {
                          const next = e.target.value;
                          if (!next.trim()) {
                            updateStep(i, { json_data_payload: null });
                            return;
                          }
                          // On garde la string exacte si elle ne parse pas — on
                          // la stocke quand même via le state pour que le user
                          // ne perde pas son édition. Mais comme le model est
                          // `unknown | null`, on ne peut PAS y mettre une string
                          // brute (sinon downstream batch lit du non-JSON). Solution :
                          // on tente le parse, et on stocke null si échec — le
                          // textarea garde le texte brut côté DOM tant qu'il n'a
                          // pas perdu le focus.
                          try {
                            const parsed = JSON.parse(next);
                            updateStep(i, { json_data_payload: parsed });
                          } catch {
                            // Parse échoue → on tag invalide via data-invalid,
                            // mais on n'écrase pas json_data_payload. Le user
                            // doit corriger pour que la valeur soit consommée
                            // par le runner.
                          }
                        }}
                        placeholder={t('wiz.jsonDataPlaceholder')}
                      />
                      {parseError && (
                        <div className="wf-restricted-warning" style={{ marginTop: 6 }}>
                          <AlertTriangle size={12} />
                          <span>{t('wiz.jsonDataParseError')} {parseError}</span>
                        </div>
                      )}
                      {!parseError && summary && (
                        <p className="text-2xs text-ghost mt-1">{summary}</p>
                      )}
                      <p className="text-2xs text-ghost mt-2">{t('wiz.jsonDataNoTemplating')}</p>
                    </div>
                  );
                })() : (() => {
                  // 0.7+ — Quick Prompt reference UX. Quand un QP est sélectionné,
                  // le textarea + les hints sont cachés derrière un <details>
                  // "Personnaliser pour ce step" pour ne pas suggérer qu'il
                  // faut TOUT remplir. Le récap du QP montre ce qui sera utilisé.
                  // Le disclosure s'ouvre auto si l'utilisateur a déjà overridé.
                  const selectedQp = step.quick_prompt_id
                    ? availableQuickPrompts.find(qp => qp.id === step.quick_prompt_id) ?? null
                    : null;
                  const hasInlineOverride = !!step.prompt_template.trim();
                  return (
                  <>
                    {availableQuickPrompts.length > 0 && (
                      <div className="mb-3">
                        <label className="text-xs text-muted mb-1">
                          {t('wiz.agentQpPicker')}
                        </label>
                        <select
                          className="wf-select text-sm"
                          value={step.quick_prompt_id ?? ''}
                          onChange={e => updateStep(i, { quick_prompt_id: e.target.value || null })}
                        >
                          <option value="">{t('wiz.agentQpPickerInline')}</option>
                          {availableQuickPrompts.map(qp => (
                            <option key={qp.id} value={qp.id}>
                              {qp.icon} {qp.name}
                            </option>
                          ))}
                        </select>
                        {selectedQp && (
                          <div className="wf-qref-banner">
                            <div className="wf-qref-banner-header">
                              <strong>🔗 {t('wiz.agentQpInheritedFrom').replace('{0}', selectedQp.name)}</strong>
                              {hasInlineOverride && (
                                <span className="wf-qref-override-badge" title={t('wiz.qrefOverrideActiveHint')}>
                                  🔓 {t('wiz.qrefOverrideActive')}
                                </span>
                              )}
                            </div>
                            <div className="wf-qref-banner-body">
                              <div className="wf-qref-field">
                                <span className="wf-qref-field-label">{t('wiz.qrefPreview')}</span>
                                <code className="wf-qref-field-value">
                                  {selectedQp.prompt_template.length > 200
                                    ? selectedQp.prompt_template.slice(0, 200) + '…'
                                    : selectedQp.prompt_template || <em>{t('wiz.qrefEmpty')}</em>}
                                </code>
                              </div>
                              {selectedQp.variables.length > 0 && (
                                <div className="wf-qref-field">
                                  <span className="wf-qref-field-label">{t('wiz.qrefVars')}</span>
                                  <span className="wf-qref-field-value">
                                    {selectedQp.variables.map(v => (
                                      <code key={v.name} className="wf-qref-var-chip">{`{{${v.name}}}`}</code>
                                    ))}
                                  </span>
                                </div>
                              )}
                            </div>
                            <p className="wf-qref-hint">{t('wiz.agentQpInheritedHint')}</p>
                          </div>
                        )}
                      </div>
                    )}
                    {/* Quand un QP est référencé, on enroule le textarea +
                        ses hints dans un <details> pour cacher le bruit
                        visuel par défaut. Open auto si override actif. */}
                    {selectedQp ? (
                      <details className="wf-qref-override" open={hasInlineOverride}>
                        <summary className="wf-qref-override-summary">
                          ✏️ {t('wiz.qrefOverridePromptToggle')}
                        </summary>
                        <div className="wf-qref-override-body">
                          <textarea
                            ref={el => { promptTextareaRefs.current[i] = el; }}
                            className="wf-textarea"
                            rows={3}
                            value={step.prompt_template}
                            onChange={e => updateStep(i, { prompt_template: e.target.value })}
                            placeholder={t('wiz.agentQpOverridePlaceholder')}
                          />
                        </div>
                      </details>
                    ) : (
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
                    )}
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
                    {/* B3 (0.7.0 UX pass) — STATE pédagogie chips.
                        Trois personas (Marie, Antony, Cyndie) ont flaggé
                        que la convention `---STATE:k=v---` est invisible.
                        On la matérialise via 2 chips conditionnelles :
                          (a) cette étape Goto vers un Agent → chip pour
                              insérer l'instruction "écris ---STATE:---"
                              dans le prompt (auto-clé `last_<this_name>`)
                          (b) cette étape est cible d'un Goto Agent → chip
                              pour insérer `{{state.last_<source_name>}}`
                              au curseur dans le prompt
                        Le user voit la convention, l'apprend, garde le
                        contrôle (transparence vs auto-magie). */}
                    {(() => {
                      const myName = step.name;
                      // Cibles Agent atteignables via Goto depuis CE step.
                      const gotoTargets = (step.on_result ?? [])
                        .filter(r => r.action.type === 'Goto')
                        .map(r => r.action.type === 'Goto' ? r.action.step_name : '')
                        .filter(name => name && name !== myName)
                        .map(name => steps.find(s => s.name === name))
                        .filter((s): s is WorkflowStep => !!s && (!s.step_type || s.step_type.type === 'Agent'));
                      // Sources Agent qui Goto vers CE step.
                      const gotoSources = steps
                        .filter(s => s.name !== myName)
                        .filter(s => !s.step_type || s.step_type.type === 'Agent')
                        .filter(s => (s.on_result ?? []).some(r =>
                          r.action.type === 'Goto' && r.action.step_name === myName));
                      if (gotoTargets.length === 0 && gotoSources.length === 0) return null;
                      return (
                        <div className="wf-state-chips mt-2">
                          {gotoTargets.length > 0 && (
                            <div className="flex-wrap flex-row gap-2 items-center">
                              <span className="text-xs text-ghost">{t('wiz.stateChipsLoopOut')}</span>
                              {gotoTargets.map(target => {
                                const stateKey = `last_${myName}`;
                                const block = t('wiz.stateInstructionBlock')
                                  .replace('{key}', stateKey)
                                  .replace('{target}', target.name);
                                return (
                                  <button
                                    key={target.name}
                                    type="button"
                                    className="wf-state-chip"
                                    onClick={() => appendPromptBlock(i, block)}
                                    title={t('wiz.stateChipWriteHint').replace('{key}', stateKey)}
                                  >
                                    + {t('wiz.stateChipWrite').replace('{key}', stateKey)}
                                  </button>
                                );
                              })}
                            </div>
                          )}
                          {gotoSources.length > 0 && (
                            <div className="flex-wrap flex-row gap-2 items-center mt-1">
                              <span className="text-xs text-ghost">{t('wiz.stateChipsLoopIn')}</span>
                              {gotoSources.map(source => {
                                const stateKey = `last_${source.name}`;
                                const varText = `{{state.${stateKey}}}`;
                                return (
                                  <button
                                    key={source.name}
                                    type="button"
                                    className="wf-state-chip"
                                    onClick={() => insertVarAtCursor(i, varText)}
                                    title={t('wiz.stateChipReadHint').replace('{key}', stateKey)}
                                  >
                                    + {varText}
                                  </button>
                                );
                              })}
                            </div>
                          )}
                        </div>
                      );
                    })()}
                    {/* P2 (0.6.0 UX pass) — Undeclared `{{var}}` warning.
                        Scans this step's prompt en live et flagge toute
                        var qui ne matche aucune source connue (steps
                        antérieurs, state, iter, artifacts, vars de
                        lancement déclarées, trigger fields). Bouton
                        "Ajouter à variables" qui pré-remplit la card
                        Variables de lancement → workflow complet en 1 clic. */}
                    {(() => {
                      const undeclared = scanUndeclaredVars(step.prompt_template, {
                        allSteps: steps,
                        currentStepIdx: i,
                        inRollback: false,
                        triggerType,
                        workflowVariables: wfVariables,
                        artifacts: {},
                      });
                      if (undeclared.length === 0) return null;
                      return (
                        <div className="wf-undeclared-warning mt-2">
                          <div className="flex-row gap-2 items-center mb-1">
                            <AlertTriangle size={11} className="text-warning" />
                            <span className="text-xs font-semibold">{t('wiz.undeclaredVarsTitle')}</span>
                          </div>
                          <p className="text-2xs text-ghost mb-2">{t('wiz.undeclaredVarsHint')}</p>
                          <div className="flex-wrap flex-row gap-2">
                            {undeclared.map(uv => {
                              // Only "unknown_bare" → actionable button to declare it.
                              // Other reasons (unknown_step, failed_step_outside_rollback)
                              // are typos / wrong context — user has to fix manually.
                              const canDeclare = uv.reason === 'unknown_bare' && !uv.name.includes('.');
                              return (
                                <span key={uv.name} className="wf-undeclared-chip">
                                  <code>{`{{${uv.name}}}`}</code>
                                  {canDeclare ? (
                                    <button
                                      type="button"
                                      className="wf-undeclared-add-btn"
                                      onClick={() => {
                                        if (wfVariables.some(v => v.name === uv.name)) return;
                                        setWfVariables(prev => [...prev, {
                                          name: uv.name,
                                          label: uv.name,
                                          placeholder: '',
                                          description: null,
                                          required: true,
                                        }]);
                                      }}
                                      title={t('wiz.undeclaredAddVarHint').replace('{name}', uv.name)}
                                    >
                                      + {t('wiz.undeclaredAddVar')}
                                    </button>
                                  ) : (
                                    <span className="text-2xs text-ghost">
                                      {uv.reason === 'unknown_step' ? t('wiz.undeclaredUnknownStep')
                                        : t('wiz.undeclaredFailedOutsideRollback')}
                                    </span>
                                  )}
                                </span>
                              );
                            })}
                          </div>
                        </div>
                      );
                    })()}
                  </>
                  );
                })()}

                {/* Skills / Profile / Directives selectors only apply to Agent
                    steps — the runner injects them into the LLM prompt. They
                    are no-ops on ApiCall (HTTP), Notify (webhook), Gate (pause),
                    Exec (shell), and BatchQuickPrompt (each item carries its
                    own QP-level config). Hiding them removes ~30vh of dead
                    real-estate on those step types. */}
                {(!step.step_type || step.step_type.type === 'Agent') && (<>
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
                              border: `1px solid ${profile.color || 'rgba(var(--kr-purple-rgb), 0.4)'}`,
                              background: `${profile.color}15`,
                              color: profile.color || 'var(--kr-purple-soft)',
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
                </>)}

                {/* Output format — visible at the root of the step card.
                    Moved out of the Advanced panel because a FreeText
                    producer feeding a `{{previous_step.data}}` consumer
                    used to silently break workflows at runtime. Only
                    relevant for Agent steps: ApiCall returns the JSONPath-
                    extracted value, Exec returns stdout, Notify/Gate emit
                    nothing parseable, BatchQuickPrompt's shape is set by
                    the QP itself. */}
                {(!step.step_type || step.step_type.type === 'Agent') && (
                  <div className="mb-4 mt-4">
                    <label className="wf-label">{t('wiz.outputFormat')}</label>
                    <div className="flex-row gap-3">
                      <button
                        className="wf-step-type-btn"
                        data-selected={step.output_format?.type === 'Structured'}
                        onClick={() => updateStep(i, { output_format: { type: 'Structured' } })}
                      >{t('wiz.outputStructured')}</button>
                      <button
                        className="wf-step-type-btn"
                        data-selected={!step.output_format || step.output_format.type === 'FreeText'}
                        onClick={() => updateStep(i, { output_format: { type: 'FreeText' } })}
                      >{t('wiz.outputFree')}</button>
                    </div>
                    <p className="text-2xs text-ghost mt-2">
                      {step.output_format?.type === 'Structured'
                        ? t('wiz.outputStructuredHint')
                        : t('wiz.outputFreeHint')}
                    </p>
                  </div>
                )}

                {/* Advanced toggle */}
                <button
                  className="wf-advanced-toggle"
                  style={{ color: hasAdvanced ? 'var(--kr-accent-ink)' : 'var(--kr-text-ghost)' }}
                  onClick={() => setExpandedStepAdvanced(isAdvOpen ? null : i)}
                >
                  <Settings size={10} />
                  {t('wiz.advanced')}{hasAdvanced ? ' *' : ''}
                  {isAdvOpen ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
                </button>

                {isAdvOpen && (
                  <div className="wf-advanced-panel">
                    {/* Agent settings — only meaningful for Agent steps; the
                        runner ignores `agent_settings` on ApiCall/Notify/
                        Gate/Exec/BatchQuickPrompt. */}
                    {(!step.step_type || step.step_type.type === 'Agent') && (
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
                    )}

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

                    {/* on_result conditions — 0.6.0 UX pass : layout 2 lignes
                        pour ne plus déborder. Avant : tout sur 1 ligne flex
                        avec un `flex-1` qui se faisait écraser par les selects
                        Goto (220+140+56+labels+X = ~500px de fixe). Maintenant :
                          ligne 1 = "Si contient [______________________]"
                          ligne 2 = "→ [Action] (si Goto: step + max N itérations)"
                        Le bouton remove vit en haut à droite, plus accessible. */}
                    <div>
                      <label className="wf-label">{t('wiz.conditions')}</label>
                      {(step.on_result ?? []).map((cond, j) => (
                        <div key={j} className="wf-condition-row mb-3">
                          <div className="wf-condition-row-line1">
                            <span className="text-xs text-dim" style={{ whiteSpace: 'nowrap' }}>{t('wiz.ifContains')}</span>
                            <input
                              className="wf-input flex-1 text-sm"
                              style={{ borderColor: !cond.contains ? 'rgba(var(--kr-error-rgb), 0.4)' : undefined }}
                              value={cond.contains}
                              onChange={e => updateCondition(i, j, { contains: e.target.value })}
                              placeholder={t('wiz.ifContainsPlaceholder')}
                            />
                            <button
                              className="wf-icon-btn"
                              onClick={() => removeCondition(i, j)}
                              aria-label="Remove condition"
                              title={t('wiz.removeCondition')}
                            >
                              <X size={10} />
                            </button>
                          </div>
                          <div className="wf-condition-row-line2">
                            <span className="text-xs text-dim">&rarr;</span>
                            {/* B7 (0.7.0 UX pass) — dropdown enrichi avec
                                sous-libellés explicites pour la diff
                                Stop/Skip/Goto. */}
                            <select
                              className="wf-select text-sm wf-condition-action-select"
                              value={cond.action.type}
                              onChange={e => {
                                const type = e.target.value as 'Stop' | 'Skip' | 'Goto';
                                const action = type === 'Goto' ? { type: 'Goto' as const, step_name: '', max_iterations: null } : { type };
                                updateCondition(i, j, { action: action as StepConditionRule['action'] });
                              }}
                            >
                              <option value="Stop">{t('wiz.condActionStop')}</option>
                              <option value="Skip">{t('wiz.condActionSkip')}</option>
                              <option value="Goto">{t('wiz.condActionGoto')}</option>
                            </select>
                            {cond.action.type === 'Goto' && (
                              <>
                                {/* B1 — step cible = dropdown (anti free-text). */}
                                <select
                                  className="wf-select text-sm wf-condition-goto-select"
                                  value={cond.action.type === 'Goto' ? cond.action.step_name : ''}
                                  onChange={e => updateCondition(i, j, { action: { type: 'Goto', step_name: e.target.value, max_iterations: cond.action.type === 'Goto' ? cond.action.max_iterations ?? null : null } })}
                                >
                                  <option value="">— {t('wiz.gotoTargetSelect')} —</option>
                                  {steps.map((s, k) => k !== i && (
                                    <option key={s.name} value={s.name}>{s.name}</option>
                                  ))}
                                </select>
                                <span className="text-2xs text-ghost">{t('wiz.gotoMaxIterLabel')}</span>
                                <input
                                  type="number"
                                  min={1}
                                  className="wf-input text-sm"
                                  style={{ width: 56 }}
                                  value={cond.action.type === 'Goto' && cond.action.max_iterations != null ? cond.action.max_iterations : ''}
                                  onChange={e => {
                                    const n = e.target.value ? parseInt(e.target.value) : null;
                                    if (cond.action.type === 'Goto') {
                                      updateCondition(i, j, { action: { type: 'Goto', step_name: cond.action.step_name, max_iterations: n } });
                                    }
                                  }}
                                  placeholder={t('wiz.gotoMaxIterPlaceholder')}
                                  title={t('wiz.gotoMaxIterHint')}
                                />
                                <span className="text-2xs text-ghost">{t('wiz.gotoMaxIterUnit')}</span>
                              </>
                            )}
                          </div>
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
              </div>
            );
          })}
          <button className="wf-add-step-btn" onClick={addStep}>
            <Plus size={12} /> {t('wiz.addStep')}
          </button>

          {/* 0.7.0 Phase 7 — Rollback / compensation chain (workflow-level).
              Wizard supports adding Notify-only rollback steps (most common
              "tell ops on failure" case). Agent / ApiCall rollback steps
              can be added via the API for advanced setups. */}
          <div className="wf-rollback-section mt-8">
            <div className="flex-row gap-3 mb-2">
              <RotateCcw size={14} className="text-warning" />
              <span className="text-md font-semibold text-secondary">{t('wiz.rollbackTitle')}</span>
            </div>
            <p className="text-xs text-muted mb-3">{t('wiz.rollbackHint')}</p>
            {/* B6 (0.7.0 UX pass) — matrice explicite des conditions de
                fire. Marie + Antony + Cyndie : "rollback se déclenche
                quand au juste ?" → on liste ✓/✗ inline plutôt que de
                renvoyer à la doc. Le mot "rollback" est polysémique,
                on désamorce le malentendu de loin. */}
            <div className="wf-rollback-matrix mb-4">
              <div className="wf-rollback-matrix-row" data-fires="true">
                <span>✓</span><span>{t('wiz.rollbackFiresFailed')}</span>
              </div>
              <div className="wf-rollback-matrix-row">
                <span>✗</span><span>{t('wiz.rollbackSkipsCancelled')}</span>
              </div>
              <div className="wf-rollback-matrix-row">
                <span>✗</span><span>{t('wiz.rollbackSkipsGuard')}</span>
              </div>
              <div className="wf-rollback-matrix-row">
                <span>✗</span><span>{t('wiz.rollbackSkipsReject')}</span>
              </div>
            </div>

            {/* 0.7.0 UX pass — rollback steps support Agent, ApiCall, AND
                Notify in the wizard (was Notify-only). The backend already
                accepted the 3 types ; the wizard caught up so users
                don't write hand-rolled JSON for rollback agents.
                Gate is explicitly excluded (deadlock — validated server-side). */}
            {onFailureSteps.map((rb, idx) => {
              const rbKind = rb.step_type?.type ?? 'Notify';
              const updateRb = (patch: Partial<WorkflowStep>) =>
                setOnFailureSteps(prev => prev.map((s, i) => i === idx ? { ...s, ...patch } : s));
              return (
                <div key={idx} className="wf-rollback-step mb-3">
                  <div className="flex-row gap-2 mb-2 flex-wrap">
                    <input
                      className="wf-input text-sm"
                      style={{ width: 200 }}
                      value={rb.name}
                      onChange={e => updateRb({ name: e.target.value })}
                      placeholder="notify_ops"
                    />
                    {/* Step-type pills: Notify / Agent / ApiCall.
                        Switching resets type-specific config to a sane
                        default (no spurious leftovers between types). */}
                    <div className="flex-row gap-1">
                      <button
                        className="wf-step-type-btn"
                        data-type="notify"
                        data-selected={rbKind === 'Notify'}
                        onClick={() => updateRb({
                          step_type: { type: 'Notify' },
                          output_format: { type: 'FreeText' },
                          notify_config: rb.notify_config ?? { url: '', method: 'POST', headers: {}, body_template: '' },
                        })}
                      >{t('wiz.stepTypeNotify')}</button>
                      <button
                        className="wf-step-type-btn"
                        data-type="agent"
                        data-selected={rbKind === 'Agent'}
                        onClick={() => updateRb({
                          step_type: { type: 'Agent' },
                          output_format: { type: 'FreeText' },
                        })}
                      >{t('wiz.stepTypeAgent')}</button>
                      <button
                        className="wf-step-type-btn"
                        data-type="api"
                        data-selected={rbKind === 'ApiCall'}
                        onClick={() => updateRb({
                          step_type: { type: 'ApiCall' },
                          output_format: { type: 'Structured' },
                        })}
                      >{t('wiz.stepTypeApiCall')}</button>
                    </div>
                    <span className="flex-1" />
                    <button
                      className="wf-icon-btn"
                      onClick={() => setOnFailureSteps(prev => prev.filter((_, i) => i !== idx))}
                      aria-label={t('wiz.removeRollbackStep')}
                      title={t('wiz.removeRollbackStep')}
                    >
                      <X size={10} />
                    </button>
                  </div>

                  {rbKind === 'Notify' && (
                    <>
                      <input
                        className="wf-input text-sm mb-2"
                        value={rb.notify_config?.url ?? ''}
                        onChange={e => updateRb({ notify_config: { ...rb.notify_config ?? { url: '', method: 'POST', headers: {}, body_template: '' }, url: e.target.value } })}
                        placeholder="https://hooks.slack.com/services/..."
                      />
                      <textarea
                        className="wf-textarea text-sm"
                        rows={3}
                        value={rb.notify_config?.body_template ?? ''}
                        onChange={e => updateRb({ notify_config: { ...rb.notify_config ?? { url: '', method: 'POST', headers: {}, body_template: '' }, body_template: e.target.value } })}
                        placeholder={`{"text": "🚨 Workflow ${name || 'X'} échoué : {{failed_step.name}} — {{failed_step.output}}"}`}
                      />
                    </>
                  )}

                  {rbKind === 'Agent' && (
                    <>
                      <div className="flex-row gap-2 mb-2">
                        <select
                          className="wf-select text-sm"
                          style={{ width: 180 }}
                          value={rb.agent}
                          onChange={e => updateRb({ agent: e.target.value as AgentType })}
                        >
                          {ALL_AGENT_TYPES.map(a => (
                            <option key={a} value={a}>{AGENT_LABELS[a] ?? a}</option>
                          ))}
                        </select>
                      </div>
                      <textarea
                        className="wf-textarea text-sm"
                        rows={3}
                        value={rb.prompt_template}
                        onChange={e => updateRb({ prompt_template: e.target.value })}
                        placeholder={t('wiz.rollbackAgentPromptPlaceholder')}
                      />
                    </>
                  )}

                  {rbKind === 'ApiCall' && (
                    <ApiCallStepCard
                      step={rb}
                      onChange={updateRb}
                      availableApiPlugins={availableApiPlugins}
                      projectId={projectId || null}
                      installedAgents={installedAgentTypes}
                      configLanguage={configLanguage}
                      availableQuickApis={availableQuickApis}
                      t={t}
                    />
                  )}

                  <div className="mt-1 text-xs text-ghost flex-wrap flex-row gap-1">
                    <span>{t('wiz.clickToInsert')} :</span>
                    <code className="wf-var-hint-code" style={{ cursor: 'default' }}>{'{{failed_step.name}}'}</code>
                    <code className="wf-var-hint-code" style={{ cursor: 'default' }}>{'{{failed_step.output}}'}</code>
                  </div>
                </div>
              );
            })}

            <button
              className="wf-add-step-btn wf-add-step-btn-inline"
              onClick={() => setOnFailureSteps(prev => [...prev, {
                name: `rollback-${prev.length + 1}`,
                step_type: { type: 'Notify' },
                description: null,
                agent: 'ClaudeCode',
                prompt_template: '',
                mode: { type: 'Normal' },
                output_format: { type: 'FreeText' },
                notify_config: { url: '', method: 'POST', headers: {}, body_template: '' },
              }])}
            >
              <Plus size={10} /> {t('wiz.addRollbackStep')}
            </button>
          </div>
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

          {/* 0.7.0 — Execution limits (Guards). Visible by default,
              placed BEFORE the Advanced toggle (Antoine UX rationale:
              not hidden, not advanced — first-class safety control). */}
          <ExecutionLimitsCard value={guards} onChange={setGuards} t={t} />

          {/* 0.7.0 Phase 5 — Exec allowlist. Visible by default; the
              prominent placement is intentional because Exec is a
              security-sensitive feature and the empty-default-disabled
              status is the user's safety net. Editable as a comma-separated
              list of binary names — same backend validation rules apply. */}
          <div className="wf-section mt-6">
            <div className="flex-row gap-3 mb-2">
              <Terminal size={14} className="text-warning" />
              <span className="text-md font-semibold text-secondary">{t('wiz.execAllowlistTitle')}</span>
            </div>
            <p className="text-xs text-muted mb-3">{t('wiz.execAllowlistHint')}</p>
            <input
              ref={execAllowlistInputRef}
              className="wf-input text-sm"
              value={execAllowlist.join(', ')}
              onChange={e => setExecAllowlist(
                e.target.value.split(',').map(s => s.trim()).filter(s => s.length > 0)
              )}
              placeholder={t('wiz.execAllowlistPlaceholder')}
            />
          </div>

          {/* 0.6.0 UX pass — Workflow launch variables (mirrors QP variables).
              When the user clicks "Lancer" with trigger=Manual + non-empty
              list, a form asks for one value per variable. Each variable
              renders as `{{name}}` inside any step prompt. Required check
              is enforced server-side; the UI just collects + validates that
              required ones are non-empty before sending. */}
          <div className="wf-section mt-6">
            <div className="flex-row gap-3 mb-2">
              <FileText size={14} className="text-accent" />
              <span className="text-md font-semibold text-secondary">{t('wiz.wfVariablesTitle')}</span>
            </div>
            <p className="text-xs text-muted mb-3">{t('wiz.wfVariablesHint')}</p>
            {wfVariables.map((v, idx) => (
              <div key={idx} className="qp-var-row mb-2" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
                <div className="flex-row gap-3" style={{ alignItems: 'center' }}>
                  <input
                    className="wf-input text-sm"
                    style={{ width: 160, fontFamily: 'var(--kr-font-mono, monospace)' }}
                    value={v.name}
                    onChange={e => setWfVariables(prev => prev.map((pv, j) =>
                      j === idx ? { ...pv, name: e.target.value.replace(/[^a-zA-Z0-9_]/g, '') } : pv
                    ))}
                    placeholder="ticket_id"
                  />
                  <input
                    className="wf-input flex-1 text-sm"
                    value={v.label}
                    onChange={e => setWfVariables(prev => prev.map((pv, j) => j === idx ? { ...pv, label: e.target.value } : pv))}
                    placeholder={t('qp.varLabel')}
                  />
                  <input
                    className="wf-input flex-1 text-sm"
                    value={v.placeholder}
                    onChange={e => setWfVariables(prev => prev.map((pv, j) => j === idx ? { ...pv, placeholder: e.target.value } : pv))}
                    placeholder={t('qp.varPlaceholder')}
                  />
                  <label className="flex-row gap-2 text-xs" style={{ whiteSpace: 'nowrap', cursor: 'pointer' }}>
                    <input
                      type="checkbox"
                      checked={v.required ?? true}
                      onChange={e => setWfVariables(prev => prev.map((pv, j) => j === idx ? { ...pv, required: e.target.checked } : pv))}
                    />
                    {t('qp.varRequired')}
                  </label>
                  <button
                    className="wf-icon-btn"
                    onClick={() => setWfVariables(prev => prev.filter((_, j) => j !== idx))}
                    aria-label={t('wiz.removeVariable')}
                    title={t('wiz.removeVariable')}
                  ><X size={10} /></button>
                </div>
                <input
                  className="wf-input text-xs"
                  value={v.description ?? ''}
                  onChange={e => setWfVariables(prev => prev.map((pv, j) => j === idx ? { ...pv, description: e.target.value || null } : pv))}
                  placeholder={t('qp.varDescriptionPlaceholder')}
                  style={{ opacity: 0.85 }}
                />
              </div>
            ))}
            <button
              className="wf-add-step-btn wf-add-step-btn-inline"
              onClick={() => setWfVariables(prev => [...prev, {
                name: `var_${prev.length + 1}`,
                label: '',
                placeholder: '',
                description: null,
                required: true,
              }])}
            >
              <Plus size={10} /> {t('wiz.addVariable')}
            </button>
          </div>

          {/* Expert options toggle */}
          <button
            className="wf-advanced-toggle"
            style={{ color: showExpertConfig ? 'var(--kr-accent-ink)' : 'var(--kr-text-ghost)' }}
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
            const typeLabel = typeKind === 'ApiCall' ? 'API'
              : typeKind === 'BatchQuickPrompt' ? 'BATCH'
              : typeKind === 'Notify' ? 'NOTIFY'
              : typeKind === 'Gate' ? 'GATE'
              : typeKind === 'Exec' ? 'EXEC'
              : typeKind === 'BatchApiCall' ? 'BATCH API'
              : typeKind === 'JsonData' ? 'JSON'
              : 'AGENT';
            const typeData = typeKind === 'ApiCall' ? 'api'
              : typeKind === 'BatchQuickPrompt' ? 'batch-qp'
              : typeKind === 'Notify' ? 'notify'
              : typeKind === 'Gate' ? 'gate'
              : typeKind === 'Exec' ? 'exec'
              : typeKind === 'BatchApiCall' ? 'batch-api'
              : typeKind === 'JsonData' ? 'json-data'
              : 'agent';
            const isBatch = typeKind === 'BatchQuickPrompt';
            const isApi = typeKind === 'ApiCall';
            const isNotify = typeKind === 'Notify';
            const isGate = typeKind === 'Gate';
            const isExec = typeKind === 'Exec';
            const isBatchApi = typeKind === 'BatchApiCall';
            const isJsonData = typeKind === 'JsonData';
            const qpName = isBatch && s.batch_quick_prompt_id
              ? availableQuickPrompts.find(qp => qp.id === s.batch_quick_prompt_id)?.name
              : null;
            // For ApiCall, show `<plugin name> — <config label> · <endpoint>`
            // — resolve the user-visible names from the catalog rather
            // than echoing internal ids (`mcp-github` slug, `cfg-abc123`
            // uuid) which don't speak to anyone. Falls back to the slug
            // only when the catalog lookup misses (offline, deleted
            // config, etc.) so the recap still tells *something*.
            // Never an agent label (the call hits an API directly, no
            // LLM in the loop). For Notify, show the webhook host so
            // the user can confirm where the ping lands. Only Agent /
            // Custom steps surface an agent label.
            const apiPlugin = isApi
              ? availableApiPlugins.find(p => p.config.id === s.api_config_id)
                ?? availableApiPlugins.find(p => p.server.id === s.api_plugin_slug)
              : null;
            const apiSubtitle = isApi
              ? [
                  apiPlugin
                    ? `${apiPlugin.server.name} — ${apiPlugin.config.label}`
                    : (s.api_plugin_slug ?? '?'),
                  s.api_endpoint_path,
                ].filter(Boolean).join(' · ')
              : null;
            const notifyHost = isNotify && s.notify_config?.url
              ? (() => {
                  try { return new URL(s.notify_config.url).host; }
                  catch { return s.notify_config.url; }
                })()
              : null;
            const nameColor = isBatch || isApi || isNotify || isGate || isExec || isBatchApi || isJsonData
              ? 'var(--kr-text-faint)'
              : (AGENT_COLORS[s.agent] ?? 'var(--kr-text-faint)');
            const execSubtitle = isExec && s.exec_command
              ? [s.exec_command, ...(s.exec_args ?? [])].join(' ')
              : null;
            // Pour BatchApiCall on montre le plugin + endpoint + la source des items.
            // Format compact pour rester sur une ligne dans le résumé.
            const matchingPlugin = isBatchApi
              ? availableApiPlugins.find(p => p.config.id === s.api_config_id)
              : undefined;
            const batchApiSubtitle = isBatchApi
              ? [
                  matchingPlugin ? matchingPlugin.server.name : (s.api_plugin_slug ?? '?'),
                  s.api_endpoint_path,
                  s.batch_items_from ? `× ${s.batch_items_from}` : null,
                ].filter(Boolean).join(' · ')
              : null;
            return (
            <div key={i} className="wf-summary-row" style={{ paddingLeft: 20 }}>
              {i + 1}. <span className="wf-summary-step-type" data-type={typeData}>{typeLabel}</span>
              <span className="font-semibold" style={{ color: nameColor }}>{s.name}</span>
              {isBatch
                ? <span className="text-dim text-xs"> ({qpName ?? t('wiz.batchQPPickerEmpty')})</span>
                : isApi
                  ? <span className="text-dim text-xs"> ({apiSubtitle})</span>
                  : isNotify
                    ? <span className="text-dim text-xs"> ({notifyHost ?? t('wiz.notifyMissingUrl')})</span>
                    : isGate
                      ? null /* Gate has no per-step subtitle worth surfacing — the message is the body, not a header */
                      : isExec
                        ? <span className="text-dim text-xs"> ({execSubtitle ?? t('wiz.execCommandSelect')})</span>
                        : isBatchApi
                          ? <span className="text-dim text-xs"> ({batchApiSubtitle || t('wiz.batchApiMissingConfig')})</span>
                          : isJsonData
                            ? <span className="text-dim text-xs"> ({(() => {
                                const p = s.json_data_payload;
                                if (p === null || p === undefined) return t('wiz.jsonDataMissingPayload');
                                if (Array.isArray(p)) return t('wiz.jsonDataSummaryArray', p.length);
                                if (typeof p === 'object') return t('wiz.jsonDataSummaryObject', Object.keys(p as object).length);
                                return t('wiz.jsonDataSummaryScalar');
                              })()})</span>
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
          } else if (s.step_type?.type === 'ApiCall') {
            // ApiCall steps have no prompt — they hit an API directly via
            // the configured plugin. Validate the API-specific fields :
            //   - Soit `quick_api_id` est set (référence vers un QuickApi
            //     existant), auquel cas les fields api_* peuvent être
            //     vides ; le runner les hydrate au run-time depuis le QA.
            //   - Soit on tombe sur le mode inline et plugin/config/endpoint
            //     sont obligatoires.
            const hasQaRef = !!s.quick_api_id;
            if (!hasQaRef) {
              if (!s.api_plugin_slug || !s.api_config_id) {
                errors.push(t('wiz.errorApiNoPlugin').replace('{0}', label));
              }
              if (!s.api_endpoint_path || !s.api_endpoint_path.trim()) {
                errors.push(t('wiz.errorApiNoEndpoint').replace('{0}', label));
              }
            }
          } else if (s.step_type?.type === 'Notify') {
            // Notify steps drive a webhook URL — no prompt either.
            if (!s.notify_config?.url) {
              errors.push(t('wiz.errorNotifyNoUrl').replace('{0}', label));
            }
          } else if (s.step_type?.type === 'Gate') {
            // Gate steps pause for a human review — they need a message
            // to show the approver, but no LLM prompt.
            if (!s.gate_message || !s.gate_message.trim()) {
              errors.push(t('wiz.errorGateNoMessage').replace('{0}', label));
            }
          } else if (s.step_type?.type === 'Exec') {
            // Exec steps run a shell command from the allowlist — they
            // need a `exec_command`, no LLM prompt.
            if (!s.exec_command || !s.exec_command.trim()) {
              errors.push(t('wiz.errorExecNoCommand').replace('{0}', label));
            }
          } else if (s.step_type?.type === 'BatchApiCall') {
            // BatchApiCall fans out an API call over a list — same API
            // requirements as ApiCall plus an items source. Aussi : si
            // `quick_api_id` est set, l'API config arrive du QA au run-time.
            const hasQaRef = !!s.quick_api_id;
            if (!hasQaRef) {
              if (!s.api_plugin_slug || !s.api_config_id) {
                errors.push(t('wiz.errorApiNoPlugin').replace('{0}', label));
              }
              if (!s.api_endpoint_path || !s.api_endpoint_path.trim()) {
                errors.push(t('wiz.errorApiNoEndpoint').replace('{0}', label));
              }
            }
            if (!s.batch_items_from || !s.batch_items_from.trim()) {
              errors.push(t('wiz.errorBatchNoItemsFrom').replace('{0}', label));
            }
          } else if (s.step_type?.type === 'JsonData') {
            // JsonData steps émettent un payload littéral. Valider qu'il
            // est non-null et un JSON valide. Le textarea front a déjà un
            // try/catch, mais double-check au save (un user pourrait
            // bypasser via copy-paste rapide).
            if (s.json_data_payload === null || s.json_data_payload === undefined) {
              errors.push(t('wiz.errorJsonDataNoPayload').replace('{0}', label));
            }
          } else if (!s.prompt_template && !s.quick_prompt_id) {
            // Agent step needs SOIT un prompt_template, SOIT une référence
            // vers un QuickPrompt. Le runner hydrate le second au run-time.
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
            // Tooltip on hover when disabled — without it, the user
            // clicks Next, nothing happens, and there's zero hint that
            // the workflow name is the missing piece. Native HTML title
            // is the cheapest "why is this disabled?" affordance.
            title={wizardStep === 0 && !name ? t('wiz.nameRequired') : undefined}
          >
            {t('wiz.next')} <ChevronRight size={12} />
          </button>
        ) : (
          <>
          {saveError && (
            <div className="wf-restricted-warning" style={{ marginRight: 'auto', flex: '1 1 auto' }}>
              <AlertTriangle size={12} />
              <span className="flex-1">{saveError}</span>
              <button
                type="button"
                className="wf-icon-btn"
                onClick={() => setSaveError(null)}
                title={t('common.dismiss')}
                aria-label={t('common.dismiss')}
              ><X size={10} /></button>
            </div>
          )}
          <button
            className="wf-next-btn"
            onClick={handleSave}
            // The disabled predicate must match the visible-error
            // validator above. If they drift, the user lands in a
            // dead-button limbo: validator says "all good", button
            // stays grey, no feedback. Per step_type:
            //   - BatchQuickPrompt: needs `batch_quick_prompt_id` +
            //     `batch_items_from`.
            //   - ApiCall: needs `api_plugin_slug` + `api_config_id`
            //     + `api_endpoint_path`.
            //   - Notify: needs `notify_config.url`.
            //   - Gate: needs `gate_message`.
            //   - Exec: needs `exec_command`.
            //   - Agent / Custom (default): needs `prompt_template`.
            disabled={saving || !name || steps.some(s => {
              if (s.step_type?.type === 'BatchQuickPrompt') {
                if (!s.batch_quick_prompt_id) return true;
                if (!s.batch_items_from || !s.batch_items_from.trim()) return true;
              } else if (s.step_type?.type === 'ApiCall') {
                // 0.7+ — quick_api_id set = config arrive du QA au run-time,
                // les fields api_* peuvent être vides.
                if (!s.quick_api_id) {
                  if (!s.api_plugin_slug || !s.api_config_id) return true;
                  if (!s.api_endpoint_path || !s.api_endpoint_path.trim()) return true;
                }
              } else if (s.step_type?.type === 'Notify') {
                if (!s.notify_config?.url) return true;
              } else if (s.step_type?.type === 'Gate') {
                if (!s.gate_message || !s.gate_message.trim()) return true;
              } else if (s.step_type?.type === 'Exec') {
                if (!s.exec_command || !s.exec_command.trim()) return true;
              } else if (s.step_type?.type === 'BatchApiCall') {
                if (!s.quick_api_id) {
                  if (!s.api_plugin_slug || !s.api_config_id) return true;
                  if (!s.api_endpoint_path || !s.api_endpoint_path.trim()) return true;
                }
                if (!s.batch_items_from || !s.batch_items_from.trim()) return true;
              } else if (s.step_type?.type === 'JsonData') {
                // 0.7+ — payload obligatoire, sinon le runner failera au lancement.
                if (s.json_data_payload === null || s.json_data_payload === undefined) return true;
              } else if (!s.prompt_template && !s.quick_prompt_id) {
                // Agent step (default) : prompt_template OU quick_prompt_id requis.
                return true;
              }
              return (s.on_result ?? []).some(r => !r.contains);
            })}
          >
            {saving ? <Loader2 size={12} /> : <Check size={12} />}
            {isEdit ? t('wiz.save') : t('wiz.create')}
          </button>
          </>
        )}
      </div>
    </div>
  );
}

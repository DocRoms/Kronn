// Form to create/edit a saved QuickApi.
//
// Reuses `ApiCallStepCard` for the API config (plugin, endpoint, method,
// headers, body, extract, etc.) by treating a QuickApi as if it were a
// `WorkflowStep` of type `ApiCall` — every field name matches verbatim
// per design (see `models/mod.rs::QuickApi`). This means the AI helper
// (`ApiCallAiHelper`) is automatically available when editing a QuickApi
// — same UX as editing an ApiCall step in a workflow.
import { useEffect, useMemo, useRef, useState } from 'react';
import { Save, X, Plus } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { Dropdown } from '../Dropdown';
import type {
  AgentType,
  CreateQuickApiRequest,
  Project,
  PromptVariable,
  QuickApi,
  WorkflowStep,
} from '../../types/generated';
import { ApiCallStepCard, type ApiPluginOption } from './ApiCallStepCard';

interface Props {
  editApi?: QuickApi;
  projects: Project[];
  availableApiPlugins: ApiPluginOption[];
  installedAgents: AgentType[];
  configLanguage?: string;
  onSave: (req: CreateQuickApiRequest) => Promise<void>;
  onCancel: () => void;
}

/** Extract `{{var}}` and `{{#var}}` names from any string template field. */
function extractVarsFromString(s: string | null | undefined): string[] {
  if (!s) return [];
  const matches = s.match(/\{\{#?(\w+)\}\}/g) ?? [];
  return matches.map(m => m.replace(/\{\{#?|\}\}/g, ''));
}

/** Walk the QuickApi config to find every `{{var}}` token across the
 *  surface that gets templated at run-time: query/headers values, path
 *  params, body (recursively), and the endpoint path itself. */
function detectVariables(qa: Pick<QuickApi,
  'api_endpoint_path' | 'api_query' | 'api_path_params' | 'api_headers' | 'api_body'
>): string[] {
  const found = new Set<string>();
  for (const v of extractVarsFromString(qa.api_endpoint_path)) found.add(v);
  for (const v of Object.values(qa.api_query ?? {})) extractVarsFromString(v).forEach(n => found.add(n));
  for (const v of Object.values(qa.api_path_params ?? {})) extractVarsFromString(v).forEach(n => found.add(n));
  for (const v of Object.values(qa.api_headers ?? {})) extractVarsFromString(v).forEach(n => found.add(n));
  // Body is JSON Value — walk it.
  const walk = (v: unknown): void => {
    if (typeof v === 'string') {
      extractVarsFromString(v).forEach(n => found.add(n));
    } else if (Array.isArray(v)) {
      v.forEach(walk);
    } else if (v && typeof v === 'object') {
      Object.values(v as Record<string, unknown>).forEach(walk);
    }
  };
  walk(qa.api_body);
  return [...found];
}

export function QuickApiForm({
  editApi,
  projects,
  availableApiPlugins,
  installedAgents,
  configLanguage,
  onSave,
  onCancel,
}: Props) {
  const { t } = useT();

  // QuickApi state. We also keep a parallel `step`-shaped object that
  // ApiCallStepCard can mutate; updates flow back into the QuickApi
  // state via field-by-field mapping (the names match, so it's mostly
  // identity).
  const [name, setName] = useState(editApi?.name ?? '');
  const [icon, setIcon] = useState(editApi?.icon ?? '🔌');
  const [description, setDescription] = useState(editApi?.description ?? '');
  const [projectId, setProjectId] = useState(editApi?.project_id ?? '');
  const [variables, setVariables] = useState<PromptVariable[]>(editApi?.variables ?? []);
  const [saving, setSaving] = useState(false);
  // Race-free guard, cf QuickPromptForm. Without this, a fast double-
  // click on Save would create the QuickApi twice.
  const savingRef = useRef(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  // The API config — shape mirrors WorkflowStep ApiCall fields one-to-one.
  const [apiPluginSlug, setApiPluginSlug] = useState(editApi?.api_plugin_slug ?? '');
  const [apiConfigId, setApiConfigId] = useState(editApi?.api_config_id ?? '');
  const [apiEndpointPath, setApiEndpointPath] = useState(editApi?.api_endpoint_path ?? '');
  const [apiMethod, setApiMethod] = useState<string | null>(editApi?.api_method ?? null);
  const [apiQuery, setApiQuery] = useState<Record<string, string> | null>(editApi?.api_query ?? null);
  const [apiPathParams, setApiPathParams] = useState<Record<string, string> | null>(editApi?.api_path_params ?? null);
  const [apiHeaders, setApiHeaders] = useState<Record<string, string> | null>(editApi?.api_headers ?? null);
  const [apiBody, setApiBody] = useState<unknown | null>(editApi?.api_body ?? null);
  const [apiExtract, setApiExtract] = useState<QuickApi['api_extract']>(editApi?.api_extract ?? null);
  const [apiPagination, setApiPagination] = useState<QuickApi['api_pagination']>(editApi?.api_pagination ?? null);
  const [apiTimeoutMs, setApiTimeoutMs] = useState<number | null>(editApi?.api_timeout_ms ?? null);
  const [apiMaxRetries, setApiMaxRetries] = useState<number | null>(editApi?.api_max_retries ?? null);
  // 0.8.5 — profile + directive bindings (symmetric with QuickPrompt).
  // QA is a pure HTTP call so these don't affect the request directly;
  // they propagate to downstream LLM consumers (e.g. when a chained QP
  // reads the QA output). The picker UI lives only in QuickPromptForm
  // today — QAs round-trip the bindings unchanged from import/bundle
  // payloads, which is enough until a real QA-driven LLM surface ships.
  const profileIds = editApi?.profile_ids ?? [];
  const directiveIds = editApi?.directive_ids ?? [];

  // Synthesize a step-shaped object so ApiCallStepCard can render against
  // it. The card only reads the fields it knows about — extra fields are
  // ignored. Memoized to avoid re-creating on every render.
  const ephemeralStep: WorkflowStep = useMemo(() => ({
    name: '__qa__',
    step_type: { type: 'ApiCall' },
    description: null,
    agent: 'ClaudeCode',
    prompt_template: '',
    mode: { type: 'Normal' },
    output_format: { type: 'Structured' },
    on_result: [],
    agent_settings: null,
    stall_timeout_secs: null,
    retry: null,
    delay_after_secs: null,
    mcp_config_ids: [],
    skill_ids: [],
    profile_ids: [],
    directive_ids: [],
    batch_quick_prompt_id: null,
    batch_items_from: null,
    batch_wait_for_completion: null,
    batch_max_items: null,
    batch_workspace_mode: null,
    batch_chain_prompt_ids: [],
    batch_concurrent_limit: null,
    notify_config: null,
    api_plugin_slug: apiPluginSlug || null,
    api_config_id: apiConfigId || null,
    api_endpoint_path: apiEndpointPath || null,
    api_method: apiMethod,
    api_query: apiQuery,
    api_path_params: apiPathParams,
    api_headers: apiHeaders,
    api_body: apiBody,
    api_extract: apiExtract,
    api_pagination: apiPagination,
    api_timeout_ms: apiTimeoutMs,
    api_max_retries: apiMaxRetries,
    api_output_var: null,
    gate_message: null,
    gate_request_changes_target: null,
    gate_notify_url: null,
    exec_command: null,
    exec_args: [],
    exec_timeout_secs: null,
  }), [
    apiPluginSlug, apiConfigId, apiEndpointPath, apiMethod, apiQuery,
    apiPathParams, apiHeaders, apiBody, apiExtract, apiPagination,
    apiTimeoutMs, apiMaxRetries,
  ]);

  /** Apply a partial WorkflowStep update by routing each known field
   *  back to the matching QuickApi state setter. Unknown fields are
   *  silently dropped (the card only edits API-related fields anyway). */
  const handleStepChange = (updates: Partial<WorkflowStep>) => {
    if ('api_plugin_slug' in updates) setApiPluginSlug(updates.api_plugin_slug ?? '');
    if ('api_config_id' in updates) setApiConfigId(updates.api_config_id ?? '');
    if ('api_endpoint_path' in updates) setApiEndpointPath(updates.api_endpoint_path ?? '');
    if ('api_method' in updates) setApiMethod(updates.api_method ?? null);
    if ('api_query' in updates) setApiQuery(updates.api_query ?? null);
    if ('api_path_params' in updates) setApiPathParams(updates.api_path_params ?? null);
    if ('api_headers' in updates) setApiHeaders(updates.api_headers ?? null);
    if ('api_body' in updates) setApiBody(updates.api_body ?? null);
    if ('api_extract' in updates) setApiExtract(updates.api_extract ?? null);
    if ('api_pagination' in updates) setApiPagination(updates.api_pagination ?? null);
    if ('api_timeout_ms' in updates) setApiTimeoutMs(updates.api_timeout_ms ?? null);
    if ('api_max_retries' in updates) setApiMaxRetries(updates.api_max_retries ?? null);
  };

  // Auto-sync detected variables across the API config — preserve any
  // existing label/placeholder/description/required set by the user.
  useEffect(() => {
    const detected = detectVariables({
      api_endpoint_path: apiEndpointPath,
      api_query: apiQuery,
      api_path_params: apiPathParams,
      api_headers: apiHeaders,
      api_body: apiBody as QuickApi['api_body'],
    });
    setVariables(prev => {
      const existing = new Map(prev.map(v => [v.name, v]));
      return detected.map(n => existing.get(n) ?? {
        name: n,
        label: n,
        placeholder: '',
        description: null,
        required: true,
      });
    });
  }, [apiEndpointPath, apiQuery, apiPathParams, apiHeaders, apiBody]);

  const handleSave = async () => {
    if (savingRef.current) return;
    savingRef.current = true;
    setSaving(true);
    setSaveError(null);
    try {
      await onSave({
        name,
        icon: icon || null,
        description,
        project_id: projectId || null,
        api_plugin_slug: apiPluginSlug,
        api_config_id: apiConfigId,
        api_endpoint_path: apiEndpointPath,
        api_method: apiMethod,
        api_query: apiQuery,
        api_path_params: apiPathParams,
        api_headers: apiHeaders,
        api_body: apiBody,
        api_extract: apiExtract,
        api_pagination: apiPagination,
        api_timeout_ms: apiTimeoutMs,
        api_max_retries: apiMaxRetries,
        variables,
        profile_ids: profileIds,
        directive_ids: directiveIds,
      });
    } catch (e) {
      // Backend validation errors (400 Bad Request, "name must be 1-200
      // chars", "api_plugin_slug missing"…) used to swallow silently —
      // the user clicked Save and the form stayed open without feedback.
      // Surface them inline so the user knows what to fix.
      console.error('[QuickApiForm] save failed:', e);
      setSaveError(e instanceof Error ? e.message : String(e));
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  };

  const canSave = !!name && !!apiPluginSlug && !!apiConfigId && !!apiEndpointPath;

  return (
    <div className="qp-form">
      <div className="flex-between mb-4">
        <h3 className="font-semibold">{editApi ? name || t('qa.name') : t('qa.new')}</h3>
        <button className="wf-icon-btn" onClick={onCancel}><X size={14} /></button>
      </div>

      <div className="flex-row gap-4 mb-4">
        <input
          className="wf-input"
          style={{ width: 50, textAlign: 'center', fontSize: 20 }}
          value={icon}
          onChange={e => setIcon(e.target.value)}
          placeholder="🔌"
          maxLength={2}
        />
        <input
          className="wf-input flex-1"
          value={name}
          onChange={e => setName(e.target.value)}
          placeholder={t('qa.namePlaceholder')}
        />
      </div>

      <div className="flex-row gap-4 mb-4">
        {/* 0.8.6 (#62) — Dropdown migration: theme parity (Firefox/Safari
            previously rendered <option> with OS chrome ignoring page CSS). */}
        <Dropdown<string>
          value={projectId}
          options={[
            { value: '', label: t('wiz.noProject') },
            ...projects.map(p => ({ value: p.id, label: p.name })),
          ]}
          onChange={v => setProjectId(v)}
          ariaLabel={t('wiz.noProject')}
          testId="qa-project-picker"
        />
      </div>

      <label className="wf-label">{t('qa.descriptionLabel')}</label>
      <textarea
        className="wf-textarea mb-4"
        rows={2}
        value={description}
        onChange={e => setDescription(e.target.value)}
        placeholder={t('qa.descriptionPlaceholder')}
      />

      {/* The actual API config — same component used by workflow ApiCall
          steps, AI helper included. The user types {{varname}} in any
          field; the variables editor below auto-syncs. */}
      <div className="mb-4">
        <ApiCallStepCard
          step={ephemeralStep}
          onChange={handleStepChange}
          availableApiPlugins={availableApiPlugins}
          projectId={projectId || null}
          installedAgents={installedAgents}
          configLanguage={configLanguage}
          t={t}
        />
      </div>

      {variables.length > 0 && (
        <div className="qp-vars mb-4">
          <label className="wf-label">{t('qa.variables')}</label>
          {variables.map((v, i) => (
            <div key={v.name} className="qp-var-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
              <div className="flex-row gap-4" style={{ alignItems: 'center' }}>
                <code className="qp-var-name">{`{{${v.name}}}`}</code>
                <input
                  className="wf-input flex-1"
                  value={v.label}
                  onChange={e => setVariables(prev => prev.map((pv, j) => j === i ? { ...pv, label: e.target.value } : pv))}
                  placeholder={t('qa.varLabel')}
                />
                <input
                  className="wf-input flex-1"
                  value={v.placeholder}
                  onChange={e => setVariables(prev => prev.map((pv, j) => j === i ? { ...pv, placeholder: e.target.value } : pv))}
                  placeholder={t('qa.varPlaceholder')}
                />
                <label className="flex-row gap-2" style={{ fontSize: 12, whiteSpace: 'nowrap', cursor: 'pointer' }}>
                  <input
                    type="checkbox"
                    checked={v.required ?? true}
                    onChange={e => setVariables(prev => prev.map((pv, j) => j === i ? { ...pv, required: e.target.checked } : pv))}
                  />
                  {t('qa.varRequired')}
                </label>
              </div>
              <input
                className="wf-input"
                value={v.description ?? ''}
                onChange={e => setVariables(prev => prev.map((pv, j) => j === i ? { ...pv, description: e.target.value || null } : pv))}
                placeholder={t('qa.varDescriptionPlaceholder')}
                style={{ fontSize: 12, opacity: 0.85 }}
              />
            </div>
          ))}
        </div>
      )}

      {variables.length === 0 && (
        <p className="text-2xs text-ghost mb-4">
          <Plus size={9} style={{ display: 'inline', verticalAlign: 'middle', marginRight: 4 }} />
          {t('qa.varsHint')}
        </p>
      )}

      {/* Surface what's missing when Save is disabled — without this hint
          the user sees a greyed button with no explanation and assumes
          the form is broken (it actually was, until this commit). The
          message lists every required field that's still empty. */}
      {!canSave && (
        <p className="text-xs text-ghost mb-2">
          {t('qa.saveBlockedHint')} :{' '}
          {[
            !name && t('qa.fieldName'),
            !apiPluginSlug && t('qa.fieldPlugin'),
            !apiConfigId && t('qa.fieldConfig'),
            !apiEndpointPath && t('qa.fieldEndpoint'),
          ].filter(Boolean).join(', ')}
        </p>
      )}
      {saveError && (
        <div className="wf-apicall-error mb-2" role="alert">
          {saveError}
        </div>
      )}
      <div className="flex-row gap-4">
        <button
          className="wf-create-btn"
          onClick={handleSave}
          disabled={saving || !canSave}
          title={!canSave ? t('qa.saveBlockedHint') : undefined}
        >
          <Save size={14} /> {saving ? '...' : t('qa.save')}
        </button>
      </div>
    </div>
  );
}

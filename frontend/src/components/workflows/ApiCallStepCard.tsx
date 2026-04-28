import { useState, useEffect, useMemo, useRef } from 'react';
import { Plug, Play, Loader2, ChevronDown, ChevronRight as ChevRight, KeyRound, Link2, X as XIcon } from 'lucide-react';
import { workflows as workflowsApi } from '../../lib/api';
import type { AgentType, McpConfigDisplay, McpServer, WorkflowStep, ExtractSpec, StepType, QuickApi } from '../../types/generated';
import { ApiCallAiHelper } from './ApiCallAiHelper';
import { authSlotsForServer, type AuthSlot } from './apiCallAuth';
import { suggestPaths, type PathSuggestion } from './apiCallSuggestions';
import { collectPlaceholders, substitutePlaceholders } from './apiCallPlaceholders';

/** Plugin + config pair rendered in the plugin picker. */
export interface ApiPluginOption {
  server: McpServer;
  config: McpConfigDisplay;
}

interface ApiCallStepCardProps {
  step: WorkflowStep;
  /** Called for every field edit. The wizard owns the source of truth. */
  onChange: (updates: Partial<WorkflowStep>) => void;
  /** Filtered list of plugins with `api_spec != null` configured on this project. */
  availableApiPlugins: ApiPluginOption[];
  /** Required for `test-api-call` — the backend scopes plugin env decryption per project. */
  projectId: string | null;
  /** Step that immediately follows this one in the wizard's list, used by
   *  [`NextStepBanner`] to validate the extraction shape. `undefined` = this
   *  is the last step (no compatibility check). */
  nextStepType?: StepType;
  /** Locally-installed agent types — shown in the AI helper picker. Empty array
   *  hides the helper button (no usable agent on this host). */
  installedAgents?: AgentType[];
  /** Backend "agent output language" — passed through to the AI helper so the
   *  ephemeral discussion + system prompt match the language the user selected
   *  in Settings → Output language. UI labels stay on the UI locale. */
  configLanguage?: string;
  /** 0.7+ — liste des QuickApis disponibles. Quand non vide, la card affiche
   *  un select "Depuis un Quick API existant" qui permet de référencer un QA
   *  via `step.quick_api_id`. Le runner hydrate les fields `api_*` manquants
   *  depuis le QA au run-time (per-field override : le step gagne quand set).
   *  Mirror du pattern existant pour BatchApiCall. */
  availableQuickApis?: QuickApi[];
  t: (key: string, ...args: (string | number)[]) => string;
}

type ExtractField = 'data' | 'status' | 'summary';

/** Card shown in the wizard when `step.step_type.type === 'ApiCall'`.
 *  Calque desktop (split 60/40) : à gauche le JSON response cliquable,
 *  à droite le panneau d'extraction (radios + path input + preview live).
 *  Mobile reste en stack vertical — l'édition sur petit écran est
 *  intentionnellement dégradée (cf. `ai/operations/deagent-apicall.md`). */
export function ApiCallStepCard({
  step,
  onChange,
  availableApiPlugins,
  projectId,
  nextStepType,
  installedAgents,
  configLanguage,
  availableQuickApis,
  t,
}: ApiCallStepCardProps) {
  const [testing, setTesting] = useState(false);
  const [response, setResponse] = useState<unknown>(null);
  const [responseError, setResponseError] = useState<string | null>(null);
  const [activeField, setActiveField] = useState<ExtractField>('data');
  // Test-time variable values modal. Opened when the user clicks Test and
  // we detect unresolved `{{var}}` placeholders that aren't runtime-only
  // tokens (`{{steps.X.*}}`, `{{state.*}}`, etc.). Without this the
  // backend would receive a literal `{{host}}` and forward it to the
  // upstream API, returning an opaque 4xx that doesn't tell the user
  // "you forgot to substitute your variable".
  const [testVarsPrompt, setTestVarsPrompt] = useState<{
    names: string[];
    values: Record<string, string>;
  } | null>(null);
  // Open the "Advanced" section by default whenever the step already
  // has any advanced field populated — covers loading an existing step
  // that uses a non-default method, custom timeout, etc. Auto-expansion
  // also fires on the false → true transition (so an AI-helper apply on
  // `body` or `method` surfaces immediately). Headers were intentionally
  // promoted OUT of advanced — they're too commonly required to hide.
  const hasAnyAdvanced = (s: WorkflowStep): boolean =>
    !!(s.api_body || s.api_method || s.api_output_var || s.api_timeout_ms || s.api_max_retries);
  const [showAdvanced, setShowAdvanced] = useState(() => hasAnyAdvanced(step));
  // Track previous "has advanced" snapshot so we only auto-expand on
  // the false → true transition. Without this, a user who manually
  // collapses the section after editing one of those fields gets the
  // section re-expanded on every subsequent keystroke — annoying.
  const prevHadAdvanced = useRef(hasAnyAdvanced(step));
  useEffect(() => {
    const now = hasAnyAdvanced(step);
    if (!prevHadAdvanced.current && now) {
      setShowAdvanced(true);
    }
    prevHadAdvanced.current = now;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [step.api_body, step.api_method, step.api_output_var, step.api_timeout_ms, step.api_max_retries]);

  // The step is identified by `api_config_id` (one row per user
  // credential) rather than by `api_plugin_slug` alone — otherwise the
  // wizard can't tell two configs of the same plugin apart (e.g. a
  // personal GitHub PAT vs an org-bound Euronews PAT both pointing at
  // `mcp-github`). The slug is derived from the matching config.
  // Legacy workflows that pre-date this fix may have only `api_plugin_slug`
  // set — fall back to the first matching server so they still render.
  const selectedPlugin = useMemo(
    () =>
      availableApiPlugins.find(p => p.config.id === step.api_config_id) ??
      availableApiPlugins.find(p => p.server.id === step.api_plugin_slug) ??
      null,
    [availableApiPlugins, step.api_config_id, step.api_plugin_slug],
  );
  const selectedServer = selectedPlugin?.server ?? null;

  // Reset response when the user switches plugin instance / endpoint —
  // the cached body becomes meaningless and clicking on its nodes would
  // generate paths against the wrong shape. Also fires when the user
  // swaps between two configs of the same server (perso → org), since
  // the env / scope differs and the response can change.
  useEffect(() => {
    setResponse(null);
    setResponseError(null);
  }, [step.api_config_id, step.api_endpoint_path]);

  // ── Test button ──
  const handleTest = async () => {
    // The backend's `/test-api-call` decrypts the plugin env scoped to a
    // project. If the wizard hasn't picked one yet we fall back to the
    // first project the selected config is already linked to (Settings →
    // APIs flow always wires at least one — global or specific). That
    // covers the natural "I just configured Chartbeat globally, why
    // does the wizard ask me to pick a project to *test*?" friction.
    const selectedConfig = availableApiPlugins.find(p => p.server.id === step.api_plugin_slug)?.config;
    const effectiveProjectId = projectId ?? selectedConfig?.project_ids?.[0] ?? null;
    if (!effectiveProjectId) {
      setResponseError(t('wf.apicall.testNeedsProjectLink'));
      return;
    }
    // Detect unresolved placeholders. If any, open the modal to collect
    // values and bail — the modal's submit handler will re-call this path
    // with `vars` provided.
    const unresolved = collectPlaceholders(step);
    if (unresolved.length > 0 && testVarsPrompt === null) {
      setTestVarsPrompt({
        names: unresolved,
        values: Object.fromEntries(unresolved.map(n => [n, ''])),
      });
      return;
    }
    setTesting(true);
    setResponseError(null);
    try {
      // Send the step WITHOUT `api_extract` so the response viewer always
      // receives the raw HTTP body — once the user has picked an extract
      // path, the backend would otherwise apply it and we'd display the
      // extracted scalar/array instead of the full JSON tree. The live
      // extract preview (right panel) uses `/test-extract` on the cached
      // raw response, so the user still sees the resolved value as they
      // type the path.
      let stepToSend: WorkflowStep = { ...step, api_extract: null };
      if (testVarsPrompt) {
        stepToSend = substitutePlaceholders(stepToSend, testVarsPrompt.values);
      }
      const res = await workflowsApi.testApiCall({ step: stepToSend, project_id: effectiveProjectId });
      if (res.success && res.envelope) {
        setResponse(res.envelope.data);
      } else {
        setResponseError(res.error ?? t('wf.apicall.testFailed'));
      }
    } catch (e) {
      setResponseError(String(e));
    } finally {
      setTesting(false);
      setTestVarsPrompt(null);
    }
  };

  // ── Empty state when no API plugin is wired on the project ──
  if (availableApiPlugins.length === 0) {
    return (
      <div className="wf-apicall-card wf-apicall-card-empty" role="region" aria-label={t('wf.apicall.title')}>
        <div className="wf-apicall-header">
          <Plug size={14} /> <strong>{t('wf.apicall.title')}</strong>
        </div>
        <p className="text-sm text-muted">{t('wf.apicall.notSupported')}</p>
      </div>
    );
  }

  const endpoints = selectedServer?.api_spec?.endpoints ?? [];
  const query = step.api_query ?? {};

  // 0.7+ — QuickApi reference state. Surfacé en variables au niveau du
  // composant pour que le JSX puisse adapter le rendu : si un QA est
  // sélectionné, les fields override sont enroulés derrière un disclosure
  // (ne pas suggérer qu'il faut tout remplir). `hasApiOverride` détecte si
  // au moins un field api_* a été overridé au niveau du step — auquel cas
  // le disclosure s'ouvre auto pour ne pas masquer le travail en cours.
  const selectedQa = availableQuickApis?.find(qa => qa.id === step.quick_api_id) ?? null;
  const hasApiOverride = !!selectedQa && (
    !!step.api_endpoint_path
    || !!step.api_method
    || !!step.api_extract
    || (!!step.api_query && Object.keys(step.api_query).length > 0)
    || (!!step.api_path_params && Object.keys(step.api_path_params).length > 0)
    || (!!step.api_headers && Object.keys(step.api_headers).length > 0)
    || step.api_body !== null && step.api_body !== undefined
    || !!step.api_pagination
    || !!step.api_timeout_ms
    || !!step.api_max_retries
    || !!step.api_output_var
  );

  return (
    <div className="wf-apicall-card" role="region" aria-label={t('wf.apicall.title')}>
      <div className="wf-apicall-header">
        <Plug size={14} /> <strong>{t('wf.apicall.title')}</strong>
        <span className="wf-apicall-subtitle">{t('wf.apicall.subtitle')}</span>
        {installedAgents && installedAgents.length > 0 && (
          <ApiCallAiHelper
            step={step}
            onApply={onChange}
            selectedServer={selectedServer}
            projectId={projectId}
            installedAgents={installedAgents}
            lastTestResponse={response}
            lastTestError={responseError}
            configLanguage={configLanguage}
            t={t}
          />
        )}
      </div>

      {/* ── QuickApi reference (0.7+) ──
          Picker + bandeau d'héritage avec récap riche. Les fields override
          ci-dessous sont cachés derrière un disclosure quand un QA est set —
          on ne suggère pas que tout doit être rempli. */}
      {availableQuickApis && availableQuickApis.length > 0 && (
        <div className="wf-apicall-qa-ref" style={{ marginBottom: 10 }}>
          <label className="wf-apicall-field">
            <span>
              <Link2 size={11} style={{ verticalAlign: 'middle', marginRight: 4 }} />
              {t('wf.apicall.qaPicker')}
            </span>
            <select
              value={step.quick_api_id ?? ''}
              onChange={e => onChange({ quick_api_id: e.target.value || null })}
            >
              <option value="">{t('wf.apicall.qaPickerInline')}</option>
              {availableQuickApis.map(qa => (
                <option key={qa.id} value={qa.id}>
                  {qa.icon} {qa.name} — {qa.api_method ?? 'GET'} {qa.api_endpoint_path}
                </option>
              ))}
            </select>
          </label>
          {selectedQa && (
            <div className="wf-qref-banner">
              <div className="wf-qref-banner-header">
                <strong>🔗 {t('wf.apicall.qaInheritedFrom').replace('{0}', selectedQa.name)}</strong>
                {hasApiOverride && (
                  <span className="wf-qref-override-badge" title={t('wiz.qrefOverrideActiveHint')}>
                    🔓 {t('wiz.qrefOverrideActive')}
                  </span>
                )}
                <button
                  type="button"
                  onClick={() => onChange({ quick_api_id: null })}
                  style={{
                    background: 'transparent', border: 'none',
                    color: 'var(--kr-text-muted)', cursor: 'pointer', padding: 2,
                  }}
                  title={t('wf.apicall.qaDetach')}
                >
                  <XIcon size={11} />
                </button>
              </div>
              <div className="wf-qref-banner-body">
                <div className="wf-qref-field">
                  <span className="wf-qref-field-label">{t('wf.apicall.qaSummaryEndpoint')}</span>
                  <code className="wf-qref-field-value">
                    {(selectedQa.api_method ?? 'GET')} {selectedQa.api_endpoint_path}
                  </code>
                </div>
                {selectedQa.api_extract && (
                  <div className="wf-qref-field">
                    <span className="wf-qref-field-label">{t('wf.apicall.qaSummaryExtract')}</span>
                    <code className="wf-qref-field-value">
                      {selectedQa.api_extract.path}
                    </code>
                  </div>
                )}
                {selectedQa.variables.length > 0 && (
                  <div className="wf-qref-field">
                    <span className="wf-qref-field-label">{t('wiz.qrefVars')}</span>
                    <span className="wf-qref-field-value">
                      {selectedQa.variables.map(v => (
                        <code key={v.name} className="wf-qref-var-chip">{`{{${v.name}}}`}</code>
                      ))}
                    </span>
                  </div>
                )}
              </div>
              <p className="wf-qref-hint">{t('wf.apicall.qaInheritedHint')}</p>
            </div>
          )}
        </div>
      )}

      {/* Quand un QA est référencé, les fields override sont cachés derrière
          un disclosure pour ne pas surcharger l'UI. Auto-open si override
          actif. Quand pas de QA, le rendu reste tel quel (mode inline). */}
      {selectedQa ? (
        <details className="wf-qref-override" open={hasApiOverride}>
          <summary className="wf-qref-override-summary">
            ✏️ {t('wf.apicall.qaOverrideToggle')}
          </summary>
          <div className="wf-qref-override-body">
            {renderApiFields()}
          </div>
        </details>
      ) : (
        renderApiFields()
      )}

      {/* Le rendu original n'est pas dupliqué : on l'extrait dans une
          fonction locale pour pouvoir le réutiliser dans les deux branches
          du conditionnel ci-dessus. JS hoisting garantit que la déclaration
          plus bas reste accessible ici. */}
    </div>
  );

  function renderApiFields() {
    return (
    <>
      {/* ── Plugin + endpoint pickers ── */}
      <div className="wf-apicall-pickers">
        <label className="wf-apicall-field">
          <span>{t('wf.apicall.pluginPicker')}</span>
          <select
            // Bound on `api_config_id` so two configs of the same plugin
            // (perso vs org) are distinguishable. The picker writes both
            // ids on change so the runner has everything it needs.
            value={step.api_config_id ?? ''}
            onChange={e => {
              const configId = e.target.value || null;
              const match = availableApiPlugins.find(p => p.config.id === configId);
              onChange({
                api_plugin_slug: match?.server.id ?? null,
                api_config_id: configId,
                // Reset downstream fields — the endpoint list may change.
                api_endpoint_path: null,
                api_method: null,
              });
            }}
          >
            <option value="">—</option>
            {availableApiPlugins.map(p => (
              <option key={p.config.id} value={p.config.id}>
                {p.server.name} — {p.config.label}
              </option>
            ))}
          </select>
        </label>

        <label className="wf-apicall-field">
          <span>{t('wf.apicall.endpointPicker')}</span>
          {/*
            Combobox: native <input> + <datalist> so the user can either
            pick a curated path from the spec OR edit it freely. Crucial
            for plugins like GitHub whose paths carry placeholders
            (`{owner}/{repo}/issues`) — the user picks the template, then
            substitutes the values inline. Stays a simple select-feel
            for plugins with stable paths (Chartbeat) because the
            datalist surfaces as a dropdown on focus.
          */}
          <input
            type="text"
            list={selectedServer ? `wf-apicall-endpoints-${selectedServer.id}` : undefined}
            value={step.api_endpoint_path ?? ''}
            onChange={e => onChange({ api_endpoint_path: e.target.value || null })}
            placeholder={t('wf.apicall.endpointPlaceholder')}
            disabled={!selectedServer}
            spellCheck={false}
          />
          {selectedServer && (
            <datalist id={`wf-apicall-endpoints-${selectedServer.id}`}>
              {endpoints.map(ep => (
                <option key={`${ep.method} ${ep.path}`} value={ep.path}>
                  {ep.method} — {ep.description.slice(0, 80)}
                </option>
              ))}
            </datalist>
          )}
        </label>
      </div>

      {/* ── Auth-managed slots (read-only, injected at runtime) ── */}
      <AuthManagedSlots server={selectedServer} t={t} />

      {/* ── Path params (only when the endpoint contains `{owner}`-style
            placeholders — typical of GitHub `/repos/{owner}/{repo}/…`).
            One input per detected token, persisted on `api_path_params`,
            substituted server-side at request time. ── */}
      <PathParamsEditor step={step} onChange={onChange} t={t} />

      {/* ── Query params key/value rows ── */}
      <QueryParamsEditor
        params={query}
        onChange={next => onChange({ api_query: next })}
        t={t}
      />

      {/*
        Headers editor — promoted out of the (collapsed-by-default)
        Advanced section. Real-world APIs need them more often than not
        (User-Agent for GitHub, X-API-Version for Adobe, Accept for
        custom mime types). Hiding them was the source of "I clicked
        Apply on a User-Agent suggestion and nothing happened" — the
        update was applied, the editor was just buried below the fold.
        Keep BodyEditor in Advanced (rare in workflow usage).
      */}
      <KeyValueEditor
        label={t('wf.apicall.headers')}
        params={step.api_headers ?? {}}
        onChange={next => onChange({ api_headers: next })}
        valuePlaceholder="application/json"
      />

      {/* ── Test button + response panel split ── */}
      <div className="wf-apicall-test-row">
        <button
          type="button"
          className="wf-apicall-test-btn"
          onClick={handleTest}
          // Only block on `testing` and the two configurations the request
          // can't be built without (slug + endpoint). `projectId` is checked
          // inside `handleTest` and the backend produces a clear error so
          // there's no point disabling the button — users wondered why the
          // button was greyed out with no explanation. Empty configurations
          // produce a fast actionable error message instead of silent block.
          disabled={testing || !step.api_plugin_slug || !step.api_endpoint_path}
          title={t('wf.apicall.testHint')}
        >
          {testing ? <Loader2 size={11} className="spin" /> : <Play size={11} />}
          {testing ? t('wf.apicall.testing') : t('wf.apicall.testBtn')}
        </button>
        <span className="wf-apicall-tokens-saved">{t('wf.apicall.tokensSaved')}</span>
      </div>

      {/* Variable-substitution modal — opens when the user clicks Test on
          a step containing user-defined `{{var}}` placeholders. Forces the
          user to provide concrete values for the test only (the underlying
          step config is unchanged). Without this, the backend forwards
          literal `{{host}}` to the upstream API and gets a confusing 4xx. */}
      {testVarsPrompt && (
        <div
          className="wf-import-modal-backdrop"
          onClick={() => !testing && setTestVarsPrompt(null)}
        >
          <div className="wf-import-modal" onClick={e => e.stopPropagation()}>
            <h3>{t('wf.apicall.testVarsTitle')}</h3>
            <p className="text-xs text-muted mb-3">{t('wf.apicall.testVarsHint')}</p>
            {testVarsPrompt.names.map(name => (
              <div key={name} className="flex-row gap-3 mb-2" style={{ alignItems: 'center' }}>
                <code className="qp-var-name" style={{ minWidth: 140 }}>{`{{${name}}}`}</code>
                <input
                  className="wf-input flex-1"
                  value={testVarsPrompt.values[name] ?? ''}
                  onChange={e => setTestVarsPrompt(prev => prev ? {
                    ...prev,
                    values: { ...prev.values, [name]: e.target.value },
                  } : prev)}
                  placeholder={t('wf.apicall.testVarsPlaceholder')}
                  autoFocus={testVarsPrompt.names[0] === name}
                />
              </div>
            ))}
            <div className="flex-row gap-3 mt-4">
              <button
                className="wf-cancel-btn"
                onClick={() => setTestVarsPrompt(null)}
                disabled={testing}
              >{t('common.cancel')}</button>
              <button
                className="wf-create-btn"
                onClick={handleTest}
                disabled={testing || testVarsPrompt.names.some(n => !testVarsPrompt.values[n]?.trim())}
              >
                {testing ? <Loader2 size={12} className="spin" /> : <Play size={12} />}
                {t('wf.apicall.testVarsGo')}
              </button>
            </div>
          </div>
        </div>
      )}

      {responseError && (
        <div className="wf-apicall-error" role="alert">
          {t('wf.apicall.testFailed')}: {responseError}
        </div>
      )}

      <div className="wf-apicall-split">
        {/* Left: JSON response viewer with click-to-pick */}
        <div className="wf-apicall-response">
          <div className="wf-apicall-section-title">{t('wf.apicall.responseTitle')}</div>
          {response == null ? (
            <div className="wf-apicall-response-empty">{t('wf.apicall.responseEmpty')}</div>
          ) : (
            <>
              <div className="text-xs text-ghost" style={{ marginBottom: 4 }}>
                {t('wf.apicall.clickToPickHint')}
              </div>
              <JsonTreeViewer
                data={response}
                onPick={path => {
                  // Apply to the currently-selected extract field.
                  applyPathToActiveField(step, onChange, activeField, path);
                }}
              />
            </>
          )}
        </div>

        {/* Right: Extract panel */}
        <ExtractPanel
          step={step}
          activeField={activeField}
          onActiveFieldChange={setActiveField}
          onChange={onChange}
          sample={response}
          nextStepType={nextStepType}
          t={t}
        />
      </div>

      {/* ── Advanced options (collapsed) ── */}
      <button
        type="button"
        className="wf-apicall-advanced-toggle"
        onClick={() => setShowAdvanced(v => !v)}
        aria-expanded={showAdvanced}
      >
        {showAdvanced ? <ChevronDown size={11} /> : <ChevRight size={11} />}
        {t('wf.apicall.advancedToggle')}
        {/* Surface a discreet pulse when at least one advanced field is
            populated — makes it obvious that "something is set in there"
            even when the section is collapsed (so an AI-helper apply on
            headers/body doesn't look silently dropped). */}
        {hasAnyAdvanced(step) && !showAdvanced && (
          <span className="wf-apicall-advanced-dot" aria-hidden="true" />
        )}
      </button>
      {showAdvanced && (
        <>
          <div className="wf-apicall-advanced">
            <label className="wf-apicall-field">
              <span>{t('wf.apicall.outputVarLabel')}</span>
              <input
                type="text"
                placeholder={step.name}
                value={step.api_output_var ?? ''}
                onChange={e => onChange({ api_output_var: e.target.value || null })}
              />
              <small className="text-xs text-ghost">{t('wf.apicall.outputVarHint')}</small>
            </label>
            <label className="wf-apicall-field">
              <span>{t('wf.apicall.timeoutLabel')}</span>
              <input
                type="number"
                min={1000}
                max={120_000}
                placeholder="30000"
                value={step.api_timeout_ms ?? ''}
                onChange={e => onChange({ api_timeout_ms: e.target.value ? Number(e.target.value) : null })}
              />
            </label>
            <label className="wf-apicall-field">
              <span>{t('wf.apicall.retriesLabel')}</span>
              <input
                type="number"
                min={0}
                max={5}
                placeholder="2"
                value={step.api_max_retries ?? ''}
                onChange={e => onChange({ api_max_retries: e.target.value ? Number(e.target.value) : null })}
              />
            </label>
          </div>

          {/* ── HTTP advanced (method override + headers + body) ── */}
          <div className="wf-apicall-advanced wf-apicall-http-advanced">
            <label className="wf-apicall-field">
              <span>{t('wf.apicall.methodLabel')}</span>
              <select
                value={step.api_method ?? ''}
                onChange={e => onChange({ api_method: e.target.value || null })}
              >
                {/* Empty = use the endpoint's spec method (recommended). The
                    plugin registry already declared it; only override if you
                    really know why. */}
                <option value="">— {t('wf.apicall.endpointPicker').toLowerCase()} —</option>
                <option value="GET">GET</option>
                <option value="POST">POST</option>
                <option value="PUT">PUT</option>
                <option value="PATCH">PATCH</option>
                <option value="DELETE">DELETE</option>
              </select>
            </label>
          </div>

          <BodyEditor
            value={step.api_body ?? null}
            onChange={next => onChange({ api_body: next })}
            label={t('wf.apicall.body')}
          />
        </>
      )}
    </>
    );
  }
}

// ─── PathParamsEditor ─────────────────────────────────────────────────────
//
// Detects `{name}` placeholders in `step.api_endpoint_path` and renders one
// input per unique token. Values land in `step.api_path_params`; the backend
// substitutes them at request time (`resolve_path_params` in the executor).
// Empty values stay literal `{name}` in the URL — the request will then
// 404, which is the right diagnostic (vs silently dropping the segment).
//
// Why a separate field rather than mutating `api_endpoint_path` directly?
// Round-trip: re-loading a saved workflow shows BOTH the template and the
// concrete values, so the user can change the token without retyping the
// whole path.
const PATH_PLACEHOLDER_RX = /\{([A-Za-z0-9_-]+)\}/g;

function PathParamsEditor({
  step,
  onChange,
  t,
}: {
  step: WorkflowStep;
  onChange: (updates: Partial<WorkflowStep>) => void;
  t: (key: string, ...args: (string | number)[]) => string;
}) {
  // Extract unique placeholder names from the current path. We dedupe so a
  // path like `/orgs/{owner}/repos/{owner}/x` (silly but possible) only
  // surfaces a single `owner` input.
  const placeholders = useMemo(() => {
    const path = step.api_endpoint_path ?? '';
    const seen = new Set<string>();
    const out: string[] = [];
    for (const m of path.matchAll(PATH_PLACEHOLDER_RX)) {
      const name = m[1];
      // Skip `{{templates.X}}` — the regex already excludes those because
      // PATH_PLACEHOLDER_RX requires word-chars only inside the braces and
      // double-braces don't form a valid match. Belt-and-braces: also
      // skip if the surrounding chars are extra braces.
      if (!seen.has(name)) {
        seen.add(name);
        out.push(name);
      }
    }
    return out;
  }, [step.api_endpoint_path]);

  if (placeholders.length === 0) return null;

  const params = step.api_path_params ?? {};
  const setParam = (name: string, value: string) => {
    const next = { ...params };
    if (value) next[name] = value;
    else delete next[name];
    onChange({ api_path_params: Object.keys(next).length > 0 ? next : null });
  };

  // Live preview of the resolved path so the user sees what's about to be
  // sent. Tokens with no value stay as `{name}` (visually flagged red).
  const resolvedPath = (step.api_endpoint_path ?? '').replace(
    PATH_PLACEHOLDER_RX,
    (_full, name: string) => (params[name] ? params[name] : `{${name}}`),
  );
  const hasUnresolved = placeholders.some(n => !params[n]);

  return (
    <fieldset className="wf-apicall-params wf-apicall-path-params">
      <legend>
        🪪 {t('wf.apicall.pathParamsTitle')}
      </legend>
      <div className="wf-apicall-auth-hint">{t('wf.apicall.pathParamsHint')}</div>
      {placeholders.map(name => (
        <div key={name} className="wf-apicall-param-row">
          <span className="wf-apicall-param-key">{`{${name}}`}</span>
          <input
            type="text"
            value={params[name] ?? ''}
            onChange={e => setParam(name, e.target.value)}
            placeholder={t('wf.apicall.pathParamsPlaceholder', name)}
          />
        </div>
      ))}
      <div className={`wf-apicall-path-preview${hasUnresolved ? ' wf-apicall-path-preview-incomplete' : ''}`}>
        <span className="text-xs text-ghost">{t('wf.apicall.pathParamsPreview')}:</span>{' '}
        <code>{resolvedPath}</code>
      </div>
    </fieldset>
  );
}

// ─── AuthManagedSlots ─────────────────────────────────────────────────────
//
// Read-only display of the params/headers that the backend injects from
// the plugin's encrypted env at request build time. Two goals:
//
//   1. Reassure the user that the apikey they entered in Settings → APIs
//      is wired through — no need to re-paste it in the step's query.
//   2. Stop a hallucinating AI helper from polluting the step config with
//      `apikey: 'VOTRE_API_KEY'` placeholders. (The helper's `applyToStep`
//      strips these silently, but having the row visible here makes the
//      mental model obvious before the user even opens the bubble.)
function AuthManagedSlots({
  server,
  t,
}: {
  server: McpServer | null;
  t: (key: string, ...args: (string | number)[]) => string;
}) {
  const [reveal, setReveal] = useState(false);
  const slots = useMemo<AuthSlot[]>(() => authSlotsForServer(server), [server]);
  if (slots.length === 0) return null;

  return (
    <fieldset className="wf-apicall-params wf-apicall-auth-managed">
      <legend>
        <KeyRound size={11} style={{ marginRight: 4 }} />
        {t('wf.apicall.authManagedTitle')}
      </legend>
      <div className="wf-apicall-auth-hint">{t('wf.apicall.authManagedHint')}</div>
      {slots.map(slot => (
        <div key={`${slot.kind}:${slot.name}`} className="wf-apicall-param-row wf-apicall-auth-row">
          <span className="wf-apicall-param-key">
            {slot.name}
            <span className="wf-apicall-auth-kind">{slot.kind === 'query' ? 'query' : 'header'}</span>
          </span>
          <code className="wf-apicall-auth-value">
            {reveal ? `(env: ${slot.envKey})` : '••••••••'}
          </code>
          <button
            type="button"
            className="wf-apicall-auth-reveal"
            onClick={() => setReveal(v => !v)}
            title={t('wf.apicall.authManagedReveal')}
            aria-label={t('wf.apicall.authManagedReveal')}
          >
            {reveal ? '🙈' : '👁'}
          </button>
        </div>
      ))}
    </fieldset>
  );
}

// ─── KeyValueEditor (reusable for query + headers) ─────────────────────────

function KeyValueEditor({
  label,
  params,
  onChange,
  valuePlaceholder,
}: {
  label: string;
  params: Record<string, string>;
  onChange: (next: Record<string, string> | null) => void;
  valuePlaceholder: string;
}) {
  const entries = Object.entries(params);
  const [draftKey, setDraftKey] = useState('');
  const [draftValue, setDraftValue] = useState('');
  const addRow = () => {
    const key = draftKey.trim();
    if (!key) return;
    onChange({ ...params, [key]: draftValue });
    setDraftKey('');
    setDraftValue('');
  };
  // Auto-commit on focus-out — same rationale as `QueryParamsEditor`.
  const commitOnBlur = (e: React.FocusEvent<HTMLDivElement>) => {
    const next = e.relatedTarget as HTMLElement | null;
    if (next && e.currentTarget.contains(next)) return;
    if (draftKey.trim().length > 0) addRow();
  };
  const remove = (k: string) => {
    const next = { ...params };
    delete next[k];
    onChange(Object.keys(next).length > 0 ? next : null);
  };
  const update = (k: string, v: string) => onChange({ ...params, [k]: v });

  return (
    <fieldset className="wf-apicall-params">
      <legend>{label}</legend>
      {entries.map(([k, v]) => (
        <div key={k} className="wf-apicall-param-row">
          <span className="wf-apicall-param-key">{k}</span>
          <input type="text" value={v} onChange={e => update(k, e.target.value)} />
          <button type="button" onClick={() => remove(k)} aria-label={`Remove ${k}`}>×</button>
        </div>
      ))}
      <div className="wf-apicall-param-row" onBlur={commitOnBlur}>
        <input
          type="text"
          placeholder="key"
          value={draftKey}
          onChange={e => setDraftKey(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); addRow(); } }}
        />
        <input
          type="text"
          placeholder={valuePlaceholder}
          value={draftValue}
          onChange={e => setDraftValue(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); addRow(); } }}
        />
        <button type="button" onClick={addRow}>+</button>
      </div>
    </fieldset>
  );
}

// ─── BodyEditor (JSON textarea with parse-on-blur) ─────────────────────────

function BodyEditor({
  value,
  onChange,
  label,
}: {
  value: unknown;
  onChange: (next: unknown) => void;
  label: string;
}) {
  // Local draft so the user can type freely without losing focus on
  // every keystroke. We commit to the parent on blur, after JSON parse.
  const initial = value === null || value === undefined
    ? ''
    : JSON.stringify(value, null, 2);
  const [draft, setDraft] = useState(initial);
  const [parseError, setParseError] = useState<string | null>(null);

  // Re-sync local draft when the external value changes (template loaded,
  // step swapped). Cheap deep-compare via JSON.stringify is fine here —
  // body payloads are tiny.
  useEffect(() => {
    const next = value === null || value === undefined ? '' : JSON.stringify(value, null, 2);
    if (next !== draft) {
      setDraft(next);
      setParseError(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [JSON.stringify(value)]);

  const commit = () => {
    if (draft.trim().length === 0) {
      onChange(null);
      setParseError(null);
      return;
    }
    try {
      const parsed = JSON.parse(draft);
      setParseError(null);
      onChange(parsed);
    } catch (e) {
      setParseError(String(e));
    }
  };

  return (
    <fieldset className="wf-apicall-params wf-apicall-body-editor">
      <legend>{label}</legend>
      <textarea
        rows={6}
        value={draft}
        onChange={e => setDraft(e.target.value)}
        onBlur={commit}
        placeholder='{"key": "value"} — supporte {{steps.X.data}} dans les valeurs string'
        className="wf-apicall-body-textarea"
        spellCheck={false}
      />
      {parseError && (
        <div className="wf-apicall-preview-error">JSON invalide: {parseError}</div>
      )}
    </fieldset>
  );
}

// ─── QueryParamsEditor ─────────────────────────────────────────────────────

/** Common edit behaviour for the query + headers editors. The key
 *  invariant: a row that the user has filled in but not explicitly added
 *  via the `+` button gets committed automatically on blur. Without this
 *  auto-commit, ~50% of users hit "Test the call" without their params
 *  applied — exactly the trap that produced a stream of "Missing host"
 *  errors during early testing. */
function QueryParamsEditor({
  params,
  onChange,
  t,
}: {
  params: Record<string, string>;
  onChange: (next: Record<string, string> | null) => void;
  t: (k: string) => string;
}) {
  const entries = Object.entries(params);
  const [draftKey, setDraftKey] = useState('');
  const [draftValue, setDraftValue] = useState('');

  const addRow = () => {
    const key = draftKey.trim();
    if (!key) return;
    const next = { ...params, [key]: draftValue };
    onChange(next);
    setDraftKey('');
    setDraftValue('');
  };
  /** Best-effort commit when the user shifts focus elsewhere. Only fires
   *  when the key is non-empty AND focus moved outside the entire row.
   *  `relatedTarget` carries the next focus target — if it's still inside
   *  our row (e.g. user clicked from key to value), we skip. */
  const commitOnBlur = (e: React.FocusEvent<HTMLDivElement>) => {
    const next = e.relatedTarget as HTMLElement | null;
    if (next && e.currentTarget.contains(next)) return;
    if (draftKey.trim().length > 0) addRow();
  };
  const remove = (key: string) => {
    const next = { ...params };
    delete next[key];
    onChange(Object.keys(next).length > 0 ? next : null);
  };
  const update = (key: string, value: string) => onChange({ ...params, [key]: value });

  return (
    <fieldset className="wf-apicall-params">
      <legend>{t('wf.apicall.queryParams')}</legend>
      {entries.map(([k, v]) => (
        <div key={k} className="wf-apicall-param-row">
          <span className="wf-apicall-param-key">{k}</span>
          <input
            type="text"
            value={v}
            onChange={e => update(k, e.target.value)}
          />
          <button type="button" onClick={() => remove(k)} aria-label={`Remove ${k}`}>×</button>
        </div>
      ))}
      <div className="wf-apicall-param-row" onBlur={commitOnBlur}>
        <input
          type="text"
          placeholder="key"
          value={draftKey}
          onChange={e => setDraftKey(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); addRow(); } }}
        />
        <input
          type="text"
          placeholder="value (can use {{steps.X.data}})"
          value={draftValue}
          onChange={e => setDraftValue(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); addRow(); } }}
        />
        <button type="button" onClick={addRow}>+</button>
      </div>
    </fieldset>
  );
}

// ─── JsonTreeViewer ────────────────────────────────────────────────────────

function JsonTreeViewer({ data, onPick }: { data: unknown; onPick: (path: string) => void }) {
  return (
    <pre className="wf-apicall-json-tree" style={{ margin: 0 }}>
      <JsonNode value={data} pathSegments={[]} onPick={onPick} />
    </pre>
  );
}

function JsonNode({
  value,
  pathSegments,
  wildcardSegments,
  onPick,
}: {
  value: unknown;
  /** The exact path to this node, with concrete `[N]` array indices. Used
   *  when picking a leaf value or an explicit `[i]` row — you want the
   *  one specific item there. */
  pathSegments: string[];
  /** Same path but with the closest enclosing array's index replaced by
   *  `[*]`. Used when picking a KEY inside an array item — DWIM: most
   *  users clicking `path` inside `toppages[0]` want "tous les paths",
   *  not "the path of the first item only". When this prop is undefined
   *  (no enclosing array yet), it falls back to `pathSegments`. */
  wildcardSegments?: string[];
  onPick: (path: string) => void;
}) {
  const path = segmentsToJsonPath(pathSegments);

  if (value === null) {
    return <Leaf path={path} label="null" onPick={onPick} />;
  }
  if (typeof value === 'string') {
    return <Leaf path={path} label={`"${truncate(value, 60)}"`} onPick={onPick} />;
  }
  if (typeof value === 'number' || typeof value === 'boolean') {
    return <Leaf path={path} label={String(value)} onPick={onPick} />;
  }
  if (Array.isArray(value)) {
    // Trois cibles cliquables sur une array :
    //   - `[N]` (le marker total) → wildcard `$.foo[*]` (itérer sur tout)
    //   - `[i]` (l'index d'une row) → `$.foo[i]` (extraire le i-ième seul)
    //   - les feuilles internes → leur path concret
    // Les enfants reçoivent en plus un `wildcardSegments` pointant sur
    // CE niveau d'array, pour que cliquer une clé enfant produise un path
    // wildcard plutôt qu'indexé sur [0].
    const wildcardPath = path === '$' ? '$[*]' : `${path}[*]`;
    return (
      <span>
        <button
          type="button"
          className="wf-apicall-json-leaf wf-apicall-json-array-marker"
          onClick={() => onPick(wildcardPath)}
          title={wildcardPath}
        >
          [{value.length}]
        </button>
        {value.length === 0 ? null : (
          <div style={{ paddingLeft: 12 }}>
            {value.slice(0, 10).map((item, i) => {
              const itemPath = path === '$' ? `$[${i}]` : `${path}[${i}]`;
              const itemSegs = [...pathSegments, `[${i}]`];
              const itemWildSegs = [...pathSegments, '[*]'];
              return (
                <div key={i} className="wf-apicall-json-array-row">
                  <button
                    type="button"
                    className="wf-apicall-json-index"
                    onClick={() => onPick(itemPath)}
                    title={itemPath}
                  >
                    [{i}]
                  </button>{' '}
                  <JsonNode
                    value={item}
                    pathSegments={itemSegs}
                    wildcardSegments={itemWildSegs}
                    onPick={onPick}
                  />
                </div>
              );
            })}
            {value.length > 10 && <div style={{ opacity: 0.5 }}>… {value.length - 10} more</div>}
          </div>
        )}
      </span>
    );
  }
  if (typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>);
    // Wildcard segments propagate untouched through objects — only arrays
    // refresh them. So a key inside `toppages[0].nested` still uses
    // `toppages[*]` as the wildcard prefix.
    const wild = wildcardSegments ?? pathSegments;
    return (
      <span>
        {'{'}
        <div style={{ paddingLeft: 12 }}>
          {entries.map(([k, v]) => {
            const childWildSegs = [...wild, k];
            const childWildPath = segmentsToJsonPath(childWildSegs);
            return (
              <div key={k}>
                <button
                  type="button"
                  className="wf-apicall-json-key wf-apicall-json-key-btn"
                  onClick={() => onPick(childWildPath)}
                  title={childWildPath}
                >
                  {k}
                </button>
                {':'}{' '}
                <JsonNode
                  value={v}
                  pathSegments={[...pathSegments, k]}
                  wildcardSegments={childWildSegs}
                  onPick={onPick}
                />
              </div>
            );
          })}
        </div>
        {'}'}
      </span>
    );
  }
  return <span>{String(value)}</span>;
}

function Leaf({ path, label, onPick }: { path: string; label: string; onPick: (path: string) => void }) {
  return (
    <button
      type="button"
      className="wf-apicall-json-leaf"
      onClick={() => onPick(path)}
      title={path}
    >
      {label}
    </button>
  );
}

function segmentsToJsonPath(segments: string[]): string {
  if (segments.length === 0) return '$';
  let out = '$';
  for (const s of segments) {
    if (s.startsWith('[')) out += s; // array index
    else if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(s)) out += `.${s}`;
    else out += `['${s.replace(/'/g, "\\'")}']`;
  }
  return out;
}

function truncate(s: string, max: number): string {
  return s.length <= max ? s : s.slice(0, max) + '…';
}

// ─── ExtractPanel ─────────────────────────────────────────────────────────

function ExtractPanel({
  step,
  activeField,
  onActiveFieldChange,
  onChange,
  sample,
  nextStepType,
  t,
}: {
  step: WorkflowStep;
  activeField: ExtractField;
  onActiveFieldChange: (f: ExtractField) => void;
  onChange: (updates: Partial<WorkflowStep>) => void;
  sample: unknown;
  nextStepType?: StepType;
  t: (k: string, ...a: (string | number)[]) => string;
}) {
  const extract = step.api_extract ?? null;
  const path = activeField === 'data' ? (extract?.path ?? '') : '';

  // Debounced preview via /test-extract (no network when sample is null).
  const [preview, setPreview] = useState<{ value: unknown; value_type: string; is_empty: boolean; error: string | null } | null>(null);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    if (timer.current) window.clearTimeout(timer.current);
    if (!sample || !path || activeField !== 'data') {
      setPreview(null);
      return;
    }
    timer.current = window.setTimeout(() => {
      workflowsApi.testExtract({ sample, path }).then(setPreview).catch(() => setPreview(null));
    }, 150);
    return () => { if (timer.current) window.clearTimeout(timer.current); };
  }, [sample, path, activeField]);

  const updatePath = (newPath: string) => {
    const nextExtract: ExtractSpec = {
      path: newPath,
      fallback: extract?.fallback ?? null,
      fail_on_empty: extract?.fail_on_empty ?? false,
    };
    onChange({ api_extract: nextExtract });
  };

  const applyExample = (exPath: string) => updatePath(exPath);

  // Auto-derived path suggestions — only on the `data` field where they
  // make sense. The list is built fresh whenever the response changes; a
  // tiny pulse on the path input lets the user see when a click landed.
  const suggestions = useMemo(() => suggestPaths(sample), [sample]);
  const [justPicked, setJustPicked] = useState(false);
  useEffect(() => {
    if (!path) return;
    setJustPicked(true);
    const id = window.setTimeout(() => setJustPicked(false), 600);
    return () => window.clearTimeout(id);
  }, [path]);

  return (
    <div className="wf-apicall-extract">
      <div className="wf-apicall-section-title">{t('wf.apicall.extractSection')}</div>
      <div className="text-xs text-ghost" style={{ marginBottom: 6 }}>
        {t('wf.apicall.extractSectionHint')}
      </div>

      {/* 3 radios for which envelope field we're editing. MVP: only
          `data` is wired — status/summary are always auto-computed by
          the backend from the extracted data + request. Radios shown
          for future parity + to set visual expectations. */}
      <div className="wf-apicall-radios" role="radiogroup">
        {(['data', 'status', 'summary'] as const).map(f => (
          <label key={f} className="wf-apicall-radio">
            <input
              type="radio"
              name="extract-field"
              checked={activeField === f}
              onChange={() => onActiveFieldChange(f)}
            />
            {t(`wf.apicall.extract${capitalize(f)}` as 'wf.apicall.extractData' | 'wf.apicall.extractStatus' | 'wf.apicall.extractSummary')}
          </label>
        ))}
      </div>

      {activeField === 'data' && (
        <>
          {suggestions.length > 0 && (
            <div className="wf-apicall-suggest" role="group" aria-label={t('wf.apicall.suggest.title')}>
              <span className="wf-apicall-suggest-title">💡 {t('wf.apicall.suggest.title')}</span>
              {suggestions.map(s => (
                <SuggestChip key={s.path} suggestion={s} onPick={updatePath} t={t} />
              ))}
            </div>
          )}

          <label className="wf-apicall-field">
            <span>{t('wf.apicall.pathLabel')}</span>
            <input
              type="text"
              className={justPicked ? 'wf-apicall-path-pulse' : undefined}
              placeholder={t('wf.apicall.pathPlaceholder')}
              value={path}
              onChange={e => updatePath(e.target.value)}
            />
          </label>

          <div className="wf-apicall-examples">
            <span className="text-xs text-ghost">{t('wf.apicall.examples')}:</span>
            <button type="button" onClick={() => applyExample('$.*[*].id')}>
              {t('wf.apicall.exampleAllIds')}
            </button>
            <button type="button" onClick={() => applyExample('$.[0]')}>
              {t('wf.apicall.exampleFirst')}
            </button>
            <button type="button" onClick={() => applyExample('$.total')}>
              {t('wf.apicall.exampleCount')}
            </button>
          </div>

          <div className="wf-apicall-preview">
            <span className="text-xs text-ghost">{t('wf.apicall.preview')}:</span>
            {preview?.error ? (
              <span className="wf-apicall-preview-error">{preview.error}</span>
            ) : preview == null ? (
              <span className="text-ghost text-xs">—</span>
            ) : (
              <>
                <code className="wf-apicall-preview-value">{previewString(preview.value)}</code>
                <span className="wf-apicall-preview-type">{preview.value_type}</span>
              </>
            )}
          </div>

          <label className="wf-apicall-checkbox">
            <input
              type="checkbox"
              checked={extract?.fail_on_empty ?? false}
              onChange={e => {
                const next: ExtractSpec = {
                  path: extract?.path ?? '',
                  fallback: extract?.fallback ?? null,
                  fail_on_empty: e.target.checked,
                };
                onChange({ api_extract: next });
              }}
            />
            <span>{t('wf.apicall.failOnEmpty')}</span>
          </label>
          <small className="text-xs text-ghost">{t('wf.apicall.failOnEmptyHint')}</small>
        </>
      )}

      {activeField !== 'data' && (
        <div className="text-xs text-muted" style={{ padding: 10 }}>
          {/* status / summary are auto-computed — no editable path for them. */}
          —
        </div>
      )}

      <NextStepBanner
        nextStepType={nextStepType}
        preview={preview}
        t={t}
      />
    </div>
  );
}

// ─── SuggestChip ───────────────────────────────────────────────────────
//
// One auto-derived JSONPath suggestion rendered as a clickable chip. The
// algorithm lives in `apiCallSuggestions.ts`; this component just renders
// the result with the human label (translated) + a sample preview so the
// user spots "the right one" without clicking through every option.
function SuggestChip({
  suggestion,
  onPick,
  t,
}: {
  suggestion: PathSuggestion;
  onPick: (path: string) => void;
  t: (k: string, ...a: (string | number)[]) => string;
}) {
  const label = t(suggestion.i18nKey as 'wf.apicall.suggest.iterate', ...suggestion.args);
  return (
    <button
      type="button"
      className="wf-apicall-suggest-chip"
      onClick={() => onPick(suggestion.path)}
      title={suggestion.path}
    >
      <span className="wf-apicall-suggest-chip-label">{label}</span>
      <code className="wf-apicall-suggest-chip-sample">{suggestion.sample}</code>
    </button>
  );
}

// ─── NextStepBanner ──────────────────────────────────────────────────────
//
// The wizard's `validation bandeau`. Fires only when the next step is a
// BatchQuickPrompt — that's the one that will silently break ("1 item
// would be launched, with the literal string") if `data` isn't an array.
// Other downstream step types accept any shape and stay quiet.

function NextStepBanner({
  nextStepType,
  preview,
  t,
}: {
  nextStepType?: StepType;
  preview: { value: unknown; value_type: string; is_empty: boolean; error: string | null } | null;
  t: (k: string, ...a: (string | number)[]) => string;
}) {
  // Only validate when the next step is a batch fan-out. Agent / Notify /
  // another ApiCall accept any shape — no banner to avoid noise.
  if (nextStepType?.type !== 'BatchQuickPrompt') return null;

  // No preview yet (never ran a test, or path empty) → skip. The wizard
  // shows a generic empty-state elsewhere; we don't want to double-warn.
  if (!preview) return null;

  if (preview.error) {
    // Invalid JSONPath — the extract preview already shows the error
    // inline. No need to reflect it here, keeps the banner focused on
    // the type-mismatch concern.
    return null;
  }

  const isArray = preview.value_type.startsWith('array');
  if (isArray) {
    return (
      <div className="wf-apicall-banner wf-apicall-banner-ok" role="status">
        {t('wf.apicall.nextStepBatchOk')}
      </div>
    );
  }
  return (
    <div className="wf-apicall-banner wf-apicall-banner-warn" role="alert">
      {t('wf.apicall.nextStepBatchMismatch', preview.value_type)}
    </div>
  );
}

function applyPathToActiveField(
  step: WorkflowStep,
  onChange: (u: Partial<WorkflowStep>) => void,
  field: ExtractField,
  path: string,
) {
  if (field !== 'data') return;
  const next: ExtractSpec = {
    path,
    fallback: step.api_extract?.fallback ?? null,
    fail_on_empty: step.api_extract?.fail_on_empty ?? false,
  };
  onChange({ api_extract: next });
}

function capitalize(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/** Render a JSONPath-resolved value as a one-line preview chip.
 *
 *  Goes one level deep so the user sees actual content for arrays/objects
 *  (`["fr.euronews.com/", "fr.euronews.com/voya…", … (+3)]`) instead of
 *  the previous `Array(5)` placeholder that conveyed nothing. Depth is
 *  capped at 1 to keep the chip readable on narrow viewports — a deeper
 *  inspection is what the JSON tree on the left is for. Exported for
 *  unit tests; otherwise component-private. */
export function previewString(v: unknown, depth = 0): string {
  if (v == null) return 'null';
  if (typeof v === 'string') {
    return `"${truncate(v, depth === 0 ? 60 : 30)}"`;
  }
  if (typeof v === 'number' || typeof v === 'boolean') return String(v);
  if (Array.isArray(v)) {
    if (v.length === 0) return '[]';
    if (depth >= 1) return `[${v.length}]`;
    const items = v.slice(0, 3).map(x => previewString(x, depth + 1));
    const more = v.length > 3 ? `, … (+${v.length - 3})` : '';
    return `[${items.join(', ')}${more}]`;
  }
  if (typeof v === 'object') {
    const entries = Object.entries(v as Record<string, unknown>);
    if (entries.length === 0) return '{}';
    if (depth >= 1) return '{…}';
    const items = entries.slice(0, 3).map(([k, val]) => `${k}: ${previewString(val, depth + 1)}`);
    const more = entries.length > 3 ? `, … (+${entries.length - 3})` : '';
    return `{${items.join(', ')}${more}}`;
  }
  return String(v);
}

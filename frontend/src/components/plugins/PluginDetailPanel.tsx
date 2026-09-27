import {
  Puzzle, Pencil, X, Trash2, Upload, Info, Plug, Check, Minus, RefreshCw,
  Sparkles, Key, Terminal, ExternalLink, Save, Eye, AlertTriangle,
  CheckSquare, Square, FileText, Globe,
} from 'lucide-react';
import type { McpConfigDisplay, McpServer, PluginInterface } from '../../types/generated';
import { linkify } from '../../lib/linkify';
import { isHiddenPath } from '../../lib/constants';
import { pluginCredentialKeys } from '../../lib/pluginCredentials';
import { HostSyncPreview } from '../HostSyncPreview';
import { CustomApiForm } from './CustomApiForm';
import { hasAgentScope, slugify } from './mcpPageHelpers';
import type { McpPageState } from './useMcpPageState';

const PROJECT_TOGGLE_LIMIT = 10;

/**
 * Plugin detail/edit side panel — the "fiche" for one installed config.
 * Rendered inline by `PluginCollectionView`'s `renderDetail` slot.
 * Extracted verbatim from the pre-KT-830 `McpPage` render (the `detail`
 * branch of `renderPlugin`); the `card` (list-row) branch was dead code
 * (only ever called with `renderDetail=true`) and was dropped, not moved.
 */
export function PluginDetailPanel({ cfg, state }: { cfg: McpConfigDisplay; state: McpPageState }) {
  const {
    t, projects, mcpOverview, mcpRegistry,
    probeByConfig, probingConfigId, handleProbeConfig,
    handleSetPreferredInterface,
    editingLabelId, editingLabelText, setEditingLabelId, setEditingLabelText, handleSaveLabel,
    editingCustomServerId,
    setEditingCustomServerId, setEditingCustomConfigId,
    setCustomName, setCustomBaseUrl, setCustomDescription, setCustomDocsUrl,
    setCustomFields, setReplacingFields, setCustomEndpoints, setCustomAuth,
    handleExportCustomPlugin, handleDeleteMcpConfig, setSelectedConfigId, resetAddMcp,
    editingEnvId, setEditingEnvId, editingEnv, setEditingEnv, editingEnvLoading, editingEnvError, visibleFields, setVisibleFields,
    handleStartEditSecrets, handleSaveSecrets, toggleFieldVisibility,
    handleToggleConfigGlobal, handleToggleConfigGeneral, handleToggleConfigProject, handleSetHostSync,
    expandedProjectLists, setExpandedProjectLists, handleOpenContext,
  } = state;

  // KT-828 — `effective_kind` is computed once server-side so it can't
  // drift from what the agent actually uses; hides host-sync UI on
  // API-only plugins (they're injected into prompts, never written to
  // ~/.claude.json & co — showing a "Sync CLI" toggle on them was a UX bug).
  const cfgServer = mcpOverview.servers.find(s => s.id === cfg.server_id);
  const cfgKind = cfg.effective_kind;
  const supportsHostSync = cfgKind !== 'api';
  const hasVisibleScope = hasAgentScope(
    cfg.is_global,
    cfg.include_general,
    cfg.project_ids,
  );

  const def = mcpRegistry.find(m => m.id === cfg.server_id);
  const isEditingLabel = editingLabelId === cfg.id;
  // Refonte 2b — this plugin's spec+credentials are being edited
  // IN the modal (body swaps to the hoisted customApiForm).
  const isEditingThisCustom = editingCustomServerId === cfg.server_id;
  // Closing the modal (backdrop / X) always cancels an in-flight
  // edit so stale form state can't leak into the next open.
  const closePluginModal = () => {
    if (editingCustomServerId) resetAddMcp();
    setSelectedConfigId(null);
  };
  const serverIncomp = mcpOverview.incompatibilities.filter(i => i.server_id === cfg.server_id);
  const probeResult = probeByConfig[cfg.id];
  const isProbing = probingConfigId === cfg.id;
  // KT-828 — both computed once server-side (`registry::
  // available_plugin_interfaces` / the `preferred_interface` clamp).
  const availableInterfaces: PluginInterface[] = cfg.interfaces;
  const effectivePreferredInterface = cfg.effective_preferred_interface;

  // 0.8.6 (#29) — open the edit form pre-filled with the
  // current Custom plugin's spec. Shared between the Edit
  // button (header) and the autodiscovery banner (body)
  // that surfaces on plugins with empty endpoints.
  const openEditCustomPlugin = async () => {
    if (!cfgServer?.api_spec) return;
    const spec = cfgServer.api_spec;
    setEditingCustomServerId(cfg.server_id);
    setEditingCustomConfigId(cfg.id);
    setCustomName(cfgServer.name);
    setCustomBaseUrl(spec.base_url);
    setCustomDescription(cfgServer.description);
    setCustomDocsUrl(spec.docs_url ?? '');
    // 2026-06-09 — never pre-fill secret values (a masked
    // `revealSecrets` pre-fill can't round-trip into a real key
    // → the desync bug). Instead, mark fields whose env_key is
    // already stored in THIS config: they render as a read-only
    // masked "🔒 enregistrée + Remplacer" affordance (see render),
    // so the user always sees a key exists and an empty field
    // never looks like a wipe. value:'' = "keep unless replaced".
    const ck = spec.config_keys ?? [];
    const storedKeys = new Set(cfg.env_keys ?? []);
    setCustomFields(
      ck.length > 0
        ? ck.map(k => ({ label: k.label, value: '', stored: storedKeys.has(k.env_key) }))
        : [{ label: '', value: '' }],
    );
    setReplacingFields(new Set());
    setCustomEndpoints(spec.endpoints ?? []);
    setCustomAuth(spec.auth ?? 'None');
    // Refonte 2b (2026-06-10) — the edit form renders IN-PLACE
    // inside this modal (see mcp-detail-body below). No more
    // jumping to the top Add panel + scroll-to-top: the modal
    // stays open, the body swaps view → form.
  };

  // 0.8.6 (#29) — surface a banner on legacy Custom plugins
  // (created before 0.8.6, OR after but with no endpoints
  // declared yet) prompting the user to ask the AI helper
  // to fill them. Detection : the plugin's `server_id`
  // starts with `custom-` AND the api_spec has zero
  // declared endpoints. Banner CTA reuses
  // `openEditCustomPlugin` so the AI helper is one click
  // away (cf. [[project_endpoints_autodiscovery_0_8_6]]).
  const isLegacyCustomNoEndpoints =
    cfg.server_id.startsWith('custom-')
    && cfgServer?.api_spec
    && (cfgServer.api_spec.endpoints?.length ?? 0) === 0;

  return (
    <aside
      key={`detail-${cfg.id}`}
      className="mcp-detail-inline"
      aria-label={cfg.label}
      data-testid="mcp-plugin-detail"
    >
      <div className="mcp-detail-header collection-detail-header">
        <div className="mcp-registry-card-icon" style={{ width: 40, height: 40 }}><Puzzle size={20} /></div>
        <div className="flex-1">
          {isEditingLabel ? (
            <input className="input mcp-detail-name-input" value={editingLabelText} onChange={e => setEditingLabelText(e.target.value)} onBlur={() => handleSaveLabel(cfg.id)} onKeyDown={e => { if (e.key === 'Enter') handleSaveLabel(cfg.id); if (e.key === 'Escape') setEditingLabelId(null); }} autoFocus />
          ) : (
            <h2 className="mcp-detail-name" onClick={() => { setEditingLabelId(cfg.id); setEditingLabelText(cfg.label); }}>{cfg.label} <Pencil size={11} className="text-ghost" /></h2>
          )}
          {def?.description && <p className="mcp-detail-desc">{def.description}</p>}
          {def && <span className={`mcp-origin-badge ${def.official ? 'mcp-origin-official' : 'mcp-origin-community'}`}>
            {def.official ? t('mcp.official') : t('mcp.community')} — {def.publisher}
          </span>}
          {serverIncomp.length > 0 && <span className="mcp-server-incompat">{serverIncomp.map(i => `⚠ ${i.agent}: ${i.reason}`).join(' · ')}</span>}
        </div>
        <div className="flex-row gap-3">
          {/* Refonte 2b — header actions hidden while EDITING:
              the body holds the form (with its own Save/Back);
              a bare header + X keeps the edit surface focused. */}
          {!isEditingThisCustom && (
            <>
              {/* Edit spec+credentials button. Only on Custom API
                  plugins (`custom-{slug}-{nano}` ids). Swaps the
                  modal body to the same form as create, pre-filled
                  from the server's api_spec — IN PLACE. */}
              {cfg.server_id.startsWith('custom-') && cfgServer?.api_spec && (
                <button
                  className="mcp-btn-action"
                  onClick={openEditCustomPlugin}
                  title={t('mcp.custom.editSpec')}
                >
                  <Pencil size={12} /> {t('mcp.custom.editSpec')}
                </button>
              )}
              {/* 0.8.6 (#33) — Export as JSON. Spec-only, no
                  credentials. Sharing the resulting payload is
                  safe. */}
              {cfg.server_id.startsWith('custom-') && cfgServer?.api_spec && (
                <button
                  className="mcp-btn-action"
                  onClick={() => handleExportCustomPlugin(cfgServer as McpServer)}
                  title={t('mcp.custom.copyAsJson')}
                  data-testid="mcp-custom-export-json"
                >
                  <Upload size={12} /> {t('mcp.custom.copyAsJson')}
                </button>
              )}
              <button className="mcp-btn-action" style={{ color: 'var(--kr-error)', borderColor: 'rgba(var(--kr-error-rgb), 0.3)' }} onClick={() => { handleDeleteMcpConfig(cfg.id); setSelectedConfigId(null); }}><Trash2 size={12} /> {t('mcp.deleteConfig')}</button>
            </>
          )}
          <button className="mcp-icon-btn" onClick={closePluginModal} aria-label={t('common.close')}><X size={14} /></button>
        </div>
      </div>
      <div className="mcp-detail-body">
        {/* Refonte 2b — EDIT mode renders the hoisted custom
            form IN PLACE of the view body. ISO with MCP env
            editing: everything happens here, no jump to the
            top Add panel, no scroll. */}
        {isEditingThisCustom ? <CustomApiForm state={state} /> : (<>
        {cfg.registry_drift && (
          <div className="mcp-registry-drift" role="status" data-testid="mcp-registry-drift">
            <Info size={16} aria-hidden="true" />
            <div>
              <strong>
                {t(cfg.registry_drift.orphaned ? 'mcp.registryOrphanTitle' : 'mcp.registryDriftTitle')}
              </strong>
              <p>
                {cfg.registry_drift.orphaned
                  ? t('mcp.registryOrphanBody', cfg.server_id, cfg.registry_drift.replacement_server_id ?? t('mcp.registryNoReplacement'))
                  : t('mcp.registryDriftBody')}
              </p>
              <dl>
                <div>
                  <dt>{t('mcp.registryStoredKeys')}</dt>
                  <dd>{cfg.registry_drift.stored_env_keys.join(', ') || '—'}</dd>
                </div>
                <div>
                  <dt>{t('mcp.registryExpectedKeys')}</dt>
                  <dd>{cfg.registry_drift.expected_env_keys.join(', ') || '—'}</dd>
                </div>
              </dl>
              <small>{t('mcp.registryNoSilentRemap')}</small>
            </div>
          </div>
        )}
        <section className="mcp-detail-section mcp-interface-section">
          <h3 className="mcp-detail-section-title">
            <Plug size={12} /> {t('mcp.interfaces')}
          </h3>
          <p className="mcp-interface-hint">{t('mcp.interfacesHint')}</p>
          <div className="mcp-interface-availability" aria-label={t('mcp.availableInterfaces')}>
            {(['api', 'mcp', 'cli'] as PluginInterface[]).map(pluginInterface => {
              const available = availableInterfaces.includes(pluginInterface);
              return (
                <span
                  key={pluginInterface}
                  className={`mcp-interface-chip${available ? ' mcp-interface-chip-on' : ''}`}
                  data-interface={pluginInterface}
                  data-available={available}
                >
                  {available ? <Check size={11} /> : <Minus size={11} />}
                  {t(`mcp.interface.${pluginInterface}`)}
                </span>
              );
            })}
          </div>
          <label className="mcp-interface-preference">
            <span>{t('mcp.preferredInterface')}</span>
            <select
              className="input"
              value={effectivePreferredInterface}
              onChange={event => handleSetPreferredInterface(
                cfg.id,
                event.target.value as PluginInterface,
              )}
              disabled={availableInterfaces.length < 2}
              data-testid="mcp-preferred-interface"
            >
              {availableInterfaces.map(pluginInterface => (
                <option key={pluginInterface} value={pluginInterface}>
                  {t(`mcp.interface.${pluginInterface}`)}
                </option>
              ))}
            </select>
          </label>
          <p className="mcp-interface-rule">
            {t('mcp.preferredInterfaceRule', t(`mcp.interface.${effectivePreferredInterface}`))}
          </p>
        </section>
        <section className="mcp-detail-section mcp-probe-section" data-testid="mcp-plugin-probe">
          <div className="mcp-probe-heading">
            <div>
              <h3 className="mcp-detail-section-title">
                <RefreshCw size={12} /> {t('mcp.diagnostics')}
              </h3>
              <p className="mcp-probe-hint">{t('mcp.diagnosticsHint')}</p>
            </div>
            <div className="mcp-probe-actions">
              {probeResult && (
                <span
                  className={`mcp-probe-status ${probeResult.ready ? 'mcp-probe-status-ready' : 'mcp-probe-status-failed'}`}
                  data-testid="mcp-probe-status"
                >
                  {probeResult.ready ? <Check size={11} /> : <X size={11} />}
                  {t(probeResult.ready ? 'mcp.ready' : 'mcp.notReady')}
                </span>
              )}
              <button
                type="button"
                className="mcp-btn-action"
                onClick={() => handleProbeConfig(cfg.id)}
                disabled={isProbing}
                data-testid="mcp-probe-button"
              >
                <RefreshCw size={12} className={isProbing ? 'spin' : undefined} />
                {t(isProbing ? 'mcp.probing' : 'mcp.runProbe')}
              </button>
            </div>
          </div>
          {probeResult && (
            <div className="mcp-probe-checks">
              {probeResult.checks.map(check => {
                const labelKey = `mcp.probeCheck.${check.id}`;
                const translatedLabel = t(labelKey);
                const checkState = check.ok ? 'ready' : check.required ? 'failed' : 'optional';
                return (
                  <div className="mcp-probe-check" data-state={checkState} key={check.id}>
                    <span className="mcp-probe-check-icon">
                      {check.ok
                        ? <Check size={12} />
                        : check.required
                          ? <X size={12} />
                          : <Minus size={12} />}
                    </span>
                    <div>
                      <strong>
                        {translatedLabel === labelKey ? check.label : translatedLabel}
                        {!check.required && <small>{t('mcp.optional')}</small>}
                      </strong>
                      <span>{check.detail}</span>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </section>
        {/* 0.8.6 (#29) — autodiscovery banner for legacy
            Custom plugins with no endpoints declared. CTA
            opens the same edit form as the header button,
            where the CustomApiAiHelper (0.8.6 Part B) is
            wired to fetch the docs_url + propose endpoints
            via KRONN:APPLY. */}
        {isLegacyCustomNoEndpoints && (
          <div className="mcp-autodiscovery-banner" data-testid="mcp-autodiscovery-banner">
            <Info size={14} className="mcp-autodiscovery-banner-icon" />
            <div className="mcp-autodiscovery-banner-body">
              <strong>{t('mcp.custom.autodiscoveryTitle')}</strong>
              <p>{t('mcp.custom.autodiscoveryHint')}</p>
            </div>
            <button
              type="button"
              className="mcp-btn-action mcp-autodiscovery-banner-cta"
              onClick={openEditCustomPlugin}
            >
              <Sparkles size={12} /> {t('mcp.custom.autodiscoveryCta')}
            </button>
          </div>
        )}
        {(() => {
          // 0.8.6 — for Custom plugins, the SPEC's config_keys is
          // the forward-looking source of truth (follows rename via
          // Edit plugin). The stored `cfg.env_keys` may still
          // carry orphan slugs from before a rename — surfacing
          // them as the editable list would re-create them on
          // save and confuse the user.  Registry plugins always
          // agree (spec = env), so this is a no-op for them.
          const isCustom = cfg.server_id.startsWith('custom-');
          // 0.8.6 unified-edit (2026-05-20) — Custom plugins
          // get a READ-ONLY env section (slugs + eye reveal,
          // no edit button). The actual editing lives in
          // "Modifier le plugin". User asked 2026-05-20 to
          // be able to SEE stored values without entering
          // edit mode ("Au pire sur la card de l'API on peut
          // toujours afficher les variables, avec le petit
          // oeil, SANS l'édition"). Registry plugins keep
          // the editable section (their only env path).
          if (isCustom) {
            if (cfg.env_keys.length === 0) return null;
            return (
              <div className="mcp-detail-section">
                <h3 className="mcp-detail-section-title">
                  <Key size={12} /> {t('mcp.envVars')}
                </h3>
                <p className="mcp-env-key-desc mb-3" style={{ fontSize: '0.85em' }}>
                  {t('mcp.custom.envViewOnlyHint')}
                </p>
                {cfg.env_keys.map(k => (
                  <div key={k} className="mcp-detail-field">
                    <label className="mcp-detail-field-label">{k}</label>
                    <div className="flex-row gap-3">
                      <input
                        className="input mcp-input-mono flex-1"
                        value={editingEnvId === cfg.id ? (editingEnv[k] ?? '') : '••••••••'}
                        type={editingEnvId === cfg.id && visibleFields.has(k) ? 'text' : 'password'}
                        readOnly
                        onChange={() => {}}
                      />
                      <button
                        className="mcp-icon-btn"
                        onClick={async () => {
                          if (editingEnvId !== cfg.id) {
                            const ok = await handleStartEditSecrets(cfg.id);
                            if (!ok) return;
                            setVisibleFields(prev => new Set(prev).add(k));
                          } else {
                            toggleFieldVisibility(k);
                          }
                        }}
                        title={visibleFields.has(k) ? t('mcp.hide') : t('mcp.show')}
                      >
                        <Eye size={12} style={{ color: visibleFields.has(k) ? 'var(--kr-accent-ink)' : 'var(--kr-text-ghost)' }} />
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            );
          }
          // KT-821/KT-828 — driven by the backend's `credential_source`
          // (derived from `ApiAuthKind`), not the registry's `cli` tag: a
          // `CliToken` auth (Microsoft 365, no `cli` tag; Fastly, tagged)
          // never stores an API-authenticating value, so both get the
          // "not stored, CLI-resolved" copy instead of "used by the API".
          const credentialKind = cfg.credential_source === 'cli_token' ? 'cli' : 'api';
          const displayEnvKeys = pluginCredentialKeys(def, cfg.env_keys);
          const hasAnything = displayEnvKeys.length > 0 || def?.token_help;
          return hasAnything ? (
          <div className="mcp-detail-section mcp-credential-fields" data-kind={credentialKind}>
            <h3 className="mcp-detail-section-title">
              {credentialKind === 'cli' ? <Terminal size={12} /> : <Key size={12} />}
              {t(credentialKind === 'cli' ? 'mcp.credentials.cliTitle' : 'mcp.credentials.apiTitle')}
              {displayEnvKeys.length > 0 && editingEnvId !== cfg.id && <button className="mcp-icon-btn" style={{ marginLeft: 4 }} onClick={() => handleStartEditSecrets(cfg.id)} title={t('mcp.editKeys')} aria-label={t('mcp.editKeys')}><Pencil size={11} style={{ color: 'var(--kr-text-dim)' }} /></button>}
            </h3>
            <div className="mcp-credential-explainer" data-kind={credentialKind} role="note">
              {credentialKind === 'cli' ? <Terminal size={14} /> : <Key size={14} />}
              <span>
                <strong>{t(credentialKind === 'cli' ? 'mcp.credentials.cliRecommended' : 'mcp.credentials.apiUsedByKronn')}</strong>
                <small>{t(credentialKind === 'cli' ? 'mcp.credentials.cliOptionalHint' : 'mcp.credentials.apiHint')}</small>
              </span>
            </div>
            {def?.token_help && (() => {
              const helpKey = `mcp.help.${def.id}`;
              const translated = t(helpKey);
              const helpText = translated !== helpKey ? translated : def.token_help;
              return <p className="mcp-credential-help" style={{ whiteSpace: 'pre-wrap' }}>{linkify(helpText)}</p>;
            })()}
            {def?.token_url && <a href={def.token_url} target="_blank" rel="noopener noreferrer" className="mcp-secrets-token-link mb-4"><ExternalLink size={10} /> {t('mcp.getToken')}</a>}
            {displayEnvKeys.map(k => {
              const stored = cfg.env_keys.includes(k);
              return (
                <div key={k} className="mcp-detail-field">
                  <label className="mcp-detail-field-label">
                    {k}
                  </label>
                  <div className="flex-row gap-3">
                    <input className="input mcp-input-mono flex-1" value={editingEnvId === cfg.id ? (editingEnv[k] ?? '') : stored ? '••••••••' : ''} onChange={e => setEditingEnv(prev => ({ ...prev, [k]: e.target.value }))} type={editingEnvId === cfg.id && visibleFields.has(k) ? 'text' : 'password'} placeholder={stored ? t('mcp.value') : t('mcp.credentials.notStored')} readOnly={editingEnvId !== cfg.id} onClick={() => { if (editingEnvId !== cfg.id) handleStartEditSecrets(cfg.id); }} />
                    <button className="mcp-icon-btn" onClick={async () => { if (editingEnvId !== cfg.id) { const ok = await handleStartEditSecrets(cfg.id); if (!ok) return; setVisibleFields(prev => new Set(prev).add(k)); } else { toggleFieldVisibility(k); } }} title={visibleFields.has(k) ? t('mcp.hide') : t('mcp.show')}><Eye size={12} style={{ color: visibleFields.has(k) ? 'var(--kr-accent-ink)' : 'var(--kr-text-ghost)' }} /></button>
                  </div>
                </div>
              );
            })}
            {editingEnvError && editingEnvId === cfg.id && (
              <div className="mcp-env-warning" style={{ color: 'var(--kr-warning)', fontSize: '0.8rem', marginTop: 6 }}>{editingEnvError}</div>
            )}
            {editingEnvId === cfg.id && (
              <div className="flex-row gap-3 mt-4">
                <button className="mcp-btn-action mcp-btn-action-primary" onClick={handleSaveSecrets} disabled={editingEnvLoading}><Save size={12} /> {editingEnvLoading ? t('mcp.saving') : t('mcp.save')}</button>
                <button className="mcp-btn-action" onClick={() => setEditingEnvId(null)}>{t('mcp.cancel')}</button>
              </div>
            )}
          </div>
          ) : null;
        })()}
        <div className="mcp-detail-section">
          <h3 className="mcp-detail-section-title">{t('mcp.scope')}</h3>
          {!hasVisibleScope && (
            <div className="mcp-scope-orphan-warning" role="alert">
              <AlertTriangle size={16} aria-hidden="true" />
              <span>
                <strong>{t('mcp.scopeOrphanTitle')}</strong>
                <small>{t('mcp.scopeOrphanBody')}</small>
              </span>
              <button
                type="button"
                onClick={() => void handleToggleConfigGeneral(cfg)}
              >
                {t('mcp.scopeRepairGeneral')}
              </button>
            </div>
          )}
          <div className="mcp-toggle-row">
            <span className={`mcp-toggle-label mcp-toggle-global${cfg.is_global ? ' mcp-toggle-global-active' : ''}`} onClick={() => handleToggleConfigGlobal(cfg)} title={cfg.is_global ? t('mcp.disableGlobal') : t('mcp.enableGlobal')}>{t('mcp.scope.globalBadge')}</span>
            <span className={`mcp-toggle-label mcp-toggle-general${cfg.include_general ? ' mcp-toggle-general-active' : ''}`} onClick={() => void handleToggleConfigGeneral(cfg)} title={cfg.include_general ? t('mcp.disableGeneral') : t('mcp.enableGeneral')}>{t('mcp.general')}</span>
          </div>
          <div className="mcp-toggle-row">
            {(() => {
              const sorted = projects.filter(p => !isHiddenPath(p.path)).sort((a, b) => {
                const aL = (cfg.is_global || cfg.project_ids.includes(a.id)) ? 0 : 1;
                const bL = (cfg.is_global || cfg.project_ids.includes(b.id)) ? 0 : 1;
                return aL - bL || a.name.localeCompare(b.name);
              });
              const showAll = expandedProjectLists.has(cfg.id);
              const visible = showAll ? sorted : sorted.slice(0, PROJECT_TOGGLE_LIMIT);
              const hiddenCount = sorted.length - visible.length;
              return (<>
                {visible.map(proj => {
                  const isLinked = cfg.is_global || cfg.project_ids.includes(proj.id);
                  const projMcpCount = mcpOverview.configs.filter(c => c.is_global || c.project_ids.includes(proj.id)).length;
                  const loadClass = projMcpCount <= 5 ? 'mcp-load-ok' : projMcpCount <= 10 ? 'mcp-load-warn' : 'mcp-load-danger';
                  const loadTitle = projMcpCount <= 5 ? t('mcp.mcpLoadOk') : projMcpCount <= 10 ? t('mcp.mcpLoadWarn') : t('mcp.mcpLoadDanger');
                  return (
                    <span key={proj.id} className="flex-row">
                      <button className={`mcp-project-toggle ${isLinked ? 'mcp-project-toggle-on' : 'mcp-project-toggle-off'}`} onClick={() => handleToggleConfigProject(cfg.id, proj.id, isLinked)}>
                        {isLinked ? <CheckSquare size={11} className="text-accent" /> : <Square size={11} />}
                        {proj.name}
                        <span className={`mcp-load-badge ${loadClass}`} title={loadTitle}>{projMcpCount}</span>
                      </button>
                      {isLinked && (() => {
                        const slug = slugify(cfg.label);
                        const isCustom = mcpOverview.customized_contexts.includes(`${slug}:${proj.id}`);
                        return <button className="mcp-icon-btn mcp-context-btn" onClick={() => handleOpenContext(proj.id, proj.name, cfg.label)} title={`${t('mcp.editContext', cfg.label, proj.name)}${isCustom ? ' ' + t('mcp.customized') : ' ' + t('mcp.default')}`}><FileText size={10} style={{ color: isCustom ? 'var(--kr-accent)' : 'var(--kr-text-ghost)' }} /></button>;
                      })()}
                    </span>
                  );
                })}
                {hiddenCount > 0 && <button className="mcp-more-projects-btn" onClick={() => setExpandedProjectLists(prev => { const n = new Set(prev); n.add(cfg.id); return n; })}>{t('mcp.moreProjects', hiddenCount)}</button>}
                {showAll && sorted.length > PROJECT_TOGGLE_LIMIT && <button className="mcp-less-projects-btn" onClick={() => setExpandedProjectLists(prev => { const n = new Set(prev); n.delete(cfg.id); return n; })}>{t('mcp.lessProjects')}</button>}
              </>);
            })()}
          </div>
        </div>
        {/* ── Sync CLIs locaux (Phase-3 refactor — checkbox dans Scope) ──
            Hidden entirely for API-only plugins: those don't have
            an MCP transport to write to `.mcp.json` / Codex / Gemini
            / Copilot, they only exist as a `## REST APIs available`
            block in the agent's system prompt. Showing a "Sync CLI"
            toggle on them was misleading — the user reported the
            confusion. Hybrid plugins keep the toggle but get a
            note that it only affects the MCP side. */}
        {supportsHostSync && (
        <div
          className="mcp-host-sync-block"
          style={{ marginTop: 12, paddingTop: 12, borderTop: '1px dashed var(--kr-border, #e5e7eb)', position: 'relative' }}
        >
          <label
            style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer', fontSize: '0.95em', fontWeight: 500 }}
          >
            <input
              type="checkbox"
              checked={cfg.host_sync !== 'None'}
              onChange={(e) => handleSetHostSync(cfg.id, e.target.checked ? 'GlobalOnly' : 'None')}
            />
            <Globe size={13} />
            {t('mcp.hostSync.localCliLabel')}
          </label>
          {cfgKind === 'hybrid' && (
            <p className="text-muted" style={{ fontSize: '0.8em', margin: '4px 0 0 22px', fontStyle: 'italic' }}>
              {t('mcp.hostSync.hybridNote')}
            </p>
          )}
          {cfg.host_sync !== 'None' && (
            <HostSyncPreview
              isGlobal={cfg.is_global}
              projectIds={cfg.project_ids}
              projects={projects}
            />
          )}
        </div>
        )}
        {/* For API-only plugins: tell the user explicitly that
            the toggle they would expect here doesn't apply. */}
        {!supportsHostSync && (
          <div
            style={{ marginTop: 12, paddingTop: 12, borderTop: '1px dashed var(--kr-border, #e5e7eb)' }}
          >
            <p className="text-muted" style={{ fontSize: '0.85em', margin: 0 }}>
              <Globe size={11} style={{ verticalAlign: 'text-bottom', marginRight: 4 }} />
              {t('mcp.hostSync.apiOnlyNote')}
            </p>
          </div>
        )}
        </>)}
      </div>
    </aside>
  );
}

import { mcps as mcpsApi } from '../../lib/api';
import type { ApiAuthKind, ApiEndpoint, CustomApiPayload } from '../../types/generated';
import { CustomApiAiHelper } from '../CustomApiAiHelper';
import { Dropdown } from '../Dropdown';
import { SecretField } from '../SecretField';
import { Trash2, Plus, X, Check } from 'lucide-react';
import { PluginScopeEditor } from './PluginScopeEditor';
import { effectiveTestEndpoint, isTestableEndpoint } from './testEndpoint';
import type { McpPageState } from './useMcpPageState';

/**
 * Custom API / JSON plugin editor. Rendered in TWO places, driven by the
 * same shared `state`: the Add-MCP panel (create mode, `addMcpSelected
 * === 'api-custom'`) and the plugin detail panel (edit mode, `editingCustomServerId`
 * set). Refonte 2b (2026-06-10) hoisted this so both places stay in sync
 * without prop-drilling; the KT-830 split turns it into a standalone
 * component instead of a JSX const closing over the page's state.
 */
export function CustomApiForm({ state }: { state: McpPageState }) {
  const {
    t,
    customName, setCustomName,
    customBaseUrl, setCustomBaseUrl,
    customDescription, setCustomDescription,
    customDocsUrl, setCustomDocsUrl,
    customFields, setCustomFields,
    customEndpoints, setCustomEndpoints,
    customHeaders, setCustomHeaders,
    customTestEndpoint, setCustomTestEndpoint,
    editingCustomServerId, editingCustomConfigId,
    replacingFields, setReplacingFields,
    customAuth, setCustomAuth,
    authKindOf, setAuthKindBy, slugEnvKey,
    addMcpGlobal, setAddMcpGlobal, addMcpProjectIds, setAddMcpProjectIds,
    addMcpIncludeGeneral, setAddMcpIncludeGeneral,
    handleAddMcpFromRegistry, resetAddMcp, setAddMcpSelected,
    installedAgentTypes, configLanguage, projects,
  } = state;
  // KT-831 — "Général" (discussions sans projet) can only be toggled once
  // the config exists: creation always starts it at the backend default
  // (`true`), there is nothing to PATCH before the row is even inserted.
  const isEditing = !!editingCustomServerId;
  const testEndpoint = effectiveTestEndpoint(customEndpoints, customTestEndpoint);

  return (
    <>
      {/* Custom API freeform editor. Mirrors `CustomApiPayload`:
          name + base URL are required; description + docs link +
          fields are optional. The backend slugifies each field
          label into an env key. */}
      <div className="mb-5">
        <label className="mcp-field-label">{t('mcp.custom.name')} *</label>
        <input
          className="input"
          value={customName}
          onChange={(e) => setCustomName(e.target.value)}
          placeholder={t('mcp.custom.namePlaceholder')}
          autoFocus
        />
      </div>
      <div className="mb-5">
        <label className="mcp-field-label">{t('mcp.custom.baseUrl')} *</label>
        <input
          className="input mcp-input-mono"
          value={customBaseUrl}
          onChange={(e) => setCustomBaseUrl(e.target.value)}
          placeholder={t('mcp.custom.baseUrlPlaceholder')}
        />
      </div>
      <div className="mb-5">
        <label className="mcp-field-label">{t('mcp.custom.description')}</label>
        <textarea
          className="input"
          rows={3}
          value={customDescription}
          onChange={(e) => setCustomDescription(e.target.value)}
          placeholder={t('mcp.custom.descriptionPlaceholder')}
        />
      </div>
      <div className="mb-5">
        <label className="mcp-field-label">{t('mcp.custom.docsUrl')}</label>
        <input
          className="input mcp-input-mono"
          value={customDocsUrl}
          onChange={(e) => setCustomDocsUrl(e.target.value)}
          placeholder="https://docs.example.com/api"
        />
      </div>
      <div className="mb-5">
        <label className="mcp-field-label">{t('mcp.custom.fields')}</label>
        <p className="mcp-env-key-desc mb-3">{t('mcp.custom.fieldsHint')}</p>
        {/* 0.8.6 — Edit mode: reassure the user their secrets
            are safe. The encrypted env lives in the per-config
            row (mcp_configs), NOT in the spec we're editing.
            Without this banner, users see empty value fields
            and assume their credentials were wiped (caught
            2026-05-19 in live test "je n'ai plus les valeurs").
            The masked `•••• stocké` per-row makes the same
            point inline. */}
        {editingCustomServerId && (
          <p className="mcp-env-key-desc mb-3" style={{ borderLeft: '3px solid var(--kr-accent)', paddingLeft: '0.6rem' }}>
            🔒 {t('mcp.custom.fieldsHintEditMode')}
          </p>
        )}
        {customFields.map((f, idx) => {
          return (
          <div key={idx} className="mcp-custom-field-row mb-2">
            <input
              className="input mcp-custom-field-label"
              value={f.label}
              onChange={(e) => setCustomFields(prev => prev.map((row, i) => i === idx ? { ...row, label: e.target.value } : row))}
              placeholder={t('mcp.custom.fieldLabel')}
            />
            {/* Secrets via the shared <SecretField> (single source of
                truth): stored → masked + 👁 peek + "Remplacer";
                replacing → empty input + 👁 + "Annuler"; create →
                input + 👁. "Modifier le plugin" reste la surface unique
                d'édition (la carte est en lecture seule). */}
            <SecretField
              value={f.value}
              onChange={(v) => setCustomFields(prev => prev.map((row, i) => i === idx ? { ...row, value: v } : row))}
              stored={!!editingCustomServerId && !!f.stored}
              replacing={replacingFields.has(idx)}
              onReplace={() => setReplacingFields(prev => { const n = new Set(prev); n.add(idx); return n; })}
              onCancelReplace={() => {
                setReplacingFields(prev => { const n = new Set(prev); n.delete(idx); return n; });
                setCustomFields(prev => prev.map((row, i) => i === idx ? { ...row, value: '' } : row));
              }}
              onRevealStored={async () => {
                if (!editingCustomConfigId) return null;
                const entries = await mcpsApi.revealSecrets(editingCustomConfigId);
                return entries.find(e => e.key === slugEnvKey(f.label))?.masked_value ?? null;
              }}
            />
            <button
              type="button"
              className="mcp-icon-btn"
              onClick={() => setCustomFields(prev => prev.filter((_, i) => i !== idx))}
              disabled={customFields.length === 1}
              aria-label={t('mcp.custom.fieldRemove')}
              title={t('mcp.custom.fieldRemove')}
            >
              <Trash2 size={12} />
            </button>
          </div>
          );
        })}
        <button
          type="button"
          className="mcp-btn-action"
          onClick={() => setCustomFields(prev => [...prev, { label: '', value: '' }])}
        >
          <Plus size={12} /> {t('mcp.custom.fieldAdd')}
        </button>
      </div>
      {/* 0.8.6 — endpoints declared at creation time. The AI helper
          (button further below) can populate this list after fetching
          `docs_url` via WebFetch. Empty list = `mcp_list` will emit
          `NEEDS_RESEARCH` and agents will go through the doc each time.
          Cf. [[project_endpoints_autodiscovery_0_8_6]]. */}
      <div className="mb-5">
        <label className="mcp-field-label">{t('mcp.custom.endpoints.header')}</label>
        <p className="mcp-env-key-desc mb-3">
          {customEndpoints.length === 0
            ? t('mcp.custom.endpoints.emptyHint')
            : t('mcp.custom.endpoints.populatedHint', customEndpoints.length)}
        </p>
        {customEndpoints.length > 0 && (
          <p className="mcp-env-key-desc mb-3" data-testid="mcp-custom-test-endpoint-hint">
            {testEndpoint
              ? t('mcp.custom.endpoints.testHint', testEndpoint)
              : t('mcp.custom.endpoints.noTestable')}
          </p>
        )}
        {customEndpoints.map((e, idx) => (
          <div key={idx} className="mcp-custom-field-row mb-2">
            <input
              type="radio"
              name="mcp-custom-test-endpoint"
              checked={testEndpoint !== null && testEndpoint === e.path.trim()}
              disabled={!e.path.trim() || !isTestableEndpoint(e)}
              onChange={() => setCustomTestEndpoint(e.path.trim())}
              aria-label={t('mcp.custom.endpoints.testLabel', e.path || '…')}
              title={isTestableEndpoint(e) ? t('mcp.custom.endpoints.testLabel', e.path || '…') : t('mcp.custom.endpoints.notTestable')}
              data-testid="mcp-custom-test-endpoint"
            />
            <select
              className="input mcp-custom-field-label"
              value={e.method || 'GET'}
              onChange={(ev) => setCustomEndpoints(prev => prev.map((row, i) => i === idx ? { ...row, method: ev.target.value } : row))}
              aria-label={t('mcp.custom.endpoints.methodLabel')}
            >
              <option value="GET">GET</option>
              <option value="POST">POST</option>
              <option value="PUT">PUT</option>
              <option value="PATCH">PATCH</option>
              <option value="DELETE">DELETE</option>
            </select>
            <input
              className="input mcp-input-mono"
              value={e.path}
              onChange={(ev) => setCustomEndpoints(prev => prev.map((row, i) => i === idx ? { ...row, path: ev.target.value } : row))}
              placeholder="/v1/widgets"
            />
            <input
              className="input"
              value={e.description}
              onChange={(ev) => setCustomEndpoints(prev => prev.map((row, i) => i === idx ? { ...row, description: ev.target.value } : row))}
              placeholder={t('mcp.custom.endpoints.descPlaceholder')}
            />
            <button
              type="button"
              className="mcp-icon-btn"
              onClick={() => setCustomEndpoints(prev => prev.filter((_, i) => i !== idx))}
              aria-label={t('mcp.custom.endpoints.remove')}
              title={t('mcp.custom.endpoints.remove')}
            >
              <X size={12} />
            </button>
          </div>
        ))}
        <button
          type="button"
          className="mcp-btn-action"
          onClick={() => setCustomEndpoints(prev => [...prev, { path: '', method: 'GET', description: '' }])}
        >
          <Plus size={12} /> {t('mcp.custom.endpoints.add')}
        </button>
      </div>
      {/* 0.8.6 — Auth section. MVP exposes 3 of the 7 runtime
          variants (None, Bearer, TokenExchange) — others (Header,
          Query, Basic, OAuth2) come in 0.8.6 Layer A. The
          TokenExchange option specifically unblocks Didomi-shape
          APIs (POST /sessions with JSON body → access_token →
          Bearer). Cf. [[project_token_exchange_generic_0_9_0]]. */}
      <div className="mb-5">
        <label className="mcp-field-label">{t('mcp.custom.auth.header')}</label>
        <p className="mcp-env-key-desc mb-3">{t('mcp.custom.auth.hint')}</p>
        <select
          className="input mcp-custom-field-label"
          value={authKindOf(customAuth)}
          onChange={(e) => setAuthKindBy(e.target.value as 'None' | 'Bearer' | 'TokenExchange')}
          style={{ marginBottom: '0.75rem', width: '100%' }}
        >
          <option value="None">{t('mcp.custom.auth.kind.none')}</option>
          <option value="Bearer">{t('mcp.custom.auth.kind.bearer')}</option>
          <option value="TokenExchange">{t('mcp.custom.auth.kind.tokenExchange')}</option>
          {authKindOf(customAuth) === 'Other' && (
            <option value="Other" disabled>{t('mcp.custom.auth.kind.other')}</option>
          )}
        </select>
        {/* Bearer — 1 env_key dropdown peuplé depuis customFields */}
        {authKindOf(customAuth) === 'Bearer' && typeof customAuth === 'object' && 'Bearer' in customAuth && (
          <div className="mcp-custom-field-row mb-2">
            <label className="mcp-field-label mcp-field-label-inline" style={{ minWidth: '120px' }}>
              {t('mcp.custom.auth.bearer.envKey')}
            </label>
            <select
              className="input mcp-input-mono"
              value={customAuth.Bearer.env_key}
              onChange={(e) => setCustomAuth({ Bearer: { env_key: e.target.value } })}
            >
              <option value="">{t('mcp.custom.auth.bearer.envKeyPlaceholder')}</option>
              {customFields.filter(f => f.label.trim()).map(f => (
                <option key={f.label} value={slugEnvKey(f.label)}>
                  {slugEnvKey(f.label)} ({f.label})
                </option>
              ))}
            </select>
          </div>
        )}
        {/* TokenExchange — Didomi pattern, JSON body POST → access_token */}
        {authKindOf(customAuth) === 'TokenExchange' && typeof customAuth === 'object' && 'TokenExchange' in customAuth && (
          <div style={{ borderLeft: '3px solid var(--kr-accent)', paddingLeft: '0.8rem' }}>
            <p className="mcp-env-key-desc mb-3" style={{ fontSize: '0.85em' }}>{t('mcp.custom.auth.tokenExchange.hint')}</p>
            <div className="mb-3">
              <label className="mcp-field-label">{t('mcp.custom.auth.tokenExchange.endpoint')}</label>
              <input
                className="input mcp-input-mono"
                value={customAuth.TokenExchange.endpoint}
                onChange={(e) => setCustomAuth({
                  TokenExchange: { ...customAuth.TokenExchange, endpoint: e.target.value },
                })}
                placeholder="/sessions"
              />
            </div>
            <div className="mb-3">
              <label className="mcp-field-label">{t('mcp.custom.auth.tokenExchange.method')}</label>
              <select
                className="input mcp-input-mono"
                value={customAuth.TokenExchange.method}
                onChange={(e) => setCustomAuth({
                  TokenExchange: { ...customAuth.TokenExchange, method: e.target.value },
                })}
              >
                <option value="POST">POST</option>
                <option value="PUT">PUT</option>
              </select>
            </div>
            <div className="mb-3">
              <label className="mcp-field-label">{t('mcp.custom.auth.tokenExchange.bodyFormat')}</label>
              <select
                className="input mcp-input-mono"
                value={customAuth.TokenExchange.body_format}
                onChange={(e) => setCustomAuth({
                  TokenExchange: { ...customAuth.TokenExchange, body_format: e.target.value as 'Json' | 'FormUrlEncoded' },
                })}
              >
                <option value="Json">{t('mcp.custom.auth.bodyFormat.json')}</option>
                <option value="FormUrlEncoded">{t('mcp.custom.auth.bodyFormat.formUrlEncoded')}</option>
              </select>
            </div>
            <div className="mb-3">
              <label className="mcp-field-label">{t('mcp.custom.auth.tokenExchange.bodyTemplate')}</label>
              <p className="mcp-env-key-desc mb-2" style={{ fontSize: '0.8em' }}>{t('mcp.custom.auth.tokenExchange.bodyTemplateHint')}</p>
              <textarea
                className="input mcp-input-mono"
                rows={5}
                value={(() => {
                  try { return JSON.stringify(customAuth.TokenExchange.body_template, null, 2); }
                  catch { return '{}'; }
                })()}
                onChange={(e) => {
                  try {
                    const parsed = JSON.parse(e.target.value);
                    setCustomAuth({ TokenExchange: { ...customAuth.TokenExchange, body_template: parsed } });
                  } catch {
                    // Keep the textarea contents user-typed even when invalid;
                    // we store the raw text via a sibling state? Simpler: just
                    // don't update the parsed value on invalid JSON. The user
                    // sees their typo and corrects.
                  }
                }}
                placeholder='{"type": "api-key", "key": "${ENV.API_KEY}", "secret": "${ENV.API_SECRET}"}'
              />
            </div>
            <div className="mb-3">
              <label className="mcp-field-label">{t('mcp.custom.auth.tokenExchange.tokenJsonpath')}</label>
              <input
                className="input mcp-input-mono"
                value={customAuth.TokenExchange.token_jsonpath}
                onChange={(e) => setCustomAuth({
                  TokenExchange: { ...customAuth.TokenExchange, token_jsonpath: e.target.value },
                })}
                placeholder="$.access_token"
              />
            </div>
            <div className="mb-3">
              <label className="mcp-field-label">{t('mcp.custom.auth.tokenExchange.ttl')}</label>
              <input
                className="input mcp-input-mono"
                type="number"
                min={0}
                value={customAuth.TokenExchange.ttl_seconds}
                onChange={(e) => setCustomAuth({
                  TokenExchange: { ...customAuth.TokenExchange, ttl_seconds: parseInt(e.target.value, 10) || 0 },
                })}
              />
            </div>
            <div className="mb-3">
              <label className="mcp-field-label">{t('mcp.custom.auth.tokenExchange.inject')}</label>
              <Dropdown<'BearerHeader' | 'CustomHeader' | 'QueryParam'>
                value={typeof customAuth.TokenExchange.inject === 'string' ? customAuth.TokenExchange.inject : (Object.keys(customAuth.TokenExchange.inject)[0] as 'CustomHeader' | 'QueryParam')}
                options={[
                  { value: 'BearerHeader', label: t('mcp.custom.auth.inject.bearerHeader') },
                  { value: 'CustomHeader', label: t('mcp.custom.auth.inject.customHeader') },
                  { value: 'QueryParam', label: t('mcp.custom.auth.inject.queryParam') },
                ]}
                onChange={(kind) => {
                  let inject: ApiAuthKind extends infer T ? T extends { TokenExchange: { inject: infer I } } ? I : never : never;
                  if (kind === 'BearerHeader') inject = 'BearerHeader' as typeof inject;
                  else if (kind === 'CustomHeader') inject = { CustomHeader: { name: 'X-Auth-Token' } } as typeof inject;
                  else inject = { QueryParam: { name: 'token' } } as typeof inject;
                  setCustomAuth({ TokenExchange: { ...customAuth.TokenExchange, inject } });
                }}
                ariaLabel={t('mcp.custom.auth.tokenExchange.inject')}
                testId="mcp-token-exchange-inject"
              />
            </div>
          </div>
        )}
        {/* Other variants (ApiKeyQuery / Header / Basic / OAuth2) :
            not yet exposed in UI. If an existing plugin uses one of
            them (e.g. registry-shipped), the picker shows "Other —
            edit in JSON" placeholder so the user knows it's
            intentional, not a bug. */}
        {authKindOf(customAuth) === 'Other' && (
          <p className="mcp-env-key-desc" style={{ color: 'var(--kr-warning)' }}>
            {t('mcp.custom.auth.kind.otherWarning')}
          </p>
        )}
      </div>
      <div className="mb-5">
        <label className="mcp-field-label">{t('mcp.custom.headers.header')}</label>
        <p className="mcp-env-key-desc mb-3">{t('mcp.custom.headers.hint')}</p>
        {customHeaders.map((h, idx) => (
          <div key={idx} className="mcp-custom-field-row mb-2" data-testid="mcp-custom-header-row">
            <input
              className="input mcp-input-mono mcp-custom-field-label"
              value={h.name}
              onChange={(ev) => setCustomHeaders(prev => prev.map((row, i) => i === idx ? { ...row, name: ev.target.value } : row))}
              placeholder={t('mcp.custom.headers.namePlaceholder')}
              aria-label={t('mcp.custom.headers.namePlaceholder')}
            />
            <input
              className="input mcp-input-mono"
              value={h.value}
              onChange={(ev) => setCustomHeaders(prev => prev.map((row, i) => i === idx ? { ...row, value: ev.target.value } : row))}
              placeholder={t('mcp.custom.headers.valuePlaceholder')}
              aria-label={t('mcp.custom.headers.valuePlaceholder')}
            />
            <button
              type="button"
              className="mcp-icon-btn"
              onClick={() => setCustomHeaders(prev => prev.filter((_, i) => i !== idx))}
              aria-label={t('mcp.custom.headers.remove')}
              title={t('mcp.custom.headers.remove')}
            >
              <X size={12} />
            </button>
          </div>
        ))}
        <button
          type="button"
          className="mcp-btn-action"
          onClick={() => setCustomHeaders(prev => [...prev, { name: '', value: '' }])}
        >
          <Plus size={12} /> {t('mcp.custom.headers.add')}
        </button>
      </div>
      {/* Single reusable scope editor (KT-831) — same component as the
          registry add flow, the fiche and bundle import. Custom API
          plugins are always API-only, so no CLI-sync section (the
          component shows the "no CLI sync" note instead). "Général" is
          only wired once the config exists (edit mode) — see `isEditing`. */}
      <div className="mb-6">
        <PluginScopeEditor
          t={t}
          projects={projects}
          isGlobal={addMcpGlobal}
          onToggleGlobal={() => setAddMcpGlobal(!addMcpGlobal)}
          includeGeneral={isEditing ? addMcpIncludeGeneral : undefined}
          onToggleGeneral={isEditing ? () => setAddMcpIncludeGeneral(!addMcpIncludeGeneral) : undefined}
          projectIds={addMcpProjectIds}
          onToggleProject={(projectId, isLinked) => setAddMcpProjectIds(
            isLinked ? addMcpProjectIds.filter(id => id !== projectId) : [...addMcpProjectIds, projectId],
          )}
          supportsHostSync={false}
          hostSync="None"
          testIdPrefix="mcp-custom-scope"
        />
      </div>
      <div className="flex-row gap-4">
        <button
          className="mcp-btn-action mcp-btn-action-primary"
          onClick={handleAddMcpFromRegistry}
          disabled={!customName.trim() || !customBaseUrl.trim()}
        >
          <Check size={14} /> {editingCustomServerId ? t('mcp.custom.saveEdit') : t('mcp.custom.save')}
        </button>
        <button className="mcp-btn-action" onClick={() => { setAddMcpSelected(null); resetAddMcp(); }}>
          {t('mcp.back')}
        </button>
        {/* AI helper bubble: pre-fills the form from a curl, a docs link
            or a freeform description. Same UX as the workflow ApiCall
            helper (header agent dropdown, top context chip, welcome
            starters). */}
        {installedAgentTypes && installedAgentTypes.length > 0 && (
          <CustomApiAiHelper
            formSnapshot={{
              name: customName,
              base_url: customBaseUrl,
              description: customDescription,
              docs_url: customDocsUrl,
              fields: customFields,
              endpoints: customEndpoints,
              default_headers: customHeaders,
              test_endpoint: testEndpoint,
            }}
            onApply={(updates: Partial<CustomApiPayload>) => {
              if (typeof updates.name === 'string') setCustomName(updates.name);
              if (typeof updates.base_url === 'string') setCustomBaseUrl(updates.base_url);
              if (typeof updates.description === 'string') setCustomDescription(updates.description);
              if (typeof updates.docs_url === 'string') setCustomDocsUrl(updates.docs_url);
              if (Array.isArray(updates.fields) && updates.fields.length > 0) {
                // Merge: keep user-typed values for fields that already
                // have content, accept agent-proposed labels/empties for
                // the rest. Avoids the agent wiping a token the user
                // already pasted while still letting it add new fields.
                const existing = customFields.filter(f => f.label.trim() || f.value.trim());
                const proposedLabels = new Set(updates.fields.map(f => f.label));
                const merged = [
                  ...existing.filter(f => !proposedLabels.has(f.label) || f.value.trim()),
                  ...updates.fields.filter(f =>
                    !existing.some(e => e.label === f.label && e.value.trim()),
                  ),
                ];
                setCustomFields(merged.length > 0 ? merged : [{ label: '', value: '' }]);
              }
              // 0.8.6 — endpoint merge. The agent typically proposes
              // 5-15 endpoints after a WebFetch. We merge by
              // (path + method) so the user's hand-typed entries are
              // preserved (no surprise wipe), and the agent's proposals
              // fill the gaps. The "Add row" trailing-empty sentinel is
              // filtered out of the seed.
              if (Array.isArray(updates.endpoints) && updates.endpoints.length > 0) {
                const existing = customEndpoints.filter(e => e.path.trim() !== '');
                const key = (e: ApiEndpoint) => `${e.method.toUpperCase()} ${e.path.trim()}`;
                const seen = new Set(existing.map(key));
                const merged: ApiEndpoint[] = [
                  ...existing,
                  ...updates.endpoints.filter(e => !seen.has(key(e))),
                ];
                setCustomEndpoints(merged);
              }
              if (typeof updates.test_endpoint === 'string') setCustomTestEndpoint(updates.test_endpoint);
              // Same no-wipe merge, keyed by case-insensitive header name.
              if (Array.isArray(updates.default_headers) && updates.default_headers.length > 0) {
                const existing = customHeaders.filter(h => h.name.trim() !== '');
                const seen = new Set(existing.map(h => h.name.trim().toLowerCase()));
                setCustomHeaders([
                  ...existing,
                  ...updates.default_headers.filter(h => !seen.has(h.name.trim().toLowerCase())),
                ]);
              }
            }}
            installedAgents={installedAgentTypes}
            configLanguage={configLanguage}
            t={t}
          />
        )}
      </div>
    </>
  );
}

import {
  Puzzle, Plus, Check, X, Key, ExternalLink, Plug, Globe, Upload, Download,
  Terminal, Eye, CheckSquare, Square,
} from 'lucide-react';
import { pluginKind } from '../../lib/pluginKind';
import { pluginCredentialKeys, pluginCredentialKind } from '../../lib/pluginCredentials';
import { CustomApiForm } from './CustomApiForm';
import type { McpPageState } from './useMcpPageState';

/** Placeholder hints for common MCP env vars — helps non-dev users understand what to enter */
const ENV_PLACEHOLDERS: Record<string, string> = {
  // Atlassian / Jira / Confluence
  JIRA_URL: 'https://your-company.atlassian.net',
  JIRA_USERNAME: 'prenom.nom@company.com',
  JIRA_API_TOKEN: 'ATATT3x... (from id.atlassian.com)',
  CONFLUENCE_URL: 'https://your-company.atlassian.net/wiki',
  CONFLUENCE_USERNAME: 'prenom.nom@company.com',
  CONFLUENCE_API_TOKEN: 'ATATT3x... (same as Jira token)',
  // GitHub
  GITHUB_PERSONAL_ACCESS_TOKEN: 'ghp_xxxxxxxxxxxx',
  GITHUB_TOKEN: 'ghp_xxxxxxxxxxxx',
  // GitLab
  GITLAB_PERSONAL_ACCESS_TOKEN: 'glpat-xxxxxxxxxxxx',
  GITLAB_TOKEN: 'glpat-xxxxxxxxxxxx',
  GITLAB_HOST: 'https://gitlab.com',
  GITLAB_URL: 'https://gitlab.com',
  // Fastly (optional CLI fallback)
  FASTLY_API_TOKEN: '01H... (optional fallback)',
  // Slack
  SLACK_BOT_TOKEN: 'xoxb-xxxxxxxxxxxx',
  SLACK_TEAM_ID: 'T0XXXXXXX',
  // MongoDB
  MDB_MCP_CONNECTION_STRING: 'mongodb+srv://user:pass@cluster.mongodb.net/db',
  MDB_MCP_ATLAS_CLIENT_ID: 'xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx',
  MDB_MCP_ATLAS_CLIENT_SECRET: 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx',
  // Qdrant
  QDRANT_URL: 'http://localhost:6333',
  COLLECTION_NAME: 'my-collection',
  EMBEDDING_MODEL: 'sentence-transformers/all-MiniLM-L6-v2',
  // Perplexity
  PERPLEXITY_API_KEY: 'pplx-xxxxxxxxxxxx',
  // Linear
  LINEAR_API_KEY: 'lin_api_xxxxxxxxxxxx',
  // Notion
  NOTION_API_KEY: 'ntn_xxxxxxxxxxxx',
  // OpenAI
  OPENAI_API_KEY: 'sk-xxxxxxxxxxxx',
  // Anthropic
  ANTHROPIC_API_KEY: 'sk-ant-xxxxxxxxxxxx',
  // Google
  GOOGLE_API_KEY: 'AIzaXXXXXXXXXX',
  // Sentry
  SENTRY_AUTH_TOKEN: 'sntrys_xxxxxxxxxxxx',
  SENTRY_ORG: 'your-organization-slug',
  SENTRY_PROJECT: 'your-project-slug',
  // Brave
  BRAVE_API_KEY: 'BSA_xxxxxxxxxxxx',
  // Exa
  EXA_API_KEY: 'exa-xxxxxxxxxxxx',
  // Redis
  REDIS_URL: 'redis://localhost:6379',
  // PostgreSQL
  DATABASE_URL: 'postgresql://user:pass@localhost:5432/db',
  POSTGRES_CONNECTION_STRING: 'postgresql://user:pass@localhost:5432/db',
  // Chartbeat (API plugin)
  CHARTBEAT_API_KEY: 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx (32-char key from chartbeat.com account)',
  CHARTBEAT_HOST: 'domain.tld (the site tracked in Chartbeat)',
  // Adobe Analytics (OAuth2 S2S) — placeholders intentionally don't match Adobe's
  // real secret prefixes (p8e-, s8e-) so GitHub's secret-scanner push protection
  // doesn't flag this file when the repo is pushed.
  ADOBE_CLIENT_ID: 'your-adobe-client-id (from Adobe Developer Console project)',
  ADOBE_CLIENT_SECRET: 'your-adobe-client-secret (generated in the same project)',
  // Google Programmable Search — ditto, avoid the AIza prefix that Google real keys use.
  GOOGLE_SEARCH_API_KEY: 'your-google-cloud-api-key (from console.cloud.google.com → APIs & Credentials)',
  // Generic patterns
  API_KEY: 'your-api-key',
  API_TOKEN: 'your-api-token',
  API_SECRET: 'your-api-secret',
  BASE_URL: 'https://api.example.com',
};

/** Add-MCP modal — registry grid (kind filter + category pills), the
 *  import-from-JSON form, the Custom API create form, and the per-plugin
 *  env-var form for a registry pick. Extracted verbatim from the
 *  pre-KT-830 `McpPage` render's `showAddMcp && (...)` block. */
export function AddPluginModal({ state }: { state: McpPageState }) {
  const {
    t,
    showAddMcp, resetAddMcp,
    addMcpRef, handleAddMcpModalKeyDown,
    addMcpSelected, setAddMcpSelected, addMcpLabel, setAddMcpLabel,
    addMcpKindFilter, setAddMcpKindFilter,
    addMcpSearch, setAddMcpSearch,
    selectedCategory, setSelectedCategory,
    availableRegistry, customApiVisible, configuredServerIds, configs,
    addMcpEnv, setAddMcpEnv, addMcpGlobal, setAddMcpGlobal, addMcpHostSync, setAddMcpHostSync,
    addVisibleFields, setAddVisibleFields,
    selectedDef, mcpOverview,
    importJsonText, setImportJsonText, importJsonError, setImportJsonError, importJsonLoading,
    handlePasteImportJson, handleImportFromFile, handleImportCustomPlugin,
    handleAddMcpFromRegistry,
  } = state;

  if (!showAddMcp) return null;

  return (
    <div
      className="mcp-add-modal-backdrop"
      data-testid="mcp-add-modal-backdrop"
      role="presentation"
      onMouseDown={event => {
        if (event.target === event.currentTarget) resetAddMcp();
      }}
    >
      <div
        ref={addMcpRef}
        className="mcp-card mcp-add-panel mcp-add-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="mcp-add-modal-title"
        onKeyDown={handleAddMcpModalKeyDown}
      >
      <div className="mcp-add-header">
        <h3 id="mcp-add-modal-title" className="mcp-add-title">
          {addMcpSelected ? t('mcp.configure', selectedDef?.name ?? addMcpLabel) : t('mcp.addTitle')}
        </h3>
        <button type="button" className="mcp-icon-btn" onClick={resetAddMcp} aria-label={t('common.close')}>
          <X size={14} />
        </button>
      </div>

      {!addMcpSelected ? (
        <>
          {/* 0.8.6 phase 4 — top-level type filter (MCP / API / CLI).
              Distinct from the per-tag category filter below : this
              one narrows by TRANSPORT KIND so the user can isolate
              "show me only CLI wrappers" (gitlab, fastly) without
              scrolling through MCP servers. `all` is the default. */}
          <div
            className="mcp-kind-filter-row"
            role="radiogroup"
            aria-label={t('mcp.kindFilter.label')}
            data-testid="mcp-kind-filter"
            style={{
              display: 'flex',
              gap: 6,
              marginBottom: 10,
              flexWrap: 'wrap',
            }}
          >
            {(['all', 'mcp', 'api', 'cli'] as const).map(kind => {
              const labelKey = `mcp.kindFilter.${kind}`;
              const icons: Record<typeof kind, string> = { all: '✱', mcp: '🔌', api: '🌐', cli: '⌨' };
              const active = addMcpKindFilter === kind;
              return (
                <button
                  key={kind}
                  type="button"
                  role="radio"
                  aria-checked={active}
                  className="mcp-kind-filter-btn"
                  data-active={active}
                  data-testid={`mcp-kind-filter-${kind}`}
                  onClick={() => setAddMcpKindFilter(kind)}
                  style={{
                    padding: '4px 10px',
                    borderRadius: 6,
                    border: active
                      ? '1px solid var(--kr-accent, #c8a0ff)'
                      : '1px solid var(--kr-border-subtle, rgba(255,255,255,0.1))',
                    background: active
                      ? 'var(--kr-bg-accent-subtle, rgba(200,160,255,0.15))'
                      : 'transparent',
                    color: active ? 'var(--kr-text-primary)' : 'var(--kr-text-secondary)',
                    cursor: 'pointer',
                    fontSize: 12,
                  }}
                >
                  <span style={{ marginRight: 4 }}>{icons[kind]}</span>
                  {t(labelKey)}
                </button>
              );
            })}
          </div>
          <input
            className="input mb-5"
            placeholder={t('mcp.searchRegistry')}
            value={addMcpSearch}
            onChange={(e) => setAddMcpSearch(e.target.value)}
            autoFocus
          />
          {/* Category filter pills */}
          {(() => {
            const categoryMap: Record<string, string> = {
              // 0.8.6 phase 4 — `cli` first so plugins that WRAP a
              // local CLI binary (Fastly, GitLab via glab, …) land in
              // their own bucket rather than the generic Git/Code or
              // Cloud groups. Same prereq surface as a CLI agent —
              // the user must install the binary on the host first.
              cli: t('mcp.cat.cli'),
              git: t('mcp.cat.gitCode'), code: t('mcp.cat.gitCode'),
              database: t('mcp.cat.databases'), sql: t('mcp.cat.databases'), cache: t('mcp.cat.databases'), embedded: t('mcp.cat.databases'),
              cloud: t('mcp.cat.cloud'), containers: t('mcp.cat.cloud'), devops: t('mcp.cat.cloud'),
              search: t('mcp.cat.search'), web: t('mcp.cat.search'), http: t('mcp.cat.search'), browser: t('mcp.cat.search'), scraping: t('mcp.cat.search'),
              monitoring: t('mcp.cat.monitoring'), analytics: t('mcp.cat.monitoring'), errors: t('mcp.cat.monitoring'),
              communication: t('mcp.cat.communication'), chat: t('mcp.cat.communication'), email: t('mcp.cat.communication'), mailing: t('mcp.cat.communication'),
              'project-management': t('mcp.cat.projectMgmt'), issues: t('mcp.cat.projectMgmt'),
              core: t('mcp.cat.utilities'), filesystem: t('mcp.cat.utilities'), docs: t('mcp.cat.utilities'), libraries: t('mcp.cat.utilities'),
              design: t('mcp.cat.design'),
            };
            const getCategory = (tags: string[]) => {
              for (const tag of tags) { if (categoryMap[tag]) return categoryMap[tag]; }
              return t('mcp.cat.other');
            };
            const categoryOrder = [t('mcp.cat.cli'), t('mcp.cat.gitCode'), t('mcp.cat.databases'), t('mcp.cat.cloud'), t('mcp.cat.search'), t('mcp.cat.monitoring'), t('mcp.cat.communication'), t('mcp.cat.projectMgmt'), t('mcp.cat.design'), t('mcp.cat.utilities'), t('mcp.cat.other')];
            const grouped = new Map<string, typeof availableRegistry>();
            for (const m of availableRegistry) {
              const cat = getCategory(m.tags);
              let bucket = grouped.get(cat);
              if (!bucket) {
                bucket = [];
                grouped.set(cat, bucket);
              }
              bucket.push(m);
            }
            const catsWithItems = categoryOrder.filter(cat => grouped.has(cat));
            return (
              <>
                <div className="mcp-cat-pills">
                  <button
                    className={`mcp-cat-pill${!selectedCategory ? ' mcp-cat-pill-active' : ''}`}
                    onClick={() => setSelectedCategory(null)}
                  >
                    {t('mcp.cat.all')}
                  </button>
                  {catsWithItems.map(cat => (
                    <button
                      key={cat}
                      className={`mcp-cat-pill${selectedCategory === cat ? ' mcp-cat-pill-active' : ''}`}
                      onClick={() => setSelectedCategory(selectedCategory === cat ? null : cat)}
                    >
                      {cat} <span className="mcp-cat-pill-count">{grouped.get(cat)?.length ?? 0}</span>
                    </button>
                  ))}
                </div>
                <div className="mcp-registry-grid">
                  {customApiVisible && (addMcpKindFilter === 'all' || addMcpKindFilter === 'api') && !selectedCategory ? (
                    <div
                      key="api-import"
                      className="mcp-registry-card mcp-registry-card-custom"
                      onClick={() => {
                        setAddMcpSelected('api-import');
                        setAddMcpLabel('');
                      }}
                      data-testid="mcp-import-json-tile"
                    >
                      <div className="mcp-registry-card-top">
                        <div className="mcp-registry-card-icon">
                          <Download size={16} />
                        </div>
                        <div className="flex-1">
                          <div className="mcp-registry-card-name">{t('mcp.custom.importTileTitle')}</div>
                          <div className="mcp-registry-card-cat">{t('mcp.custom.tileCat')}</div>
                        </div>
                      </div>
                      <div className="mcp-registry-card-desc">{t('mcp.custom.importTileDesc')}</div>
                      <div className="mcp-registry-card-meta">
                        <span className="mcp-kind-badge mcp-kind-badge-api" title={t('mcp.kind.apiTooltip')}>
                          <Globe size={9} /> {t('mcp.kind.api')}
                        </span>
                        <span className="mcp-origin-badge mcp-origin-community">
                          {t('mcp.custom.tileBadge')}
                        </span>
                      </div>
                    </div>
                  ) : null}
                  {customApiVisible && (addMcpKindFilter === 'all' || addMcpKindFilter === 'api') && !selectedCategory ? (
                    <div
                      key="custom-api"
                      className="mcp-registry-card mcp-registry-card-custom"
                      onClick={() => {
                        setAddMcpSelected('api-custom');
                        setAddMcpLabel('');
                      }}
                      data-tour-id="custom-api-tile"
                    >
                      <div className="mcp-registry-card-top">
                        <div className="mcp-registry-card-icon">
                          <Plus size={16} />
                        </div>
                        <div className="flex-1">
                          <div className="mcp-registry-card-name">{t('mcp.custom.tileTitle')}</div>
                          <div className="mcp-registry-card-cat">{t('mcp.custom.tileCat')}</div>
                        </div>
                      </div>
                      <div className="mcp-registry-card-desc">{t('mcp.custom.tileDesc')}</div>
                      <div className="mcp-registry-card-meta">
                        <span className="mcp-kind-badge mcp-kind-badge-api" title={t('mcp.kind.apiTooltip')}>
                          <Globe size={9} /> {t('mcp.kind.api')}
                        </span>
                        <span className="mcp-origin-badge mcp-origin-community">
                          {t('mcp.custom.tileBadge')}
                        </span>
                      </div>
                    </div>
                  ) : null}
                  {catsWithItems.flatMap(cat =>
                    (grouped.get(cat) ?? [])
                      .filter(m => {
                        // Category filter (kind filtering already applied
                        // upstream via `availableRegistry` / addMcpKindFilter).
                        if (selectedCategory && selectedCategory !== cat) return false;
                        // Text search filter
                        if (addMcpSearch && !m.name.toLowerCase().includes(addMcpSearch.toLowerCase()) && !m.tags.some(tag => tag.toLowerCase().includes(addMcpSearch.toLowerCase()))) return false;
                        return true;
                      })
                      .map(m => {
                        const alreadyAdded = configuredServerIds.has(m.id);
                        return (
                          <div
                            key={m.id}
                            className={`mcp-registry-card${alreadyAdded ? ' mcp-registry-card-installed' : ''}`}
                            onClick={() => {
                              setAddMcpSelected(m.id);
                              setAddMcpLabel(alreadyAdded ? `${m.name} (${configs.filter(c => c.server_name === m.name).length + 1})` : m.name);
                              const envInit: Record<string, string> = {};
                              pluginCredentialKeys(m).forEach(k => { envInit[k] = ''; });
                              setAddMcpEnv(envInit);
                            }}
                          >
                            <div className="mcp-registry-card-top">
                              <div className="mcp-registry-card-icon">
                                <Puzzle size={16} />
                              </div>
                              <div className="flex-1">
                                <div className="mcp-registry-card-name">{m.name}</div>
                                <div className="mcp-registry-card-cat">{getCategory(m.tags)}</div>
                              </div>
                              {alreadyAdded && <Check size={14} className="text-info" />}
                            </div>
                            <div className="mcp-registry-card-desc">{m.description}</div>
                            <div className="mcp-registry-card-meta">
                              {(() => {
                                const kind = pluginKind(m);
                                const label = kind === 'api'
                                  ? t('mcp.kind.api')
                                  : kind === 'hybrid'
                                    ? t('mcp.kind.hybrid')
                                    : t('mcp.kind.mcp');
                                const Icon = kind === 'mcp' ? Plug : kind === 'api' ? Globe : Puzzle;
                                return (
                                  <span className={`mcp-kind-badge mcp-kind-badge-${kind}`} title={t(`mcp.kind.${kind}Tooltip`)}>
                                    <Icon size={9} /> {label}
                                  </span>
                                );
                              })()}
                              <span className={`mcp-origin-badge ${m.official ? 'mcp-origin-official' : 'mcp-origin-community'}`}>
                                {m.official ? t('mcp.official') : t('mcp.community')} — {m.publisher}
                              </span>
                              {(m.env_keys.length > 0 || m.token_help) && (
                                pluginCredentialKind(m) === 'cli'
                                  ? <span className="mcp-credential-kind" data-kind="cli"><Terminal size={9} /> {t('mcp.credentials.cliBadge')}</span>
                                  : <span className="mcp-credential-kind" data-kind="api"><Key size={9} /> {t('mcp.credentials.apiBadge')}</span>
                              )}
                            </div>
                          </div>
                        );
                      })
                  )}
                </div>
              </>
            );
          })()}
        </>
      ) : addMcpSelected === 'api-import' ? (
        <>
          {/* 0.8.6 (#33) — Import-from-JSON form. Paste the export
              payload from another Kronn install, validate light
              (name + base_url required), then POST. Credentials are
              NEVER imported even if the JSON contains values. */}
          <div className="mb-5" data-testid="mcp-import-json-form">
            <label className="mcp-field-label">{t('mcp.custom.importPasteLabel')} *</label>
            <textarea
              className="input mcp-input-mono"
              rows={12}
              value={importJsonText}
              onChange={e => { setImportJsonText(e.target.value); if (importJsonError) setImportJsonError(null); }}
              placeholder={t('mcp.custom.importPlaceholder')}
              autoFocus
              data-testid="mcp-import-json-textarea"
            />
          </div>
          <div className="flex-row gap-3 mb-5">
            <button
              type="button"
              className="mcp-btn-action"
              onClick={handlePasteImportJson}
              data-testid="mcp-import-paste-clipboard"
            >
              <Download size={12} /> {t('mcp.custom.importPasteFromClipboard')}
            </button>
            {/* 0.8.6 (#63) — Path B file upload. Hidden input
                triggered by a styled button so the UX matches the
                other action buttons. */}
            <label className="mcp-btn-action" style={{ cursor: 'pointer', display: 'inline-flex' }}>
              <Upload size={12} /> {t('mcp.custom.importFromFile')}
              <input
                type="file"
                accept=".json,application/json"
                style={{ display: 'none' }}
                onChange={e => {
                  const f = e.target.files?.[0];
                  if (f) handleImportFromFile(f);
                  // Reset so re-picking the same file fires onChange again.
                  e.target.value = '';
                }}
                data-testid="mcp-import-file-input"
              />
            </label>
            <button
              type="button"
              className="mcp-btn-action mcp-btn-action-primary"
              onClick={handleImportCustomPlugin}
              disabled={importJsonLoading || !importJsonText.trim()}
              data-testid="mcp-import-submit"
            >
              <Plus size={12} /> {t('mcp.custom.importSubmit')}
            </button>
          </div>
          {importJsonError && (
            <div className="mcp-form-error" data-testid="mcp-import-error">
              {importJsonError}
            </div>
          )}
          <p className="mcp-form-hint">{t('mcp.custom.importSecretsHint')}</p>
        </>
      ) : addMcpSelected === 'api-custom' ? (
        <CustomApiForm state={state} />
      ) : (
        <>
          {/* Label */}
          <div className="mb-5">
            <label className="mcp-field-label">{t('mcp.label')}</label>
            <input
              className="input"
              value={addMcpLabel}
              onChange={(e) => setAddMcpLabel(e.target.value)}
              placeholder={selectedDef?.name ?? 'Label'}
            />
          </div>
          {/* Env vars */}
          {(() => {
            const envKeys = pluginCredentialKeys(
              selectedDef,
              mcpOverview.configs.find(c => c.server_id === addMcpSelected)?.env_keys ?? [],
            );
            const credentialKind = pluginCredentialKind(selectedDef);
            return envKeys.length > 0 ? (
            <div className="mb-5 mcp-credential-fields" data-kind={credentialKind}>
              <div className="flex-row gap-4 mb-3">
                <label className="mcp-field-label mcp-field-label-inline">
                  {t(credentialKind === 'cli' ? 'mcp.credentials.cliTitle' : 'mcp.credentials.apiTitle')}
                </label>
                {selectedDef?.token_url && (
                  <a
                    href={selectedDef.token_url}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="mcp-token-link"
                  >
                    <ExternalLink size={10} />
                    {t(credentialKind === 'cli' ? 'mcp.credentials.createFallback' : 'mcp.getToken')}
                  </a>
                )}
                {!selectedDef?.token_url && selectedDef?.token_help && (
                  <span className="mcp-token-hint">{selectedDef.token_help}</span>
                )}
              </div>
              <div className="mcp-credential-explainer" data-kind={credentialKind} role="note">
                {credentialKind === 'cli' ? <Terminal size={14} /> : <Key size={14} />}
                <span>
                  <strong>{t(credentialKind === 'cli' ? 'mcp.credentials.cliRecommended' : 'mcp.credentials.apiUsedByKronn')}</strong>
                  <small>{t(credentialKind === 'cli' ? 'mcp.credentials.cliOptionalHint' : 'mcp.credentials.apiHint')}</small>
                </span>
              </div>
              {envKeys.map(k => {
                const isVisible = addVisibleFields.has(k);
                // Prefer per-plugin metadata (api_spec.config_keys) over
                // the global ENV_PLACEHOLDERS map — that way any future
                // API plugin gets meaningful placeholders via its own
                // registry entry, no code change needed here.
                const configKey = selectedDef?.api_spec?.config_keys?.find(c => c.env_key === k);
                const hint = configKey?.placeholder
                  ?? ENV_PLACEHOLDERS[k]
                  ?? ENV_PLACEHOLDERS[k.replace(/^.*_/, '')] // fallback: match suffix (e.g. _API_KEY → API_KEY)
                  ?? t('mcp.value');
                // Non-secret config keys are rendered as plain text
                // (no masking) — they're not credentials and hiding
                // them behind dots just makes the form unusable.
                const isPlainTextConfig = !!configKey
                  || (credentialKind === 'cli' && k.endsWith('_HOST'));
                return (
                  <div key={k} className="mb-2">
                    <div className="flex-row gap-4">
                      <span className="mcp-env-key-label">
                        {configKey?.label ?? k}
                      </span>
                      <div className="mcp-env-input-wrap">
                        <input
                          className="input mcp-input-mono mcp-input-with-eye"
                          value={addMcpEnv[k] ?? ''}
                          onChange={(e) => setAddMcpEnv(prev => ({ ...prev, [k]: e.target.value }))}
                          placeholder={hint}
                          type={isPlainTextConfig || isVisible ? 'text' : 'password'}
                        />
                        {!isPlainTextConfig && (
                          <button
                            type="button"
                            className="mcp-eye-btn"
                            onClick={() => setAddVisibleFields(prev => {
                              const next = new Set(prev);
                              if (next.has(k)) next.delete(k); else next.add(k);
                              return next;
                            })}
                            tabIndex={-1}
                          >
                            <Eye size={12} style={{ color: isVisible ? 'var(--kr-accent-ink)' : 'var(--kr-text-ghost)' }} />
                          </button>
                        )}
                      </div>
                    </div>
                    {/* Inline description from api_spec.config_keys
                        (e.g. Chartbeat host explains "the site tracked
                        in Chartbeat"). Static ENV_PLACEHOLDERS map has
                        no equivalent, so this only fires for API
                        plugins. */}
                    {configKey?.description && (
                      <div className="mcp-env-key-desc">{configKey.description}</div>
                    )}
                  </div>
                );
              })}
            </div>
          ) : null; })()}
          {/* Global toggle */}
          <div className="flex-row gap-4 mb-6">
            <button className={`mcp-project-toggle ${addMcpGlobal ? 'mcp-project-toggle-on' : 'mcp-project-toggle-off'}`} onClick={() => setAddMcpGlobal(!addMcpGlobal)}>
              {addMcpGlobal ? <CheckSquare size={11} className="text-accent" /> : <Square size={11} />}
              {t('mcp.globalAll')}
            </button>
            {selectedDef && pluginKind(selectedDef) !== 'api' && (
              <button
                className={`mcp-project-toggle ${addMcpHostSync ? 'mcp-project-toggle-on' : 'mcp-project-toggle-off'}`}
                onClick={() => setAddMcpHostSync(!addMcpHostSync)}
                title={t('mcp.createHostScopeHint')}
              >
                {addMcpHostSync ? <CheckSquare size={11} className="text-accent" /> : <Square size={11} />}
                <Globe size={11} /> {t('mcp.createHostScope')}
              </button>
            )}
          </div>
          {/* Actions */}
          <div className="flex-row gap-4">
            <button
              className="mcp-btn-action mcp-btn-action-primary"
              onClick={handleAddMcpFromRegistry}
            >
              <Check size={14} /> {t('mcp.addBtn')}
            </button>
            <button className="mcp-btn-action" onClick={() => setAddMcpSelected(null)}>
              {t('mcp.back')}
            </button>
          </div>
        </>
      )}
      </div>
    </div>
  );
}

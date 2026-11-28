import {
  Puzzle, Plus, Check, X, Key, ExternalLink, Download,
  Terminal, Eye,
} from 'lucide-react';
import { useLayoutEffect, useRef, type UIEvent } from 'react';
import { onActivationKey } from '../../lib/keyboardActivation';
import { pluginKind } from '../../lib/pluginKind';
import { RegistryKindBadge } from './PluginKindBadge';
import { pluginCredentialKeys, pluginCredentialKind } from '../../lib/pluginCredentials';
import { CustomApiForm } from './CustomApiForm';
import { PluginScopeEditor } from './PluginScopeEditor';
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

/** The card and its focus ring (its scroll margin) lie inside the visible part
 *  of the grid's scroll box, itself clipped to the window. Same rule as the
 *  Git list's rows (KT-1989). */
const cardInView = (card: HTMLElement, box: HTMLElement | null) => {
  const margin = parseFloat(getComputedStyle(card).scrollMarginTop) || 0;
  const cardRect = card.getBoundingClientRect();
  const boxRect = box?.getBoundingClientRect();
  const top = Math.max(boxRect?.top ?? 0, 0);
  const bottom = Math.min(boxRect?.bottom ?? window.innerHeight, window.innerHeight);
  return cardRect.top - margin >= top && cardRect.bottom + margin <= bottom;
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
    addMcpEnv, setAddMcpEnv, addMcpGlobal, setAddMcpGlobal, addMcpProjectIds, setAddMcpProjectIds,
    addMcpHostSync, setAddMcpHostSync,
    addVisibleFields, setAddVisibleFields,
    selectedDef, mcpOverview, projects,
    setPortabilityMode,
    handleAddMcpFromRegistry,
  } = state;

  // KT-2006 review — activating a card unmounts the grid: the step that
  // replaces it takes the focus (its first field, `autoFocus`), and the way
  // back to the catalogue puts the grid at the scroll offset it had, then the
  // focus on the card that was activated (keyboard and mouse alike).
  // The card to come back to is the selection itself (a registry id, or
  // `api-custom`), remembered while its step is open; the grid's offset is
  // kept as it scrolls, since the grid is gone once the step shows.
  const gridRef = useRef<HTMLDivElement | null>(null);
  const gridScrollTopRef = useRef(0);
  const openedCardRef = useRef<string | null>(null);
  const rememberGridScroll = (event: UIEvent<HTMLDivElement>) => {
    gridScrollTopRef.current = event.currentTarget.scrollTop;
  };

  // A layout effect of this component runs after its children's mount: the
  // card's focus wins over the search field's `autoFocus`.
  useLayoutEffect(() => {
    if (!showAddMcp) {
      // Closing the form forgets the card: reopening starts at the search field.
      openedCardRef.current = null;
      gridScrollTopRef.current = 0;
      return;
    }
    if (addMcpSelected) {
      openedCardRef.current = addMcpSelected;
      return;
    }
    const key = openedCardRef.current;
    if (!key) return;
    openedCardRef.current = null;
    const grid = gridRef.current;
    if (!grid) return;
    grid.scrollTop = gridScrollTopRef.current;
    const card = [...grid.querySelectorAll<HTMLElement>('[data-registry-card]')]
      .find(el => el.dataset.registryCard === key);
    // The card is gone (filtered out): the search field keeps its autofocus.
    if (!card) return;
    card.focus({ preventScroll: true });
    // Fallback only: scroll as far as needed to show the card with its whole
    // focus ring (its scroll margin), never to centre it.
    if (!cardInView(card, grid)) {
      card.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
    }
  }, [showAddMcp, addMcpSelected]);

  // KT-2006 — every card is a keyboard stop (Tab, then Enter or Space), so the
  // scrolling grid can be reached and scrolled without a mouse. `key` is the
  // selection the card opens: the way back from its step lands on it.
  const cardActivation = (key: string, activate: () => void) => ({
    'data-registry-card': key,
    role: 'button' as const,
    tabIndex: 0,
    onClick: activate,
    onKeyDown: onActivationKey<HTMLDivElement>(activate),
  });

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
                <div className="mcp-registry-grid" ref={gridRef} onScroll={rememberGridScroll}>
                  {customApiVisible && (addMcpKindFilter === 'all' || addMcpKindFilter === 'api') && !selectedCategory ? (
                    <div
                      key="api-import"
                      className="mcp-registry-card mcp-registry-card-custom"
                      {...cardActivation('api-import', () => {
                        resetAddMcp();
                        setPortabilityMode('import');
                      })}
                      data-testid="mcp-import-bundle-tile"
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
                        <RegistryKindBadge kind="api" />
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
                      {...cardActivation('api-custom', () => {
                        setAddMcpSelected('api-custom');
                        setAddMcpLabel('');
                      })}
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
                        <RegistryKindBadge kind="api" />
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
                            {...cardActivation(m.id, () => {
                              setAddMcpSelected(m.id);
                              setAddMcpLabel(alreadyAdded ? `${m.name} (${configs.filter(c => c.server_name === m.name).length + 1})` : m.name);
                              const envInit: Record<string, string> = {};
                              pluginCredentialKeys(m).forEach(k => { envInit[k] = ''; });
                              setAddMcpEnv(envInit);
                            })}
                          >
                            <div className="mcp-registry-card-top">
                              <div className="mcp-registry-card-icon">
                                <Puzzle size={16} />
                              </div>
                              <div className="flex-1">
                                <div className="mcp-registry-card-name">{m.name}</div>
                                <div className="mcp-registry-card-cat">{getCategory(m.tags)}</div>
                              </div>
                              {/* KT-2006 review — the installed state has a word:
                                  the mark's name joins the card's name (and its
                                  tooltip names the dashed look). */}
                              {alreadyAdded && (
                                <span
                                  className="mcp-registry-card-installed-mark"
                                  role="img"
                                  aria-label={t('mcp.alreadyAdded')}
                                  title={t('mcp.alreadyAdded')}
                                >
                                  <Check size={14} className="text-info" aria-hidden="true" />
                                </span>
                              )}
                            </div>
                            <div className="mcp-registry-card-desc">{m.description}</div>
                            <div className="mcp-registry-card-meta">
                              <RegistryKindBadge kind={pluginKind(m)} />
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
      ) : addMcpSelected === 'api-custom' ? (
        <CustomApiForm state={state} />
      ) : (
        <>
          {/* Label */}
          <div className="mb-5">
            <label className="mcp-field-label">{t('mcp.label')}</label>
            {/* KT-2006 review — the first field of the step that replaces the
                grid takes the focus, so it never falls back to the page. */}
            <input
              className="input"
              autoFocus
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
          {/* Single reusable scope editor (KT-831) — proposes projects
              AND CLI sync at add time, same component as the fiche and
              bundle import. "Général" isn't offered here: the backend
              always starts a new config at `include_general: true`,
              there's nothing to toggle before the fiche (opened right
              after creation) exists. */}
          <div className="mb-6">
            <PluginScopeEditor
              t={t}
              projects={projects}
              isGlobal={addMcpGlobal}
              onToggleGlobal={() => setAddMcpGlobal(!addMcpGlobal)}
              projectIds={addMcpProjectIds}
              onToggleProject={(projectId, isLinked) => setAddMcpProjectIds(
                isLinked ? addMcpProjectIds.filter(id => id !== projectId) : [...addMcpProjectIds, projectId],
              )}
              supportsHostSync={!!selectedDef && pluginKind(selectedDef) !== 'api'}
              hostSync={addMcpHostSync ? 'GlobalOnly' : 'None'}
              onSetHostSync={(mode) => setAddMcpHostSync(mode !== 'None')}
              hostSyncHint={t('mcp.createHostScopeHint')}
              testIdPrefix="mcp-add-scope"
            />
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

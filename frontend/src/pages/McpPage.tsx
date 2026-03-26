import { useState, useRef } from 'react';
import { mcps as mcpsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { isHiddenPath } from '../lib/constants';
import type { Project, McpConfigDisplay, McpDefinition, McpOverview } from '../types/generated';
import {
  Server, Plus, Trash2, Eye, Check, RefreshCw, Square, CheckSquare,
  X, Key, Pencil, FileText, ExternalLink, Save, Search, ChevronRight,
} from 'lucide-react';
import './McpPage.css';

const slugify = (label: string) => label.toLowerCase().replace(/[^a-z0-9]/g, '-').replace(/-+/g, '-').replace(/^-|-$/g, '');

interface McpPageProps {
  projects: Project[];
  mcpOverview: McpOverview;
  mcpRegistry: McpDefinition[];
  refetchMcps: () => void;
}

export function McpPage({ projects, mcpOverview, mcpRegistry, refetchMcps }: McpPageProps) {
  const { t } = useT();
  const [editingLabelId, setEditingLabelId] = useState<string | null>(null);
  const [editingLabelText, setEditingLabelText] = useState('');
  const [showAddMcp, setShowAddMcp] = useState(false);
  const [addMcpSearch, setAddMcpSearch] = useState('');
  const [addMcpSelected, setAddMcpSelected] = useState<string | null>(null);
  const [addMcpLabel, setAddMcpLabel] = useState('');
  const [addMcpEnv, setAddMcpEnv] = useState<Record<string, string>>({});
  const [addMcpGlobal, setAddMcpGlobal] = useState(false);
  const addMcpRef = useRef<HTMLDivElement>(null);
  // Edit secrets
  const [editingEnvId, setEditingEnvId] = useState<string | null>(null);
  const [editingEnv, setEditingEnv] = useState<Record<string, string>>({});
  const [editingEnvLoading, setEditingEnvLoading] = useState(false);
  const [visibleFields, setVisibleFields] = useState<Set<string>>(new Set());
  // MCP context editor
  const [contextEditor, setContextEditor] = useState<{ projectId: string; projectName: string; slug: string; content: string } | null>(null);
  const [contextSaving, setContextSaving] = useState(false);
  // Search & collapse
  const [mcpSearch, setMcpSearch] = useState('');
  const [expandedServers, setExpandedServers] = useState<Set<string>>(new Set());
  // "Show more" for project toggles per config
  const [expandedProjectLists, setExpandedProjectLists] = useState<Set<string>>(new Set());
  const PROJECT_TOGGLE_LIMIT = 10;

  // ── Handlers ──

  const handleSaveLabel = async (configId: string) => {
    if (!editingLabelText.trim()) return;
    try {
      await mcpsApi.updateConfig(configId, { label: editingLabelText.trim() });
      setEditingLabelId(null);
      refetchMcps();
    } catch (e) {
      console.warn('Failed to save label:', e);
    }
  };

  const handleAddMcpFromRegistry = async () => {
    if (!addMcpSelected) return;
    try {
      await mcpsApi.createConfig({
        server_id: addMcpSelected,
        label: addMcpLabel || mcpRegistry.find(m => m.id === addMcpSelected)?.name || 'New MCP',
        env: addMcpEnv,
        args_override: null,
        is_global: addMcpGlobal,
        project_ids: [],
      });
      setShowAddMcp(false);
      setAddMcpSelected(null);
      setAddMcpLabel('');
      setAddMcpEnv({});
      setAddMcpGlobal(false);
      setAddMcpSearch('');
      refetchMcps();
    } catch (e) {
      console.warn('Failed to add MCP config:', e);
    }
  };

  const handleAddDuplicateConfig = (serverId: string, serverName: string) => {
    const def = mcpRegistry.find(m => m.id === serverId);
    const existingConfigs = mcpOverview.configs.filter(c => c.server_id === serverId);
    const existingCount = existingConfigs.length;
    const envInit: Record<string, string> = {};
    const keys = def?.env_keys ?? existingConfigs[0]?.env_keys ?? [];
    keys.forEach(k => { envInit[k] = ''; });
    setAddMcpSelected(serverId);
    setAddMcpLabel(`${serverName} (${existingCount + 1})`);
    setAddMcpEnv(envInit);
    setAddMcpGlobal(false);
    setShowAddMcp(true);
    setAddMcpSearch('');
    setTimeout(() => addMcpRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' }), 50);
  };

  const handleDeleteMcpConfig = async (configId: string) => {
    try {
      await mcpsApi.deleteConfig(configId);
      refetchMcps();
    } catch (e) {
      console.warn('Failed to delete MCP config:', e);
    }
  };

  const handleToggleConfigGlobal = async (config: McpConfigDisplay) => {
    try {
      await mcpsApi.updateConfig(config.id, { is_global: !config.is_global });
      refetchMcps();
    } catch (e) {
      console.warn('Failed to toggle global:', e);
    }
  };

  const handleToggleConfigProject = async (configId: string, projectId: string, currentlyLinked: boolean) => {
    const config = mcpOverview.configs.find(c => c.id === configId);
    if (!config) return;
    const newIds = currentlyLinked
      ? config.project_ids.filter(id => id !== projectId)
      : [...config.project_ids, projectId];
    try {
      await mcpsApi.setConfigProjects(configId, { project_ids: newIds });
      refetchMcps();
    } catch (e) {
      console.warn('Failed to toggle project:', e);
    }
  };

  const handleStartEditSecrets = async (configId: string) => {
    if (editingEnvId === configId) { setEditingEnvId(null); return; }
    setEditingEnvLoading(true);
    setVisibleFields(new Set());
    try {
      const entries = await mcpsApi.revealSecrets(configId);
      const env: Record<string, string> = {};
      entries.forEach(e => { env[e.key] = e.masked_value; });
      setEditingEnv(env);
      setEditingEnvId(configId);
    } catch (e) {
      console.warn('Failed to load secrets:', e);
    } finally {
      setEditingEnvLoading(false);
    }
  };

  const handleSaveSecrets = async () => {
    if (!editingEnvId) return;
    setEditingEnvLoading(true);
    try {
      await mcpsApi.updateConfig(editingEnvId, { env: editingEnv });
      setEditingEnvId(null);
      refetchMcps();
    } catch (e) {
      console.warn('Failed to save secrets:', e);
    } finally {
      setEditingEnvLoading(false);
    }
  };

  const toggleFieldVisibility = (key: string) => {
    setVisibleFields(prev => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key); else next.add(key);
      return next;
    });
  };

  const handleOpenContext = async (projectId: string, projectName: string, configLabel: string) => {
    // Slugify the label (same algo as backend)
    const slug = slugify(configLabel);
    try {
      const entry = await mcpsApi.getContext(projectId, slug);
      setContextEditor({ projectId, projectName, slug, content: entry.content });
    } catch {
      // File might not exist yet — create with empty marker
      setContextEditor({ projectId, projectName, slug, content: `# ${configLabel} — Usage Context\n\n> Instructions for AI agents using **${configLabel}** in this project.\n> Edit this file with project-specific rules.\n\n## Rules\n\n` });
    }
  };

  const handleSaveContext = async () => {
    if (!contextEditor) return;
    setContextSaving(true);
    try {
      await mcpsApi.updateContext(contextEditor.projectId, contextEditor.slug, contextEditor.content);
      setContextEditor(null);
    } catch (e) {
      console.warn('Failed to save context:', e);
    } finally {
      setContextSaving(false);
    }
  };

  // ── Computed ──

  const { servers, configs } = mcpOverview;
  const totalConfigs = configs.length;
  const globalConfigs = configs.filter(c => c.is_global);

  const configsByServer = new Map<string, { serverId: string; serverName: string; configs: McpConfigDisplay[] }>();
  for (const c of configs) {
    const key = c.server_name || c.server_id;
    const existing = configsByServer.get(key) ?? { serverId: c.server_id, serverName: key, configs: [] };
    existing.configs.push(c);
    configsByServer.set(key, existing);
  }

  const configuredServerIds = new Set(configs.map(c => c.server_id));
  const availableRegistry = mcpRegistry.filter(m =>
    (!addMcpSearch || m.name.toLowerCase().includes(addMcpSearch.toLowerCase()) || m.tags.some(tag => tag.includes(addMcpSearch.toLowerCase())))
  );
  const selectedDef = mcpRegistry.find(m => m.id === addMcpSelected);

  // Filter configs by search
  const searchLower = mcpSearch.toLowerCase();
  const filteredConfigsByServer = new Map<string, { serverId: string; serverName: string; configs: McpConfigDisplay[] }>();
  for (const [serverName, group] of configsByServer) {
    if (!mcpSearch) {
      filteredConfigsByServer.set(serverName, group);
    } else {
      const nameMatch = serverName.toLowerCase().includes(searchLower);
      const filteredConfigs = group.configs.filter(c =>
        nameMatch || c.label.toLowerCase().includes(searchLower) ||
        c.project_names.some(n => n.toLowerCase().includes(searchLower))
      );
      if (filteredConfigs.length > 0) {
        filteredConfigsByServer.set(serverName, { ...group, configs: filteredConfigs });
      }
    }
  }
  const toggleServer = (name: string) => {
    setExpandedServers(prev => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name); else next.add(name);
      return next;
    });
  };

  // ── Render ──

  return (
    <div>
      <div className="mcp-page-header">
        <div>
          <h1 className="mcp-h1">{t('mcp.title')}</h1>
          <p className="mcp-meta">
            {totalConfigs} {totalConfigs > 1 ? t('mcp.configPlural') : t('mcp.config')} · {servers.length} {servers.length > 1 ? t('mcp.serverPlural') : t('mcp.server')} · {globalConfigs.length} {globalConfigs.length > 1 ? t('mcp.globalPlural') : t('mcp.global')}
          </p>
        </div>
        <div className="flex-row gap-4">
          <button className="mcp-btn-action mcp-btn-action-primary" onClick={() => { setShowAddMcp(true); setAddMcpSelected(null); setAddMcpSearch(''); }} title={t('mcp.addTitle')}>
            <Plus size={14} /> {t('mcp.add')}
          </button>
          <button className="mcp-btn-action" onClick={async () => { try { await mcpsApi.refresh(); refetchMcps(); } catch (e) { console.warn('Failed to refresh MCPs:', e); } }} title={t('mcp.detect')}>
            <RefreshCw size={14} /> {t('mcp.detect')}
          </button>
        </div>
      </div>

      {/* ── Add MCP from registry ── */}
      {showAddMcp && (
        <div ref={addMcpRef} className="mcp-card mcp-add-panel">
          <div className="mcp-add-header">
            <h3 className="mcp-add-title">
              {addMcpSelected ? t('mcp.configure', selectedDef?.name ?? addMcpLabel) : t('mcp.addTitle')}
            </h3>
            <button className="mcp-icon-btn" onClick={() => { setShowAddMcp(false); setAddMcpSelected(null); }} aria-label="Close">
              <X size={14} />
            </button>
          </div>

          {!addMcpSelected ? (
            <>
              <input
                className="input mb-5"
                placeholder={t('mcp.searchRegistry')}
                value={addMcpSearch}
                onChange={(e) => setAddMcpSearch(e.target.value)}
                autoFocus
              />
              <div className="mcp-registry-list">
                {(() => {
                  const categoryMap: Record<string, string> = {
                    git: t('mcp.cat.gitCode'), code: t('mcp.cat.gitCode'),
                    database: t('mcp.cat.databases'), sql: t('mcp.cat.databases'), cache: t('mcp.cat.databases'), embedded: t('mcp.cat.databases'),
                    cloud: t('mcp.cat.cloud'), containers: t('mcp.cat.cloud'), devops: t('mcp.cat.cloud'),
                    search: t('mcp.cat.search'), web: t('mcp.cat.search'), http: t('mcp.cat.search'), browser: t('mcp.cat.search'), scraping: t('mcp.cat.search'),
                    monitoring: t('mcp.cat.monitoring'), analytics: t('mcp.cat.monitoring'), errors: t('mcp.cat.monitoring'),
                    communication: t('mcp.cat.communication'), chat: t('mcp.cat.communication'), email: t('mcp.cat.communication'), mailing: t('mcp.cat.communication'),
                    'project-management': t('mcp.cat.projectMgmt'), issues: t('mcp.cat.projectMgmt'),
                    core: t('mcp.cat.utilities'), filesystem: t('mcp.cat.utilities'), docs: t('mcp.cat.utilities'), libraries: t('mcp.cat.utilities'),
                  };
                  const getCategory = (tags: string[]) => {
                    for (const tag of tags) { if (categoryMap[tag]) return categoryMap[tag]; }
                    return t('mcp.cat.other');
                  };
                  const categoryOrder = [t('mcp.cat.gitCode'), t('mcp.cat.databases'), t('mcp.cat.cloud'), t('mcp.cat.search'), t('mcp.cat.monitoring'), t('mcp.cat.communication'), t('mcp.cat.projectMgmt'), t('mcp.cat.utilities'), t('mcp.cat.other')];
                  const grouped = new Map<string, typeof availableRegistry>();
                  for (const m of availableRegistry) {
                    const cat = getCategory(m.tags);
                    if (!grouped.has(cat)) grouped.set(cat, []);
                    grouped.get(cat)!.push(m);
                  }
                  return categoryOrder.filter(cat => grouped.has(cat)).map(cat => (
                    <div key={cat}>
                      <div className="mcp-category-header">{cat}</div>
                      {grouped.get(cat)!.map(m => {
                        const alreadyAdded = configuredServerIds.has(m.id);
                        return (
                          <div
                            key={m.id}
                            className="mcp-registry-item"
                            onClick={() => {
                              setAddMcpSelected(m.id);
                              setAddMcpLabel(alreadyAdded ? `${m.name} (${configs.filter(c => c.server_name === m.name).length + 1})` : m.name);
                              const envInit: Record<string, string> = {};
                              m.env_keys.forEach(k => { envInit[k] = ''; });
                              setAddMcpEnv(envInit);
                            }}
                          >
                            <Server size={12} className={alreadyAdded ? 'text-info' : 'text-accent'} style={{ flexShrink: 0 }} />
                            <div className="flex-1">
                              <div className="flex-row gap-3">
                                <span className="mcp-registry-item-name">{m.name}</span>
                                {alreadyAdded && <span className="mcp-already-badge">{t('mcp.alreadyAdded')}</span>}
                                {m.env_keys.length > 0 && <span className="mcp-registry-item-keys">{m.env_keys.length} {m.env_keys.length > 1 ? t('mcp.keysPlural') : t('mcp.keys')}</span>}
                              </div>
                              <div className="mcp-registry-item-desc">{m.description}</div>
                            </div>
                            <Plus size={14} className="text-ghost" />
                          </div>
                        );
                      })}
                    </div>
                  ));
                })()}
              </div>
            </>
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
                const envKeys = selectedDef?.env_keys ?? mcpOverview.configs.find(c => c.server_id === addMcpSelected)?.env_keys ?? [];
                return envKeys.length > 0 ? (
                <div className="mb-5">
                  <div className="flex-row gap-4 mb-3">
                    <label className="mcp-field-label mcp-field-label-inline">{t('mcp.envVars')}</label>
                    {selectedDef?.token_url && (
                      <a
                        href={selectedDef.token_url}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="mcp-token-link"
                      >
                        <ExternalLink size={10} />
                        {selectedDef.token_help ?? t('mcp.getToken')}
                      </a>
                    )}
                    {!selectedDef?.token_url && selectedDef?.token_help && (
                      <span className="mcp-token-hint">{selectedDef.token_help}</span>
                    )}
                  </div>
                  {envKeys.map(k => (
                    <div key={k} className="flex-row gap-4 mb-2">
                      <span className="mcp-env-key-label">{k}</span>
                      <input
                        className="input mcp-input-mono"
                        value={addMcpEnv[k] ?? ''}
                        onChange={(e) => setAddMcpEnv(prev => ({ ...prev, [k]: e.target.value }))}
                        placeholder={t('mcp.value')}
                        type="password"
                      />
                    </div>
                  ))}
                </div>
              ) : null; })()}
              {/* Global toggle */}
              <div className="flex-row gap-4 mb-6">
                <button className={`mcp-project-toggle ${addMcpGlobal ? 'mcp-project-toggle-on' : 'mcp-project-toggle-off'}`} onClick={() => setAddMcpGlobal(!addMcpGlobal)}>
                  {addMcpGlobal ? <CheckSquare size={11} className="text-accent" /> : <Square size={11} />}
                  {t('mcp.globalAll')}
                </button>
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
      )}

      {/* ── Search bar ── */}
      {totalConfigs > 3 && (
        <div className="mcp-search-wrap">
          <Search size={14} className="mcp-search-icon" />
          <input
            className="input mcp-search-input"
            placeholder={t('mcp.search')}
            value={mcpSearch}
            onChange={(e) => setMcpSearch(e.target.value)}
          />
          {mcpSearch && (
            <button
              className="mcp-search-clear"
              onClick={() => setMcpSearch('')}
              aria-label="Clear search"
            >
              <X size={12} />
            </button>
          )}
        </div>
      )}

      {/* ── Configured MCPs ── */}
      {totalConfigs > 0 ? (
        <>
          {[...filteredConfigsByServer.entries()].map(([serverName, group]) => {
            const isExpanded = expandedServers.has(serverName) || !!mcpSearch;
            const linkedCount = group.configs.reduce((sum, c) => sum + (c.is_global ? projects.length : c.project_ids.length), 0);
            return (
            <div key={serverName} className="mcp-server-group">
              <button
                className="mcp-server-header"
                onClick={() => toggleServer(serverName)}
                aria-expanded={isExpanded}
                style={isExpanded ? { marginBottom: 8 } : undefined}
              >
                <ChevronRight size={13} className={`mcp-server-chevron${isExpanded ? ' mcp-server-chevron-open' : ''}`} />
                <Server size={13} className="text-accent" />
                <h2 className="mcp-server-label">
                  {serverName}
                </h2>
                <span className="mcp-server-meta">
                  {group.configs.length} {group.configs.length > 1 ? t('mcp.configPlural') : t('mcp.config')}
                  {!isExpanded && linkedCount > 0 && ` · ${linkedCount} ${linkedCount > 1 ? t('mcp.projectPlural') : t('mcp.project')}`}
                </span>
                {(() => {
                  const serverIncomp = mcpOverview.incompatibilities.filter(i => i.server_id === group.serverId);
                  return serverIncomp.length > 0 ? (
                    <span
                      title={serverIncomp.map(i => `⚠ ${i.agent}: ${i.reason}`).join('\n')}
                      className="mcp-server-incompat"
                    >
                      ⚠ {serverIncomp.map(i => i.agent).join(', ')}
                    </span>
                  ) : null;
                })()}
                {group.configs.some(c => c.env_keys.length > 0) && (
                  <button
                    className="mcp-icon-btn"
                    style={{ marginLeft: 4, color: 'rgba(255,255,255,0.3)' }}
                    onClick={(e) => { e.stopPropagation(); handleAddDuplicateConfig(group.serverId, serverName); }}
                    title={t('mcp.addAnother', serverName)}
                    aria-label={t('mcp.addAnother', serverName)}
                  >
                    <Plus size={12} />
                  </button>
                )}
              </button>

              {isExpanded && group.configs.map(cfg => {
                const isEditingLabel = editingLabelId === cfg.id;
                return (
                  <div key={cfg.id} className="mcp-config-card">
                    <div className="flex-start gap-5 flex-1">
                      <div className={`mcp-config-dot ${cfg.is_global ? 'mcp-config-dot-global' : 'mcp-config-dot-project'}`} />
                      <div className="flex-1">
                        {/* Header: label (editable) + badges */}
                        <div className="mcp-config-header">
                          {isEditingLabel ? (
                            <input
                              className="input mcp-config-label-input"
                              value={editingLabelText}
                              onChange={(e) => setEditingLabelText(e.target.value)}
                              onBlur={() => handleSaveLabel(cfg.id)}
                              onKeyDown={(e) => { if (e.key === 'Enter') handleSaveLabel(cfg.id); if (e.key === 'Escape') setEditingLabelId(null); }}
                              autoFocus
                            />
                          ) : (
                            <span
                              className="mcp-config-label"
                              onClick={() => { setEditingLabelId(cfg.id); setEditingLabelText(cfg.label); }}
                              title={t('mcp.clickToRename')}
                            >
                              {cfg.label}
                              <Pencil size={9} className="text-ghost" style={{ marginLeft: 4 }} />
                            </span>
                          )}
                          {cfg.project_names.length > 0 && (
                            <span className="mcp-source-badge" title={cfg.project_names.join(', ')}>
                              {cfg.project_names.length <= 3
                                ? cfg.project_names.join(', ')
                                : `${cfg.project_names.slice(0, 3).join(', ')} +${cfg.project_names.length - 3}`}
                            </span>
                          )}
                        </div>

                        {/* Env keys */}
                        {cfg.env_keys.length > 0 && (
                          <>
                            <div className="mcp-env-keys-row">
                              <Key size={10} className="text-dim" />
                              {editingEnvId !== cfg.id && cfg.env_keys.map(k => (
                                <span key={k} className="mcp-env-key-name">{k}</span>
                              ))}
                              <button
                                className="mcp-icon-btn"
                                style={{ marginLeft: 4 }}
                                onClick={() => handleStartEditSecrets(cfg.id)}
                                title={editingEnvId === cfg.id ? t('mcp.close') : t('mcp.editKeys')}
                                aria-label={editingEnvId === cfg.id ? t('mcp.close') : t('mcp.editKeys')}
                              >
                                <Pencil size={11} style={{ color: editingEnvId === cfg.id ? 'var(--kr-accent)' : 'rgba(255,255,255,0.3)' }} />
                              </button>
                            </div>
                            {/* Inline edit secrets form */}
                            {editingEnvId === cfg.id && (() => {
                              const def = mcpRegistry.find(m => m.id === cfg.server_id);
                              return (
                                <div className="mcp-secrets-editor">
                                  {def?.token_url && (
                                    <a
                                      href={def.token_url}
                                      target="_blank"
                                      rel="noopener noreferrer"
                                      className="mcp-secrets-token-link"
                                    >
                                      <ExternalLink size={10} />
                                      {def.token_help ?? t('mcp.getToken')}
                                    </a>
                                  )}
                                  {!def?.token_url && def?.token_help && (
                                    <p className="mcp-secrets-hint">{def.token_help}</p>
                                  )}
                                  {cfg.env_keys.map(k => (
                                    <div key={k} className="flex-row gap-4 mb-2">
                                      <span className="mcp-env-key-label">{k}</span>
                                      <input
                                        className="input mcp-input-mono"
                                        value={editingEnv[k] ?? ''}
                                        onChange={e => setEditingEnv(prev => ({ ...prev, [k]: e.target.value }))}
                                        type={visibleFields.has(k) ? 'text' : 'password'}
                                        placeholder={t('mcp.value')}
                                      />
                                      <button
                                        className="mcp-icon-btn flex-shrink-0"
                                        style={{ padding: '4px 6px' }}
                                        onClick={() => toggleFieldVisibility(k)}
                                        title={visibleFields.has(k) ? t('mcp.hide') : t('mcp.show')}
                                        aria-label={visibleFields.has(k) ? t('mcp.hide') : t('mcp.show')}
                                      >
                                        <Eye size={11} style={{ color: visibleFields.has(k) ? 'var(--kr-accent)' : 'rgba(255,255,255,0.25)' }} />
                                      </button>
                                    </div>
                                  ))}
                                  <div className="flex-row gap-3 mt-4">
                                    <button
                                      className="mcp-btn-action mcp-btn-action-primary"
                                      onClick={handleSaveSecrets}
                                      disabled={editingEnvLoading}
                                    >
                                      <Save size={12} /> {editingEnvLoading ? t('mcp.saving') : t('mcp.save')}
                                    </button>
                                    <button className="mcp-btn-action" onClick={() => setEditingEnvId(null)}>{t('mcp.cancel')}</button>
                                  </div>
                                </div>
                              );
                            })()}
                          </>
                        )}

                        {/* Global label + project toggles */}
                        <div className="mcp-toggle-row">
                          <span
                            className={`mcp-toggle-label mcp-toggle-global${cfg.is_global ? ' mcp-toggle-global-active' : ''}`}
                            onClick={() => handleToggleConfigGlobal(cfg)}
                            title={cfg.is_global ? t('mcp.disableGlobal') : t('mcp.enableGlobal')}
                          >
                            Global
                          </span>
                          <span
                            className={`mcp-toggle-label mcp-toggle-general${cfg.include_general ? ' mcp-toggle-general-active' : ''}`}
                            onClick={async () => { try { await mcpsApi.updateConfig(cfg.id, { include_general: !cfg.include_general }); refetchMcps(); } catch (e) { console.warn('Failed to toggle general:', e); } }}
                            title={cfg.include_general ? t('mcp.disableGeneral') : t('mcp.enableGeneral')}
                          >
                            {t('mcp.general')}
                          </span>
                          <span className="mcp-separator">|</span>
                          {(() => {
                            const sorted = projects.filter(p => !isHiddenPath(p.path)).sort((a, b) => {
                              const aLinked = (cfg.is_global || cfg.project_ids.includes(a.id)) ? 0 : 1;
                              const bLinked = (cfg.is_global || cfg.project_ids.includes(b.id)) ? 0 : 1;
                              return aLinked - bLinked || a.name.localeCompare(b.name);
                            });
                            const showAll = expandedProjectLists.has(cfg.id);
                            const visible = showAll ? sorted : sorted.slice(0, PROJECT_TOGGLE_LIMIT);
                            const hiddenCount = sorted.length - visible.length;
                            return (<>
                              {visible.map(proj => {
                                const isLinked = cfg.is_global || cfg.project_ids.includes(proj.id);
                                return (
                                  <span key={proj.id} className="flex-row">
                                    <button
                                      className={`mcp-project-toggle ${isLinked ? 'mcp-project-toggle-on' : 'mcp-project-toggle-off'}`}
                                      onClick={() => handleToggleConfigProject(cfg.id, proj.id, isLinked)}
                                    >
                                      {isLinked ? <CheckSquare size={11} className="text-accent" /> : <Square size={11} />}
                                      {proj.name}
                                    </button>
                                    {isLinked && (() => {
                                      const slug = slugify(cfg.label);
                                      const isCustomized = mcpOverview.customized_contexts.includes(`${slug}:${proj.id}`);
                                      return (
                                        <button
                                          className="mcp-icon-btn mcp-context-btn"
                                          onClick={() => handleOpenContext(proj.id, proj.name, cfg.label)}
                                          title={`${t('mcp.editContext', cfg.label, proj.name)}${isCustomized ? ' ' + t('mcp.customized') : ' ' + t('mcp.default')}`}
                                          aria-label={`${t('mcp.editContext', cfg.label, proj.name)}`}
                                        >
                                          <FileText size={10} style={{ color: isCustomized ? 'var(--kr-accent)' : 'rgba(255,255,255,0.2)' }} />
                                        </button>
                                      );
                                    })()}
                                  </span>
                                );
                              })}
                              {hiddenCount > 0 && (
                                <button
                                  className="mcp-more-projects-btn"
                                  onClick={() => setExpandedProjectLists(prev => { const n = new Set(prev); n.add(cfg.id); return n; })}
                                >
                                  {t('mcp.moreProjects', hiddenCount)}
                                </button>
                              )}
                              {showAll && sorted.length > PROJECT_TOGGLE_LIMIT && (
                                <button
                                  className="mcp-less-projects-btn"
                                  onClick={() => setExpandedProjectLists(prev => { const n = new Set(prev); n.delete(cfg.id); return n; })}
                                >
                                  {t('mcp.lessProjects')}
                                </button>
                              )}
                            </>);
                          })()}
                        </div>
                      </div>

                      {/* Delete button */}
                      <button
                        className="mcp-icon-btn text-error"
                        onClick={() => handleDeleteMcpConfig(cfg.id)}
                        title={t('mcp.deleteConfig')}
                        aria-label={t('mcp.deleteConfig')}
                      >
                        <Trash2 size={12} />
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>
          );
          })}
        </>
      ) : !showAddMcp ? (
        <div className="mcp-card mcp-empty">
          <Server size={32} className="text-ghost mb-6" />
          <p className="mcp-empty-text">
            {t('mcp.empty')}
          </p>
        </div>
      ) : null}

      {/* ── MCP Context Editor Modal ── */}
      {contextEditor && (
        <div className="mcp-modal-overlay" onClick={() => setContextEditor(null)}>
          <div
            className="mcp-modal"
            onClick={e => e.stopPropagation()}
            role="dialog"
            aria-modal="true"
            aria-labelledby="context-editor-title"
            onKeyDown={e => { if (e.key === 'Escape') setContextEditor(null); }}
          >
            <div className="flex-between">
              <div>
                <h3 id="context-editor-title" className="mcp-modal-title">
                  <FileText size={14} className="text-accent" style={{ marginRight: 6 }} />
                  {t('mcp.contextTitle', contextEditor.slug.replace(/-/g, ' '))}
                </h3>
                <p className="mcp-modal-subtitle">
                  {t('mcp.contextInfo', contextEditor.projectName, contextEditor.slug)}
                </p>
              </div>
              <button className="mcp-icon-btn" onClick={() => setContextEditor(null)} aria-label="Close"><X size={14} /></button>
            </div>

            <textarea
              className="input mcp-modal-textarea"
              value={contextEditor.content}
              onChange={e => setContextEditor(prev => prev ? { ...prev, content: e.target.value } : null)}
              placeholder={t('mcp.contextPlaceholder')}
            />

            <div className="flex-row gap-4" style={{ justifyContent: 'flex-end' }}>
              <button className="mcp-btn-action" onClick={() => setContextEditor(null)}>{t('mcp.cancel')}</button>
              <button
                className="mcp-btn-action mcp-btn-action-primary"
                onClick={handleSaveContext}
                disabled={contextSaving}
              >
                {contextSaving ? t('mcp.saving') : t('mcp.save')}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

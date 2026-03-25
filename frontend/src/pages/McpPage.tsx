import { useState, useRef } from 'react';
import { mcps as mcpsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { isHiddenPath } from '../lib/constants';
import type { Project, McpConfigDisplay, McpDefinition, McpOverview } from '../types/generated';
import {
  Server, Plus, Trash2, Eye, Check, RefreshCw, Square, CheckSquare,
  X, Key, Pencil, FileText, ExternalLink, Save, Search, ChevronRight,
} from 'lucide-react';

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
      <div style={s.pageHeader}>
        <div>
          <h1 style={s.h1}>{t('mcp.title')}</h1>
          <p style={s.meta}>
            {totalConfigs} {totalConfigs > 1 ? t('mcp.configPlural') : t('mcp.config')} · {servers.length} {servers.length > 1 ? t('mcp.serverPlural') : t('mcp.server')} · {globalConfigs.length} {globalConfigs.length > 1 ? t('mcp.globalPlural') : t('mcp.global')}
          </p>
        </div>
        <div style={{ display: 'flex', gap: 8 }}>
          <button style={{ ...s.scanBtn, background: 'rgba(200,255,0,0.1)', color: '#c8ff00' }} onClick={() => { setShowAddMcp(true); setAddMcpSelected(null); setAddMcpSearch(''); }} title={t('mcp.addTitle')}>
            <Plus size={14} /> {t('mcp.add')}
          </button>
          <button style={s.scanBtn} onClick={async () => { try { await mcpsApi.refresh(); refetchMcps(); } catch (e) { console.warn('Failed to refresh MCPs:', e); } }} title={t('mcp.detect')}>
            <RefreshCw size={14} /> {t('mcp.detect')}
          </button>
        </div>
      </div>

      {/* ── Add MCP from registry ── */}
      {showAddMcp && (
        <div ref={addMcpRef} style={{ ...s.card, marginBottom: 20, border: '1px solid rgba(200,255,0,0.2)' }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 12 }}>
            <h3 style={{ fontSize: 14, fontWeight: 600, color: '#c8ff00', margin: 0 }}>
              {addMcpSelected ? t('mcp.configure', selectedDef?.name ?? addMcpLabel) : t('mcp.addTitle')}
            </h3>
            <button style={s.iconBtn} onClick={() => { setShowAddMcp(false); setAddMcpSelected(null); }} aria-label="Close">
              <X size={14} />
            </button>
          </div>

          {!addMcpSelected ? (
            <>
              <input
                style={{ ...s.input, marginBottom: 10 }}
                placeholder={t('mcp.searchRegistry')}
                value={addMcpSearch}
                onChange={(e) => setAddMcpSearch(e.target.value)}
                autoFocus
              />
              <div style={{ display: 'flex', flexDirection: 'column', gap: 2, maxHeight: 400, overflowY: 'auto' }}>
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
                      <div style={s.categoryHeader}>{cat}</div>
                      {grouped.get(cat)!.map(m => {
                        const alreadyAdded = configuredServerIds.has(m.id);
                        return (
                          <div
                            key={m.id}
                            style={s.registryItem}
                            onClick={() => {
                              setAddMcpSelected(m.id);
                              setAddMcpLabel(alreadyAdded ? `${m.name} (${configs.filter(c => c.server_name === m.name).length + 1})` : m.name);
                              const envInit: Record<string, string> = {};
                              m.env_keys.forEach(k => { envInit[k] = ''; });
                              setAddMcpEnv(envInit);
                            }}
                          >
                            <Server size={12} style={{ color: alreadyAdded ? '#00d4ff' : '#c8ff00', flexShrink: 0 }} />
                            <div style={{ flex: 1 }}>
                              <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                                <span style={{ fontWeight: 600, fontSize: 12 }}>{m.name}</span>
                                {alreadyAdded && <span style={s.alreadyBadge}>{t('mcp.alreadyAdded')}</span>}
                                {m.env_keys.length > 0 && <span style={{ fontSize: 9, color: 'rgba(255,255,255,0.4)' }}>{m.env_keys.length} {m.env_keys.length > 1 ? t('mcp.keysPlural') : t('mcp.keys')}</span>}
                              </div>
                              <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.35)', marginTop: 1 }}>{m.description}</div>
                            </div>
                            <Plus size={14} style={{ color: 'rgba(255,255,255,0.2)' }} />
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
              <div style={{ marginBottom: 10 }}>
                <label style={s.fieldLabel}>{t('mcp.label')}</label>
                <input
                  style={s.input}
                  value={addMcpLabel}
                  onChange={(e) => setAddMcpLabel(e.target.value)}
                  placeholder={selectedDef?.name ?? 'Label'}
                />
              </div>
              {/* Env vars */}
              {(() => {
                const envKeys = selectedDef?.env_keys ?? mcpOverview.configs.find(c => c.server_id === addMcpSelected)?.env_keys ?? [];
                return envKeys.length > 0 ? (
                <div style={{ marginBottom: 10 }}>
                  <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 6 }}>
                    <label style={{ ...s.fieldLabel, margin: 0 }}>{t('mcp.envVars')}</label>
                    {selectedDef?.token_url && (
                      <a
                        href={selectedDef.token_url}
                        target="_blank"
                        rel="noopener noreferrer"
                        style={{ display: 'inline-flex', alignItems: 'center', gap: 4, fontSize: 11, color: '#00d4ff', textDecoration: 'none' }}
                      >
                        <ExternalLink size={10} />
                        {selectedDef.token_help ?? t('mcp.getToken')}
                      </a>
                    )}
                    {!selectedDef?.token_url && selectedDef?.token_help && (
                      <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.35)' }}>{selectedDef.token_help}</span>
                    )}
                  </div>
                  {envKeys.map(k => (
                    <div key={k} style={{ display: 'flex', gap: 8, marginBottom: 4, alignItems: 'center' }}>
                      <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.5)', fontFamily: 'monospace', minWidth: 180 }}>{k}</span>
                      <input
                        style={{ ...s.input, flex: 1, fontFamily: 'monospace', fontSize: 11 }}
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
              <div style={{ display: 'flex', gap: 8, marginBottom: 12, alignItems: 'center' }}>
                <button style={s.projectToggle(addMcpGlobal)} onClick={() => setAddMcpGlobal(!addMcpGlobal)}>
                  {addMcpGlobal ? <CheckSquare size={11} style={{ color: '#c8ff00' }} /> : <Square size={11} />}
                  {t('mcp.globalAll')}
                </button>
              </div>
              {/* Actions */}
              <div style={{ display: 'flex', gap: 8 }}>
                <button
                  style={{ ...s.scanBtn, background: 'rgba(200,255,0,0.15)', color: '#c8ff00' }}
                  onClick={handleAddMcpFromRegistry}
                >
                  <Check size={14} /> {t('mcp.addBtn')}
                </button>
                <button style={s.scanBtn} onClick={() => setAddMcpSelected(null)}>
                  {t('mcp.back')}
                </button>
              </div>
            </>
          )}
        </div>
      )}

      {/* ── Search bar ── */}
      {totalConfigs > 3 && (
        <div style={{ position: 'relative', marginBottom: 16 }}>
          <Search size={14} style={{ position: 'absolute', left: 10, top: '50%', transform: 'translateY(-50%)', color: 'rgba(255,255,255,0.25)', pointerEvents: 'none' }} />
          <input
            style={{ ...s.input, paddingLeft: 32, width: '100%' }}
            placeholder={t('mcp.search')}
            value={mcpSearch}
            onChange={(e) => setMcpSearch(e.target.value)}
          />
          {mcpSearch && (
            <button
              style={{ position: 'absolute', right: 8, top: '50%', transform: 'translateY(-50%)', background: 'none', border: 'none', cursor: 'pointer', color: 'rgba(255,255,255,0.3)', padding: 2 }}
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
            <div key={serverName} style={{ marginBottom: 12 }}>
              <button
                style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: isExpanded ? 8 : 0, cursor: 'pointer', padding: '4px 0', background: 'none', border: 'none', width: '100%', font: 'inherit', color: 'inherit', textAlign: 'left' as const }}
                onClick={() => toggleServer(serverName)}
                aria-expanded={isExpanded}
              >
                <ChevronRight size={13} style={{ color: '#c8ff00', transform: isExpanded ? 'rotate(90deg)' : 'none', transition: 'transform 0.15s', flexShrink: 0 }} />
                <Server size={13} style={{ color: '#c8ff00' }} />
                <h2 style={s.sectionLabel}>
                  {serverName}
                </h2>
                <span style={{ fontWeight: 400, fontSize: 11, color: 'rgba(255,255,255,0.4)' }}>
                  {group.configs.length} {group.configs.length > 1 ? t('mcp.configPlural') : t('mcp.config')}
                  {!isExpanded && linkedCount > 0 && ` · ${linkedCount} ${linkedCount > 1 ? t('mcp.projectPlural') : t('mcp.project')}`}
                </span>
                {(() => {
                  const serverIncomp = mcpOverview.incompatibilities.filter(i => i.server_id === group.serverId);
                  return serverIncomp.length > 0 ? (
                    <span
                      title={serverIncomp.map(i => `⚠ ${i.agent}: ${i.reason}`).join('\n')}
                      style={{ fontSize: 10, color: '#ff6b6b', background: 'rgba(255,107,107,0.1)', padding: '1px 6px', borderRadius: 4, fontWeight: 500 }}
                    >
                      ⚠ {serverIncomp.map(i => i.agent).join(', ')}
                    </span>
                  ) : null;
                })()}
                {group.configs.some(c => c.env_keys.length > 0) && (
                  <button
                    style={{ ...s.iconBtn, marginLeft: 4, color: 'rgba(255,255,255,0.3)' }}
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
                  <div key={cfg.id} style={s.mcpCard}>
                    <div style={{ display: 'flex', alignItems: 'flex-start', gap: 10, flex: 1 }}>
                      <div style={{ width: 8, height: 8, borderRadius: '50%', background: cfg.is_global ? '#c8ff00' : '#00d4ff', marginTop: 5, flexShrink: 0 }} />
                      <div style={{ flex: 1 }}>
                        {/* Header: label (editable) + badges */}
                        <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
                          {isEditingLabel ? (
                            <input
                              style={{ ...s.input, width: 180, fontSize: 13, fontWeight: 600, padding: '2px 6px' }}
                              value={editingLabelText}
                              onChange={(e) => setEditingLabelText(e.target.value)}
                              onBlur={() => handleSaveLabel(cfg.id)}
                              onKeyDown={(e) => { if (e.key === 'Enter') handleSaveLabel(cfg.id); if (e.key === 'Escape') setEditingLabelId(null); }}
                              autoFocus
                            />
                          ) : (
                            <span
                              style={{ fontWeight: 600, fontSize: 13, cursor: 'pointer' }}
                              onClick={() => { setEditingLabelId(cfg.id); setEditingLabelText(cfg.label); }}
                              title={t('mcp.clickToRename')}
                            >
                              {cfg.label}
                              <Pencil size={9} style={{ marginLeft: 4, color: 'rgba(255,255,255,0.2)' }} />
                            </span>
                          )}
                          {cfg.project_names.length > 0 && (
                            <span style={s.sourceBadge} title={cfg.project_names.join(', ')}>
                              {cfg.project_names.length <= 3
                                ? cfg.project_names.join(', ')
                                : `${cfg.project_names.slice(0, 3).join(', ')} +${cfg.project_names.length - 3}`}
                            </span>
                          )}
                        </div>

                        {/* Env keys */}
                        {cfg.env_keys.length > 0 && (
                          <>
                            <div style={{ display: 'flex', gap: 4, marginTop: 6, alignItems: 'center', flexWrap: 'wrap' }}>
                              <Key size={10} style={{ color: 'rgba(255,255,255,0.3)' }} />
                              {editingEnvId !== cfg.id && cfg.env_keys.map(k => (
                                <span key={k} style={{ fontSize: 11, color: 'rgba(255,255,255,0.35)', fontFamily: 'monospace' }}>{k}</span>
                              ))}
                              <button
                                style={{ ...s.iconBtn, marginLeft: 4 }}
                                onClick={() => handleStartEditSecrets(cfg.id)}
                                title={editingEnvId === cfg.id ? t('mcp.close') : t('mcp.editKeys')}
                                aria-label={editingEnvId === cfg.id ? t('mcp.close') : t('mcp.editKeys')}
                              >
                                <Pencil size={11} style={{ color: editingEnvId === cfg.id ? '#c8ff00' : 'rgba(255,255,255,0.3)' }} />
                              </button>
                            </div>
                            {/* Inline edit secrets form */}
                            {editingEnvId === cfg.id && (() => {
                              const def = mcpRegistry.find(m => m.id === cfg.server_id);
                              return (
                                <div style={{ marginTop: 8, padding: 12, background: 'rgba(255,255,255,0.02)', borderRadius: 6, border: '1px solid rgba(200,255,0,0.1)' }}>
                                  {def?.token_url && (
                                    <a
                                      href={def.token_url}
                                      target="_blank"
                                      rel="noopener noreferrer"
                                      style={{ display: 'inline-flex', alignItems: 'center', gap: 4, fontSize: 11, color: '#00d4ff', marginBottom: 8, textDecoration: 'none' }}
                                    >
                                      <ExternalLink size={10} />
                                      {def.token_help ?? t('mcp.getToken')}
                                    </a>
                                  )}
                                  {!def?.token_url && def?.token_help && (
                                    <p style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', margin: '0 0 8px' }}>{def.token_help}</p>
                                  )}
                                  {cfg.env_keys.map(k => (
                                    <div key={k} style={{ display: 'flex', gap: 8, marginBottom: 4, alignItems: 'center' }}>
                                      <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.5)', fontFamily: 'monospace', minWidth: 180 }}>{k}</span>
                                      <input
                                        style={{ ...s.input, flex: 1, fontFamily: 'monospace', fontSize: 11 }}
                                        value={editingEnv[k] ?? ''}
                                        onChange={e => setEditingEnv(prev => ({ ...prev, [k]: e.target.value }))}
                                        type={visibleFields.has(k) ? 'text' : 'password'}
                                        placeholder={t('mcp.value')}
                                      />
                                      <button
                                        style={{ ...s.iconBtn, padding: '4px 6px', flexShrink: 0 }}
                                        onClick={() => toggleFieldVisibility(k)}
                                        title={visibleFields.has(k) ? t('mcp.hide') : t('mcp.show')}
                                        aria-label={visibleFields.has(k) ? t('mcp.hide') : t('mcp.show')}
                                      >
                                        <Eye size={11} style={{ color: visibleFields.has(k) ? '#c8ff00' : 'rgba(255,255,255,0.25)' }} />
                                      </button>
                                    </div>
                                  ))}
                                  <div style={{ display: 'flex', gap: 6, marginTop: 8 }}>
                                    <button
                                      style={{ ...s.scanBtn, background: 'rgba(200,255,0,0.15)', color: '#c8ff00' }}
                                      onClick={handleSaveSecrets}
                                      disabled={editingEnvLoading}
                                    >
                                      <Save size={12} /> {editingEnvLoading ? t('mcp.saving') : t('mcp.save')}
                                    </button>
                                    <button style={s.scanBtn} onClick={() => setEditingEnvId(null)}>{t('mcp.cancel')}</button>
                                  </div>
                                </div>
                              );
                            })()}
                          </>
                        )}

                        {/* Global label + project toggles */}
                        <div style={{ display: 'flex', gap: 6, marginTop: 8, alignItems: 'center', flexWrap: 'wrap' }}>
                          <span
                            style={s.globalLabel(cfg.is_global)}
                            onClick={() => handleToggleConfigGlobal(cfg)}
                            title={cfg.is_global ? t('mcp.disableGlobal') : t('mcp.enableGlobal')}
                          >
                            Global
                          </span>
                          <span
                            style={s.generalLabel(cfg.include_general)}
                            onClick={async () => { try { await mcpsApi.updateConfig(cfg.id, { include_general: !cfg.include_general }); refetchMcps(); } catch (e) { console.warn('Failed to toggle general:', e); } }}
                            title={cfg.include_general ? t('mcp.disableGeneral') : t('mcp.enableGeneral')}
                          >
                            {t('mcp.general')}
                          </span>
                          <span style={{ color: 'rgba(255,255,255,0.12)', fontSize: 11 }}>|</span>
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
                                  <span key={proj.id} style={{ display: 'inline-flex', alignItems: 'center', gap: 0 }}>
                                    <button
                                      style={s.projectToggle(isLinked)}
                                      onClick={() => handleToggleConfigProject(cfg.id, proj.id, isLinked)}
                                    >
                                      {isLinked ? <CheckSquare size={11} style={{ color: '#c8ff00' }} /> : <Square size={11} />}
                                      {proj.name}
                                    </button>
                                    {isLinked && (() => {
                                      const slug = slugify(cfg.label);
                                      const isCustomized = mcpOverview.customized_contexts.includes(`${slug}:${proj.id}`);
                                      return (
                                        <button
                                          style={{ ...s.iconBtn, padding: '3px 5px', marginLeft: -1, borderLeft: 'none', borderTopLeftRadius: 0, borderBottomLeftRadius: 0 }}
                                          onClick={() => handleOpenContext(proj.id, proj.name, cfg.label)}
                                          title={`${t('mcp.editContext', cfg.label, proj.name)}${isCustomized ? ' ' + t('mcp.customized') : ' ' + t('mcp.default')}`}
                                          aria-label={`${t('mcp.editContext', cfg.label, proj.name)}`}
                                        >
                                          <FileText size={10} style={{ color: isCustomized ? '#c8ff00' : 'rgba(255,255,255,0.2)' }} />
                                        </button>
                                      );
                                    })()}
                                  </span>
                                );
                              })}
                              {hiddenCount > 0 && (
                                <button
                                  style={{ background: 'none', border: '1px solid rgba(200,255,0,0.15)', borderRadius: 4, color: '#c8ff00', fontSize: 10, padding: '2px 8px', cursor: 'pointer', fontFamily: 'inherit' }}
                                  onClick={() => setExpandedProjectLists(prev => { const n = new Set(prev); n.add(cfg.id); return n; })}
                                >
                                  {t('mcp.moreProjects', hiddenCount)}
                                </button>
                              )}
                              {showAll && sorted.length > PROJECT_TOGGLE_LIMIT && (
                                <button
                                  style={{ background: 'none', border: '1px solid rgba(255,255,255,0.1)', borderRadius: 4, color: 'rgba(255,255,255,0.4)', fontSize: 10, padding: '2px 8px', cursor: 'pointer', fontFamily: 'inherit' }}
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
                        style={{ ...s.iconBtn, color: '#ff4d6a' }}
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
        <div style={{ ...s.card, textAlign: 'center', padding: 40 }}>
          <Server size={32} style={{ color: 'rgba(255,255,255,0.15)', marginBottom: 12 }} />
          <p style={{ color: 'rgba(255,255,255,0.4)', fontSize: 13 }}>
            {t('mcp.empty')}
          </p>
        </div>
      ) : null}

      {/* ── MCP Context Editor Modal ── */}
      {contextEditor && (
        <div style={{
          position: 'fixed', top: 0, left: 0, right: 0, bottom: 0,
          background: 'rgba(0,0,0,0.7)', zIndex: 1000,
          display: 'flex', alignItems: 'center', justifyContent: 'center',
        }} onClick={() => setContextEditor(null)}>
          <div
            style={{
              background: '#12151c', border: '1px solid rgba(200,255,0,0.15)',
              borderRadius: 12, padding: 24, width: '90%', maxWidth: 700, maxHeight: '80vh',
              display: 'flex', flexDirection: 'column', gap: 12,
            }}
            onClick={e => e.stopPropagation()}
            role="dialog"
            aria-modal="true"
            aria-labelledby="context-editor-title"
            onKeyDown={e => { if (e.key === 'Escape') setContextEditor(null); }}
          >
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              <div>
                <h3 id="context-editor-title" style={{ margin: 0, fontSize: 15, fontWeight: 600 }}>
                  <FileText size={14} style={{ marginRight: 6, color: '#c8ff00' }} />
                  {t('mcp.contextTitle', contextEditor.slug.replace(/-/g, ' '))}
                </h3>
                <p style={{ margin: '4px 0 0', fontSize: 11, color: 'rgba(255,255,255,0.35)' }}>
                  {t('mcp.contextInfo', contextEditor.projectName, contextEditor.slug)}
                </p>
              </div>
              <button style={s.iconBtn} onClick={() => setContextEditor(null)} aria-label="Close"><X size={14} /></button>
            </div>

            <textarea
              style={{
                ...s.input, minHeight: 350, resize: 'vertical', fontFamily: 'monospace',
                fontSize: 12, lineHeight: 1.6, whiteSpace: 'pre-wrap',
              }}
              value={contextEditor.content}
              onChange={e => setContextEditor(prev => prev ? { ...prev, content: e.target.value } : null)}
              placeholder={t('mcp.contextPlaceholder')}
            />

            <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end' }}>
              <button style={s.scanBtn} onClick={() => setContextEditor(null)}>{t('mcp.cancel')}</button>
              <button
                style={{ ...s.scanBtn, background: 'rgba(200,255,0,0.15)', color: '#c8ff00' }}
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

// ── Styles ──

const s = {
  pageHeader: { display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 20 } as const,
  h1: { fontSize: 20, fontWeight: 700, letterSpacing: '-0.02em' } as const,
  meta: { color: 'rgba(255,255,255,0.4)', fontSize: 12, marginTop: 4 } as const,
  scanBtn: { padding: '7px 14px', borderRadius: 6, border: '1px solid rgba(200,255,0,0.2)', background: 'rgba(200,255,0,0.05)', color: '#c8ff00', cursor: 'pointer', fontSize: 12, fontFamily: 'inherit', fontWeight: 500, display: 'flex', alignItems: 'center', gap: 6 } as const,
  card: { background: '#12151c', border: '1px solid rgba(255,255,255,0.07)', borderRadius: 10, padding: '16px 20px' } as const,
  input: { width: '100%', padding: '8px 12px', background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.08)', borderRadius: 6, color: '#e8eaed', fontSize: 12, fontFamily: 'inherit' } as const,
  iconBtn: { background: 'none', border: '1px solid rgba(255,255,255,0.08)', borderRadius: 4, padding: '4px 8px', color: 'rgba(255,255,255,0.5)', cursor: 'pointer', display: 'flex', alignItems: 'center', fontSize: 11 } as const,
  sectionLabel: { fontSize: 11, fontWeight: 600, textTransform: 'uppercase' as const, letterSpacing: '0.05em', color: 'rgba(255,255,255,0.4)', margin: 0 },
  mcpCard: { display: 'flex', alignItems: 'flex-start', gap: 12, padding: '14px 16px', borderRadius: 8, background: 'rgba(255,255,255,0.03)', marginBottom: 8, border: '1px solid rgba(255,255,255,0.06)' } as const,
  sourceBadge: { fontSize: 10, fontWeight: 600, padding: '1px 6px', borderRadius: 4, background: 'rgba(200,255,0,0.08)', color: 'rgba(200,255,0,0.6)', border: '1px solid rgba(200,255,0,0.12)' } as const,
  globalLabel: (active: boolean) => ({ fontSize: 11, fontWeight: 700, letterSpacing: '0.04em', textTransform: 'uppercase' as const, cursor: 'pointer', userSelect: 'none' as const, padding: '4px 10px', borderRadius: 5, border: `1px solid ${active ? 'rgba(200,255,0,0.3)' : 'rgba(255,255,255,0.08)'}`, background: active ? 'rgba(200,255,0,0.12)' : 'transparent', color: active ? '#c8ff00' : 'rgba(255,255,255,0.25)', transition: 'all 0.15s' }),
  generalLabel: (active: boolean) => ({ fontSize: 11, fontWeight: 700, letterSpacing: '0.04em', textTransform: 'uppercase' as const, cursor: 'pointer', userSelect: 'none' as const, padding: '4px 10px', borderRadius: 5, border: `1px solid ${active ? 'rgba(96,165,250,0.3)' : 'rgba(255,255,255,0.08)'}`, background: active ? 'rgba(96,165,250,0.12)' : 'transparent', color: active ? '#60a5fa' : 'rgba(255,255,255,0.25)', transition: 'all 0.15s' }),
  projectToggle: (active: boolean) => ({ display: 'flex', alignItems: 'center', gap: 4, padding: '4px 10px', borderRadius: 5, fontSize: 11, fontFamily: 'inherit', cursor: 'pointer', border: `1px solid ${active ? 'rgba(200,255,0,0.2)' : 'rgba(255,255,255,0.08)'}`, background: active ? 'rgba(200,255,0,0.06)' : 'rgba(255,255,255,0.02)', color: active ? 'rgba(200,255,0,0.8)' : 'rgba(255,255,255,0.35)' } as const),
  fieldLabel: { fontSize: 11, color: 'rgba(255,255,255,0.4)', display: 'block', marginBottom: 4 } as const,
  categoryHeader: { fontSize: 10, fontWeight: 700, color: 'rgba(255,255,255,0.35)', textTransform: 'uppercase' as const, letterSpacing: 1, padding: '8px 4px 4px', borderBottom: '1px solid rgba(255,255,255,0.05)', marginBottom: 2 },
  registryItem: { display: 'flex', alignItems: 'center', gap: 8, padding: '7px 10px', borderRadius: 6, cursor: 'pointer', background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.06)' } as const,
  alreadyBadge: { fontSize: 9, color: '#00d4ff', background: 'rgba(0,212,255,0.1)', padding: '1px 5px', borderRadius: 3 } as const,
};

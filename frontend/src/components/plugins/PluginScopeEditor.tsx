import { useState } from 'react';
import { AlertTriangle, CheckSquare, Globe, Square } from 'lucide-react';
import type { HostSyncMode, Project } from '../../types/generated';
import { isHiddenPath } from '../../lib/constants';
import { HostSyncPreview } from '../HostSyncPreview';
import { hasAgentScope } from './mcpPageHelpers';

const PROJECT_TOGGLE_LIMIT = 10;

export interface PluginScopeEditorProps {
  t: (key: string, ...args: (string | number)[]) => string;
  projects: Project[];
  isGlobal: boolean;
  onToggleGlobal: () => void;
  /** Omitted while the config doesn't exist yet (the Add flow): the
   *  creation request has no `include_general` field, the backend
   *  always starts a new config at `true` — there is nothing to toggle
   *  until the fiche opens after creation. */
  includeGeneral?: boolean;
  onToggleGeneral?: () => void;
  projectIds: string[];
  onToggleProject: (projectId: string, currentlyLinked: boolean) => void;
  /** Per-project decoration (load badge + context-edit button) — only
   *  meaningful once the config already exists (the fiche). */
  renderProjectExtra?: (projectId: string, isLinked: boolean) => React.ReactNode;
  supportsHostSync: boolean;
  hostSync: HostSyncMode;
  onSetHostSync?: (mode: HostSyncMode) => void;
  hostSyncHint?: string;
  hostSyncNote?: 'hybrid';
  /** Present only once the config exists — shows the orphan-scope banner
   *  when `isGlobal`/`includeGeneral`/`projectIds` leave it invisible to
   *  every agent, with a one-click repair (enable Général). */
  onRepairOrphan?: () => void;
  testIdPrefix?: string;
}

/**
 * The ONE plugin-scope editor (KT-831): reused verbatim by the Add-MCP
 * panel, the plugin detail "fiche" and the bundle-import scope table. It
 * separates two questions that used to be tangled in three different ad
 * hoc UIs: "visible par les agents Kronn" (global / général / projets —
 * Kronn-internal routing) and "aussi dans mes CLIs locaux" (host_sync —
 * whether Kronn writes this plugin into ~/.claude.json & co, with a live
 * preview of the files that will be touched).
 */
export function PluginScopeEditor({
  t, projects, isGlobal, onToggleGlobal, includeGeneral, onToggleGeneral,
  projectIds, onToggleProject, renderProjectExtra,
  supportsHostSync, hostSync, onSetHostSync, hostSyncHint, hostSyncNote,
  onRepairOrphan, testIdPrefix = 'mcp-scope',
}: PluginScopeEditorProps) {
  const [showAllProjects, setShowAllProjects] = useState(false);

  const showOrphanWarning = !!onRepairOrphan
    && !hasAgentScope(isGlobal, includeGeneral ?? false, projectIds);

  const sorted = projects.filter(p => !isHiddenPath(p.path)).sort((a, b) => {
    const aLinked = projectIds.includes(a.id) ? 0 : 1;
    const bLinked = projectIds.includes(b.id) ? 0 : 1;
    return aLinked - bLinked || a.name.localeCompare(b.name);
  });
  const visible = showAllProjects ? sorted : sorted.slice(0, PROJECT_TOGGLE_LIMIT);
  const hiddenCount = sorted.length - visible.length;

  return (
    <div className="mcp-detail-section" data-testid={testIdPrefix}>
      <h3 className="mcp-detail-section-title">{t('mcp.scope')}</h3>
      {showOrphanWarning && (
        <div className="mcp-scope-orphan-warning" role="alert">
          <AlertTriangle size={16} aria-hidden="true" />
          <span>
            <strong>{t('mcp.scopeOrphanTitle')}</strong>
            <small>{t('mcp.scopeOrphanBody')}</small>
          </span>
          <button type="button" onClick={onRepairOrphan}>
            {t('mcp.scopeRepairGeneral')}
          </button>
        </div>
      )}
      <div
        style={{ fontSize: '0.78em', color: 'var(--kr-text-muted)', textTransform: 'uppercase', letterSpacing: '0.04em', marginBottom: 4 }}
      >
        {t('mcp.scope.agentVisibility')}
      </div>
      <div className="mcp-toggle-row">
        <button
          type="button"
          className={`mcp-project-toggle ${isGlobal ? 'mcp-project-toggle-on' : 'mcp-project-toggle-off'}`}
          onClick={onToggleGlobal}
          title={isGlobal ? t('mcp.disableGlobal') : t('mcp.enableGlobal')}
          aria-pressed={isGlobal}
          data-testid={`${testIdPrefix}-global`}
        >
          {isGlobal ? <CheckSquare size={11} className="text-accent" /> : <Square size={11} />}
          {t('mcp.globalAll')}
        </button>
        {onToggleGeneral && (
          <button
            type="button"
            className={`mcp-toggle-label mcp-toggle-general${includeGeneral ? ' mcp-toggle-general-active' : ''}`}
            onClick={onToggleGeneral}
            title={includeGeneral ? t('mcp.disableGeneral') : t('mcp.enableGeneral')}
            aria-pressed={includeGeneral}
          >
            {t('mcp.general')}
          </button>
        )}
      </div>
      <div className="mcp-toggle-row">
        {visible.map(proj => {
          const isLinked = isGlobal || projectIds.includes(proj.id);
          return (
            <span key={proj.id} className="flex-row">
              <button
                type="button"
                className={`mcp-project-toggle ${isLinked ? 'mcp-project-toggle-on' : 'mcp-project-toggle-off'}`}
                onClick={() => onToggleProject(proj.id, isLinked)}
                aria-pressed={isLinked}
                disabled={isGlobal}
              >
                {isLinked ? <CheckSquare size={11} className="text-accent" /> : <Square size={11} />}
                {proj.name}
              </button>
              {renderProjectExtra?.(proj.id, isLinked)}
            </span>
          );
        })}
        {hiddenCount > 0 && (
          <button type="button" className="mcp-more-projects-btn" onClick={() => setShowAllProjects(true)}>
            {t('mcp.moreProjects', hiddenCount)}
          </button>
        )}
        {showAllProjects && sorted.length > PROJECT_TOGGLE_LIMIT && (
          <button type="button" className="mcp-less-projects-btn" onClick={() => setShowAllProjects(false)}>
            {t('mcp.lessProjects')}
          </button>
        )}
      </div>
      <div
        className="mcp-host-sync-block"
        style={{ marginTop: 12, paddingTop: 12, borderTop: '1px dashed var(--kr-border)', position: 'relative' }}
      >
        {supportsHostSync ? (
          <>
            <label
              style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer', fontSize: '0.95em', fontWeight: 500 }}
              title={hostSyncHint}
            >
              <input
                type="checkbox"
                checked={hostSync !== 'None'}
                onChange={e => onSetHostSync?.(e.target.checked ? 'GlobalOnly' : 'None')}
              />
              <Globe size={13} />
              {t('mcp.hostSync.localCliLabel')}
            </label>
            {hostSyncNote === 'hybrid' && (
              <p className="text-muted" style={{ fontSize: '0.8em', margin: '4px 0 0 22px', fontStyle: 'italic' }}>
                {t('mcp.hostSync.hybridNote')}
              </p>
            )}
            {hostSync !== 'None' && (
              <HostSyncPreview isGlobal={isGlobal} projectIds={projectIds} projects={projects} />
            )}
          </>
        ) : (
          <p className="text-muted" style={{ fontSize: '0.85em', margin: 0 }}>
            <Globe size={11} style={{ verticalAlign: 'text-bottom', marginRight: 4 }} />
            {t('mcp.hostSync.apiOnlyNote')}
          </p>
        )}
      </div>
    </div>
  );
}

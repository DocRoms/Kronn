import { RefreshCw, SearchCheck, Star } from 'lucide-react';
import type { McpConfigDisplay, PluginInterface } from '../../types/generated';
import { accessHealth, isAvailableLocally, visibleToPluginProject } from './pluginHealth';
import { PluginHealthBadge } from './PluginHealthBadge';
import type { McpPageState } from './useMcpPageState';

const ACCESS_ORDER: PluginInterface[] = ['mcp', 'api', 'cli'];

/** Summary + lanes for the project picked in the Plugins filter panel
 *  (KT-907): the summary counts what the list shows, so the health of a
 *  project reads here rather than on a per-project tree in the sidebar. */
export function PluginProjectOverview({ state }: { state: McpPageState }) {
  const {
    t, projects, configs, matchingConfigs, selectedProjectId, setSelectedConfigId,
    probeByConfig, probeTestedAtByConfig, driftBySlug, mcpOverview, healthFor,
    testingProjectId, handleTestProject, syncing, handlePreviewRescan,
    rescanPreview, setRescanPreview, handleApplyRescan,
  } = state;
  const selectedProject = projects.find(project => project.id === selectedProjectId);
  const scopeLabel = selectedProjectId === '__all__'
    ? t('mcp.allProjects')
    : selectedProjectId === '__none__'
      ? t('disc.noProject')
      : selectedProject?.name ?? t('mcp.allProjects');
  // `matchingConfigs` already carries the Project filter (usePluginListState).
  const scopedConfigs = matchingConfigs;
  const testTargets = configs.filter(config => visibleToPluginProject(config, selectedProjectId));
  const healthContext = (config: McpConfigDisplay) => ({
    liveProbe: probeByConfig[config.id],
    liveTestedAt: probeTestedAtByConfig[config.id],
    incomplete: mcpOverview.incomplete_configs.find(item => item.config_id === config.id),
    hasEndpointDrift: (driftBySlug[config.server_id]?.length ?? 0) > 0,
  });
  const states = scopedConfigs.map(healthFor);
  const errorCount = states.filter(stateValue => stateValue === 'error').length;
  const warningCount = states.filter(stateValue => stateValue === 'warning').length;
  const mcpCount = scopedConfigs.filter(config => config.interfaces.includes('mcp')).length;
  const localCount = scopedConfigs.filter(isAvailableLocally).length;
  const isTesting = testingProjectId === selectedProjectId;

  return <section className="mcp-project-overview" data-testid="mcp-project-overview">
    <header className="mcp-project-overview-header collection-detail-header">
      <div>
        <h2>{t('mcp.projectOverviewTitle', scopeLabel)}</h2>
        <div className="mcp-project-summary" aria-label={t('mcp.projectSummary')}>
          <span><strong>{scopedConfigs.length}</strong>{t('mcp.summary.plugins')}</span>
          <span><strong>{mcpCount}</strong>{t('mcp.summary.mcp')}</span>
          <span><strong>{localCount}</strong>{t('mcp.summary.local')}</span>
          <span data-state="error"><strong>{errorCount}</strong>{t('mcp.summary.errors')}</span>
          <span data-state="warning"><strong>{warningCount}</strong>{t('mcp.summary.warnings')}</span>
        </div>
      </div>
      <div className="mcp-project-actions">
        <button
          type="button"
          className="mcp-btn-action mcp-btn-action-primary"
          disabled={isTesting || testTargets.length === 0}
          onClick={() => handleTestProject(selectedProjectId, testTargets)}
          data-testid="mcp-test-project"
        >
          <SearchCheck size={13} />
          {t(isTesting
            ? 'mcp.testingProject'
            : selectedProjectId === '__all__' ? 'mcp.testAll' : 'mcp.testProject')}
        </button>
        <button
          type="button"
          className="mcp-btn-action"
          disabled={syncing}
          onClick={handlePreviewRescan}
          data-testid="mcp-rescan-preview-button"
        >
          <RefreshCw size={13} className={syncing ? 'spin' : undefined} />
          {t('mcp.rescan')}
        </button>
      </div>
    </header>

    {rescanPreview && <div className="mcp-rescan-preview" role="region" aria-label={t('mcp.rescanPreviewTitle')}>
      <strong>{t('mcp.rescanPreviewTitle')}</strong>
      <ul>
        <li>{t('mcp.rescanCreated', rescanPreview.configs_created)}</li>
        <li>{t('mcp.rescanMerged', rescanPreview.configs_merged)}</li>
        <li>{t('mcp.rescanDeleted', rescanPreview.configs_deleted)}</li>
      </ul>
      <div className="mcp-rescan-preview-actions">
        <button type="button" className="mcp-btn-action mcp-btn-action-primary" disabled={syncing} onClick={handleApplyRescan}>{t('mcp.rescanApply')}</button>
        <button type="button" className="mcp-btn-action" disabled={syncing} onClick={() => setRescanPreview(null)}>{t('mcp.cancel')}</button>
      </div>
    </div>}

    <div className="mcp-access-lanes">
      {ACCESS_ORDER.map(access => {
        const laneConfigs = scopedConfigs.filter(config => config.interfaces.includes(access));
        return <section className="mcp-access-lane" key={access} data-access={access}>
          <header>
            <span className="mcp-access-label">{t(`mcp.interface.${access}`)}</span>
            <div>
              <h3>{t(`mcp.projectLane.${access}.title`)}</h3>
              <p>{t(`mcp.projectLane.${access}.hint`)}</p>
            </div>
          </header>
          <div className="mcp-access-lane-list">
            {laneConfigs.map(config => {
              const health = accessHealth(config, access, healthContext(config));
              return <button
                type="button"
                className="mcp-access-card"
                data-state={health.state}
                key={config.id}
                onClick={() => setSelectedConfigId(config.id)}
              >
                <span className="mcp-access-card-heading">
                  <strong>{config.label}</strong>
                  {config.effective_preferred_interface === access && config.interfaces.length > 1 && (
                    <span className="mcp-used-access"><Star size={10} />{t('mcp.usedByAgent')}</span>
                  )}
                </span>
                <PluginHealthBadge health={health} t={t} />
              </button>;
            })}
            {laneConfigs.length === 0 && <p className="mcp-access-lane-empty">{t('mcp.projectLane.empty')}</p>}
          </div>
        </section>;
      })}
    </div>
  </section>;
}

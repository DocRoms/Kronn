import { X, FileText } from 'lucide-react';
import { RecoveryRestorePanel } from '../components/RecoveryRestorePanel';
import { PluginPortabilityModal } from '../components/PluginPortabilityModal';
import { AddPluginModal } from '../components/plugins/AddPluginModal';
import { PluginCollectionView } from '../components/plugins/PluginCollectionView';
import { useMcpPageState, type McpPageProps } from '../components/plugins/useMcpPageState';
import './McpPage.css';

export function McpPage(props: McpPageProps) {
  const state = useMcpPageState(props);
  const {
    t, toast, ToastContainer,
    portabilityMode, setPortabilityMode, configs, projects, refetchMcps,
    mcpOverview, mcpRegistry, setSelectedConfigId,
    contextEditor, setContextEditor, contextSaving, handleSaveContext,
  } = state;

  return (
    <div className="mcp-page">
      <ToastContainer />
      {portabilityMode && (
        <PluginPortabilityModal
          mode={portabilityMode}
          configs={configs}
          registry={mcpRegistry}
          projects={projects}
          onClose={() => setPortabilityMode(null)}
          onImported={() => {
            refetchMcps();
          }}
        />
      )}

      {/* Incomplete-config warning banner. Lists the MCPs whose env_keys
          are declared but values are missing/empty — those would fail
          handshake at agent boot and slow down every Kronn-spawned run.
          The scanner already SKIPS them in project-level config files;
          this banner tells the operator which plugins to fix. Click on
          a row to jump to the config detail. */}
      {(mcpOverview.incomplete_configs ?? []).length > 0 && (
        <div className="mcp-warning-banner" data-testid="mcp-incomplete-banner">
          <div className="mcp-warning-banner-title">
            ⚠ {t('mcp.incomplete.title', (mcpOverview.incomplete_configs ?? []).length)}
          </div>
          <p className="mcp-warning-banner-hint">{t('mcp.incomplete.hint')}</p>
          <ul className="mcp-warning-banner-list">
            {(mcpOverview.incomplete_configs ?? []).map(ic => (
              <li key={ic.config_id}>
                <button
                  type="button"
                  className="mcp-warning-banner-item"
                  onClick={() => setSelectedConfigId(ic.config_id)}
                >
                  <strong>{ic.label}</strong>
                  <span className="mcp-warning-banner-server"> · {ic.server_name}</span>
                  <span className="mcp-warning-banner-reason"> — {ic.reason}</span>
                  {ic.missing_keys.length > 0 && (
                    <code className="mcp-warning-banner-keys">{ic.missing_keys.join(', ')}</code>
                  )}
                </button>
              </li>
            ))}
          </ul>
          {/* P2 — when the cause is a changed encryption key (secrets
              unreadable), the recovery passphrase can restore the original
              key in one step instead of re-entering every token. */}
          <RecoveryRestorePanel toast={toast} t={t} onRestored={refetchMcps} />
        </div>
      )}

      <AddPluginModal state={state} />

      {/* ── Installed plugins grid (detail expands inline) ── */}
      <PluginCollectionView state={state} />

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
              <button className="mcp-icon-btn" onClick={() => setContextEditor(null)} aria-label={t('common.close')}><X size={14} /></button>
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

import { Upload, X, Download, FileText } from 'lucide-react';
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
    exportPayload, exportCopyState, exportTextareaRef,
    closeExportModal, handleExportRetryCopy, handleExportDownloadFile,
    portabilityMode, setPortabilityMode, configs, projects, refetchMcps,
    mcpOverview, setSelectedConfigId,
    contextEditor, setContextEditor, contextSaving, handleSaveContext,
  } = state;

  return (
    <div className="mcp-page">
      <ToastContainer />
      {/* 0.8.6 (#33 fix 2026-05-21) — Custom plugin export modal.
          Renders unconditionally at the top so it survives navigation
          inside McpPage (detail panel state can flip while the modal
          is open). Surfaces the JSON in a readonly textarea + best-
          effort clipboard write, with a copy-retry button when the
          auto-copy failed (Tauri / sandboxed webview case). */}
      {exportPayload && (
        <div
          className="mcp-export-modal-backdrop"
          data-testid="mcp-export-modal"
          onClick={closeExportModal}
        >
          <div
            className="mcp-export-modal"
            onClick={e => e.stopPropagation()}
          >
            <div className="mcp-export-modal-header">
              <span>
                <Upload size={13} style={{ marginRight: 6 }} />
                {t('mcp.custom.exportTitle', exportPayload.name)}
              </span>
              <button
                className="mcp-icon-btn"
                onClick={closeExportModal}
                aria-label={t('common.close')}
                data-testid="mcp-export-modal-close"
              >
                <X size={14} />
              </button>
            </div>
            <p className="mcp-export-modal-hint">
              {exportCopyState === 'copied'
                ? t('mcp.custom.copied')
                : exportCopyState === 'failed'
                  ? t('mcp.custom.copyManualInstruction')
                  : t('mcp.custom.exportHint')}
            </p>
            <textarea
              ref={exportTextareaRef}
              className="input mcp-input-mono"
              data-testid="mcp-export-modal-textarea"
              value={exportPayload.json}
              readOnly
              rows={14}
              autoFocus
              onFocus={e => e.currentTarget.select()}
            />
            <div className="flex-row gap-3 mt-3">
              <button
                type="button"
                className="mcp-btn-action mcp-btn-action-primary"
                onClick={handleExportRetryCopy}
                data-testid="mcp-export-modal-copy"
              >
                <Upload size={12} /> {t('mcp.custom.copyAsJson')}
              </button>
              {/* 0.8.6 (#63) — Path B file download. Blob the JSON
                  locally and trigger a download — no server round-trip,
                  no Auth headers to plumb. Works inside Tauri too. */}
              <button
                type="button"
                className="mcp-btn-action"
                onClick={() => handleExportDownloadFile(exportPayload.name, exportPayload.json)}
                data-testid="mcp-export-modal-download"
              >
                <Download size={12} /> {t('mcp.custom.downloadAsFile')}
              </button>
              <button
                type="button"
                className="mcp-btn-action"
                onClick={closeExportModal}
              >
                {t('mcp.back')}
              </button>
            </div>
          </div>
        </div>
      )}

      {portabilityMode && (
        <PluginPortabilityModal
          mode={portabilityMode}
          configs={configs}
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

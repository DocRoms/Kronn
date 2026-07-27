import { useMemo, useState } from 'react';
import { Download, KeyRound, LockKeyhole, Upload, X } from 'lucide-react';
import { mcps as mcpsApi } from '../lib/api';
import { triggerDownload } from '../lib/downloadBlob';
import { useT } from '../lib/I18nContext';
import { useToast } from '../hooks/useToast';
import { userError } from '../lib/userError';
import type {
  ImportPluginBundleReport,
  McpConfigDisplay,
  PluginBundlePreview,
} from '../types/generated';

interface PluginPortabilityModalProps {
  mode: 'export' | 'import';
  configs: McpConfigDisplay[];
  onClose: () => void;
  onImported: () => void;
}

interface BundleHeader {
  kind?: string;
  encrypted?: boolean;
  includes_values?: boolean;
  plugin_labels?: string[];
}

export function PluginPortabilityModal({
  mode,
  configs,
  onClose,
  onImported,
}: PluginPortabilityModalProps) {
  const { t } = useT();
  const { toast } = useToast();
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [preview, setPreview] = useState<PluginBundlePreview | null>(null);
  const [includeValues, setIncludeValues] = useState(false);
  const [confirmation, setConfirmation] = useState('');
  const [passphrase, setPassphrase] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [importContent, setImportContent] = useState('');
  const [importHeader, setImportHeader] = useState<BundleHeader | null>(null);
  const [importFilename, setImportFilename] = useState('');
  const [report, setReport] = useState<ImportPluginBundleReport | null>(null);

  const orderedConfigs = useMemo(
    () => [...configs].sort((left, right) => left.label.localeCompare(right.label)),
    [configs],
  );
  const selectionChanged = (configId: string, checked: boolean) => {
    setSelected(previous => {
      const next = new Set(previous);
      if (checked) next.add(configId);
      else next.delete(configId);
      return next;
    });
    setPreview(null);
    setIncludeValues(false);
    setConfirmation('');
    setPassphrase('');
    setError(null);
  };

  const loadPreview = async () => {
    if (selected.size === 0 || busy) return;
    setBusy(true);
    setError(null);
    try {
      setPreview(await mcpsApi.previewBundle({ config_ids: [...selected] }));
    } catch (caught) {
      setError(userError(caught));
    } finally {
      setBusy(false);
    }
  };

  const runExport = async () => {
    if (!preview || busy) return;
    setBusy(true);
    setError(null);
    try {
      const result = await mcpsApi.exportBundle({
        config_ids: [...selected],
        include_values: includeValues,
        passphrase: includeValues ? passphrase : null,
        confirmation: includeValues ? confirmation : null,
      });
      triggerDownload(result.filename, result.blob);
      toast(t('mcp.portability.exportDone', selected.size), 'success');
      onClose();
    } catch (caught) {
      setError(userError(caught));
    } finally {
      setBusy(false);
    }
  };

  const readImportFile = async (file: File) => {
    setError(null);
    setReport(null);
    try {
      const content = await file.text();
      const parsed = JSON.parse(content) as BundleHeader;
      if (parsed.kind !== 'kronn.plugins') {
        throw new Error(t('mcp.portability.invalidBundle'));
      }
      setImportContent(content);
      setImportHeader(parsed);
      setImportFilename(file.name);
      setPassphrase('');
    } catch (caught) {
      setImportContent('');
      setImportHeader(null);
      setImportFilename('');
      setError(userError(caught));
    }
  };

  const runImport = async () => {
    if (!importContent || busy) return;
    setBusy(true);
    setError(null);
    try {
      const nextReport = await mcpsApi.importBundle({
        content: importContent,
        passphrase: importHeader?.encrypted ? passphrase : null,
      });
      setReport(nextReport);
      onImported();
      toast(
        nextReport.already_imported
          ? t('mcp.portability.importAlready')
          : t('mcp.portability.importDone', nextReport.imported_config_ids.length),
        nextReport.conflicts.length > 0 ? 'warning' : 'success',
      );
    } catch (caught) {
      setError(userError(caught));
    } finally {
      setBusy(false);
    }
  };

  const exportDisabled = !preview
    || busy
    || (includeValues && (
      confirmation !== preview.confirmation_phrase
      || passphrase.length < preview.minimum_passphrase_length
    ));
  const labels = importHeader?.plugin_labels ?? [];

  return (
    <div
      className="mcp-export-modal-backdrop"
      role="presentation"
      onMouseDown={event => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <section
        className="mcp-export-modal mcp-portability-modal"
        role="dialog"
        aria-modal="true"
        aria-label={t(mode === 'export'
          ? 'mcp.portability.exportTitle'
          : 'mcp.portability.importTitle')}
      >
        <header className="mcp-export-modal-header">
          <span>
            {mode === 'export'
              ? <><Download size={15} /> {t('mcp.portability.exportTitle')}</>
              : <><Upload size={15} /> {t('mcp.portability.importTitle')}</>}
          </span>
          <button
            type="button"
            className="mcp-icon-btn"
            onClick={onClose}
            aria-label={t('common.close')}
          >
            <X size={14} />
          </button>
        </header>

        {mode === 'export' ? (
          <>
            <p className="mcp-export-modal-hint">{t('mcp.portability.exportHint')}</p>
            <div className="mcp-portability-select-actions">
              <button
                type="button"
                className="mcp-btn-action"
                onClick={() => {
                  setSelected(new Set(orderedConfigs.map(config => config.id)));
                  setPreview(null);
                }}
              >
                {t('mcp.portability.selectAll')}
              </button>
              <span>{t('mcp.portability.selectedCount', selected.size)}</span>
            </div>
            <div className="mcp-portability-plugin-list">
              {orderedConfigs.map(config => (
                <label key={config.id} className="mcp-portability-plugin-row">
                  <input
                    type="checkbox"
                    checked={selected.has(config.id)}
                    onChange={event => selectionChanged(config.id, event.target.checked)}
                  />
                  <span>
                    <strong>{config.label}</strong>
                    <small>{config.server_name}</small>
                  </span>
                </label>
              ))}
            </div>
            {!preview && (
              <button
                type="button"
                className="mcp-btn-action mcp-btn-action-primary"
                disabled={selected.size === 0 || busy}
                onClick={loadPreview}
              >
                {busy ? t('common.loading') : t('mcp.portability.review')}
              </button>
            )}
            {preview && (
              <>
                <div className="mcp-portability-safe-note">
                  <KeyRound size={15} />
                  <span>{t('mcp.portability.configOnly')}</span>
                </div>
                <div className="mcp-portability-danger">
                  <label>
                    <input
                      type="checkbox"
                      checked={includeValues}
                      onChange={event => {
                        setIncludeValues(event.target.checked);
                        setConfirmation('');
                        setPassphrase('');
                      }}
                    />
                    <strong>{t('mcp.portability.includeValues')}</strong>
                  </label>
                  <p>{t('mcp.portability.dangerHint')}</p>
                  {includeValues && (
                    <>
                      <div className="mcp-portability-value-list">
                        {preview.plugins.map(plugin => (
                          <div key={plugin.config_id}>
                            <strong>{plugin.label}</strong>
                            {plugin.values.length === 0 && (
                              <span>{t('mcp.portability.noValues')}</span>
                            )}
                            {plugin.values.map(value => (
                              <span
                                key={value.key}
                                data-sensitive={value.sensitive}
                                data-excluded={!value.exportable}
                              >
                                {value.key} · {value.exportable
                                  ? value.sensitive
                                    ? t('mcp.portability.sensitive')
                                    : t('mcp.portability.parameter')
                                  : t('mcp.portability.cliExcluded')}
                              </span>
                            ))}
                          </div>
                        ))}
                      </div>
                      <label className="mcp-field-label">
                        {t('mcp.portability.typeConfirmation', preview.confirmation_phrase)}
                        <input
                          className="input"
                          value={confirmation}
                          onChange={event => setConfirmation(event.target.value)}
                          autoComplete="off"
                        />
                      </label>
                      <label className="mcp-field-label">
                        {t('mcp.portability.passphrase', preview.minimum_passphrase_length)}
                        <input
                          className="input"
                          type="password"
                          value={passphrase}
                          onChange={event => setPassphrase(event.target.value)}
                          autoComplete="new-password"
                        />
                      </label>
                      <div className="mcp-portability-encrypted">
                        <LockKeyhole size={14} />
                        {t('mcp.portability.encrypted')}
                      </div>
                    </>
                  )}
                </div>
                <button
                  type="button"
                  className="mcp-btn-action mcp-btn-action-primary"
                  disabled={exportDisabled}
                  onClick={runExport}
                >
                  <Download size={13} />
                  {busy ? t('common.loading') : t('mcp.portability.download')}
                </button>
              </>
            )}
          </>
        ) : (
          <>
            <p className="mcp-export-modal-hint">{t('mcp.portability.importHint')}</p>
            <label className="mcp-portability-file">
              <Upload size={16} />
              <span>{importFilename || t('mcp.portability.chooseFile')}</span>
              <input
                type="file"
                accept=".json,.kronn-plugins.json,application/json"
                onChange={event => {
                  const file = event.target.files?.[0];
                  if (file) void readImportFile(file);
                }}
              />
            </label>
            {importHeader && (
              <div className="mcp-portability-import-summary">
                <strong>{t('mcp.portability.bundleContents', labels.length)}</strong>
                {labels.map(label => <span key={label}>{label}</span>)}
                {importHeader.encrypted && (
                  <label className="mcp-field-label">
                    <LockKeyhole size={13} /> {t('mcp.portability.importPassphrase')}
                    <input
                      className="input"
                      type="password"
                      value={passphrase}
                      onChange={event => setPassphrase(event.target.value)}
                      autoComplete="current-password"
                    />
                  </label>
                )}
              </div>
            )}
            <button
              type="button"
              className="mcp-btn-action mcp-btn-action-primary"
              disabled={!importContent || busy || (!!importHeader?.encrypted && !passphrase)}
              onClick={runImport}
            >
              <Upload size={13} />
              {busy ? t('common.loading') : t('mcp.portability.importAction')}
            </button>
            {report && (
              <div className="mcp-portability-report">
                <strong>{t('mcp.portability.report')}</strong>
                <span>{t('mcp.portability.importedCount', report.imported_config_ids.length)}</span>
                {report.warnings.map((warning, index) => (
                  <span key={`warning-${index}`}>⚠ {warning}</span>
                ))}
                {report.conflicts.map((conflict, index) => (
                  <span key={`conflict-${index}`} className="text-danger">✕ {conflict}</span>
                ))}
              </div>
            )}
          </>
        )}

        {error && <div className="mcp-form-error">{error}</div>}
      </section>
    </div>
  );
}

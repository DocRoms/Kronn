import { useMemo, useRef, useState } from 'react';
import { Check, Download, KeyRound, LockKeyhole, Upload, X } from 'lucide-react';
import { mcps as mcpsApi } from '../lib/api';
import { triggerDownload } from '../lib/downloadBlob';
import { useT } from '../lib/I18nContext';
import { useToast } from '../hooks/useToast';
import { userError } from '../lib/userError';
import { pluginKind } from '../lib/pluginKind';
import { PluginScopeEditor } from './plugins/PluginScopeEditor';
import { hasAgentScope } from './plugins/mcpPageHelpers';
import type {
  HostSyncMode,
  ImportedPluginConfig,
  ImportPluginBundleReport,
  McpConfigDisplay,
  McpDefinition,
  PluginBundlePreview,
  ImportBundlePreview,
  Project,
} from '../types/generated';

interface PluginPortabilityModalProps {
  mode: 'export' | 'import';
  configs: McpConfigDisplay[];
  registry: McpDefinition[];
  projects: Project[];
  onClose: () => void;
  onImported: () => void;
}

interface BundleHeader {
  name?: string;
  base_url?: string;
  kind?: string;
  encrypted?: boolean;
  includes_values?: boolean;
  plugin_labels?: string[];
}

export function PluginPortabilityModal({
  mode,
  configs,
  registry,
  projects,
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
  const [importPreview, setImportPreview] = useState<ImportBundlePreview | null>(null);
  const [argsConsent, setArgsConsent] = useState<Set<string>>(() => new Set());
  const [report, setReport] = useState<ImportPluginBundleReport | null>(null);
  type ImportScope = {
    global: boolean;
    includeGeneral: boolean;
    projectIds: string[];
    hostSync: HostSyncMode;
  };
  const [importScopes, setImportScopes] = useState<Record<string, ImportScope>>({});
  const closingRef = useRef(false);

  // Keep the displayed defaults deterministic while the parent refreshes its
  // overview: the scope shown to the operator must not change underneath
  // them. All projects is the documented import default; General and local-CLI
  // sync remain explicit opt-ins on this machine.
  const scopeFor = (item: ImportedPluginConfig): ImportScope => {
    const edited = importScopes[item.config_id];
    if (edited) return edited;
    return { global: true, includeGeneral: false, projectIds: [], hostSync: 'None' };
  };

  const definitionFor = (item: ImportedPluginConfig) => registry.find(definition => definition.id === item.server_id);
  const supportsHostSync = (item: ImportedPluginConfig) => {
    const definition = definitionFor(item);
    return !!definition && pluginKind(definition) !== 'api';
  };

  const applyScopes = async (scopes: Record<string, ImportScope>) => {
    if (!report) return;
    for (const item of report.imported_configs) {
      const scope = scopes[item.config_id] ?? scopeFor(item);
      await mcpsApi.updateConfig(item.config_id, {
        is_global: scope.global,
        include_general: scope.includeGeneral,
        host_sync: supportsHostSync(item) ? scope.hostSync : 'None',
      });
      await mcpsApi.setConfigProjects(item.config_id, {
        project_ids: scope.projectIds,
      });
    }
  };

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
      // A single-plugin JSON from the old per-plugin export is converted by the
      // server into a one-plugin bundle; anything else must be a bundle.
      const legacy = parsed.kind === undefined
        && typeof parsed.name === 'string' && typeof parsed.base_url === 'string';
      if (parsed.kind !== 'kronn.plugins' && !legacy) {
        throw new Error(t('mcp.portability.invalidBundle'));
      }
      setImportContent(content);
      setImportPreview(null);
      setArgsConsent(new Set());
      const header = legacy ? { ...parsed, plugin_labels: [parsed.name as string] } : parsed;
      setImportHeader(header);
      setImportFilename(file.name);
      setPassphrase('');
      if (!header.encrypted && !header.includes_values) void reviewImport(content, header);
    } catch (caught) {
      setImportContent('');
      setImportHeader(null);
      setImportPreview(null);
      setImportFilename('');
      setError(userError(caught));
    }
  };

  const reviewImport = async (content = importContent, header = importHeader) => {
    if (!content || busy) return;
    setBusy(true);
    setError(null);
    try {
      setImportPreview(await mcpsApi.previewImportBundle({
        content,
        passphrase: header?.encrypted ? passphrase : null,
        accept_args_for: [],
      }));
    } catch (caught) {
      setImportPreview(null);
      setError(userError(caught));
    } finally {
      setBusy(false);
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
        accept_args_for: [...argsConsent],
      });
      setReport(nextReport);
      setImportScopes(Object.fromEntries(nextReport.imported_configs.map(item => [
        item.config_id,
        { global: true, includeGeneral: false, projectIds: [], hostSync: 'None' },
      ])));
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

  const saveImportScopes = async () => {
    if (!report || busy) return;
    const missingScope = report.imported_configs.some(item => {
      const scope = scopeFor(item);
      return !hasAgentScope(scope.global, scope.includeGeneral, scope.projectIds);
    });
    if (missingScope) {
      setError(t('mcp.portability.scopeRequired'));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await applyScopes(importScopes);
      onImported();
      toast(t('mcp.portability.scopeSaved'), 'success');
      onClose();
    } catch (caught) {
      setError(userError(caught));
    } finally {
      setBusy(false);
    }
  };

  // KT-831 (reliquat KT-352) — a bundle import closed (X / backdrop) before
  // "Appliquer la portée" used to leave every imported config exactly as
  // the backend created it: unscoped, non-global, no projects. Any item
  // whose inherited `includeGeneral` is also false becomes invisible to
  // every agent with no further signal. Closing now applies whatever was
  // on screen (all projects pre-checked by default, matching what the operator
  // saw) instead of silently discarding it — not a hidden broadening:
  // it's exactly the state already displayed.
  const handleClose = async () => {
    if (busy || closingRef.current) return;
    if (report && report.imported_configs.length > 0) {
      const missingScope = report.imported_configs.some(item => {
        const scope = scopeFor(item);
        return !hasAgentScope(scope.global, scope.includeGeneral, scope.projectIds);
      });
      if (missingScope) {
        setError(t('mcp.portability.scopeRequired'));
        return;
      }
      closingRef.current = true;
      setBusy(true);
      setError(null);
      try {
        await applyScopes(Object.fromEntries(
          report.imported_configs.map(item => [item.config_id, scopeFor(item)]),
        ));
        onImported();
      } catch (caught) {
        console.warn('Failed to apply the default scope on close:', caught);
        setError(userError(caught));
        return;
      } finally {
        setBusy(false);
        closingRef.current = false;
      }
    }
    onClose();
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
        if (event.target === event.currentTarget) void handleClose();
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
            onClick={() => void handleClose()}
            disabled={busy}
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
                      onChange={event => {
                        setPassphrase(event.target.value);
                        setImportPreview(null);
                      }}
                      autoComplete="current-password"
                    />
                  </label>
                )}
                {importHeader.encrypted && !importPreview && (
                  <button
                    type="button"
                    className="mcp-btn-action"
                    disabled={busy || !passphrase}
                    onClick={() => void reviewImport()}
                  >
                    {t('mcp.portability.reviewBundle')}
                  </button>
                )}
              </div>
            )}
            {importPreview && (
              <div className="mcp-portability-import-review" data-testid="mcp-import-review">
                {importPreview.legacy && <p className="mcp-form-hint">{t('mcp.portability.legacyNotice')}</p>}
                {importPreview.already_imported && <p className="mcp-form-hint">{t('mcp.portability.alreadyImported')}</p>}
                {importPreview.plugins.map(plugin => (
                  <div key={plugin.source_config_id} className="mcp-portability-import-plugin">
                    <strong>{plugin.label}</strong>
                    <small>{plugin.server_name}</small>
                    {plugin.issue && (
                      <span className="mcp-form-hint" role="note">{t(`mcp.portability.issue.${plugin.issue}`)}</span>
                    )}
                    {plugin.args_differ && plugin.proposed_args && (
                      <div className="mcp-portability-args-diff">
                        <p className="mcp-form-hint">{t('mcp.portability.argsWarning')}</p>
                        <div>
                          <span>{t('mcp.portability.argsUsual')}</span>
                          <code>{(plugin.usual_args ?? []).join(' ')}</code>
                        </div>
                        <div>
                          <span>{t('mcp.portability.argsProposed')}</span>
                          <code>{plugin.proposed_args.join(' ')}</code>
                        </div>
                        <label>
                          <input
                            type="checkbox"
                            checked={argsConsent.has(plugin.source_config_id)}
                            aria-label={t('mcp.portability.argsAcceptFor', plugin.label)}
                            onChange={event => setArgsConsent(previous => {
                              const next = new Set(previous);
                              if (event.target.checked) next.add(plugin.source_config_id);
                              else next.delete(plugin.source_config_id);
                              return next;
                            })}
                          />
                          {t('mcp.portability.argsAccept')}
                        </label>
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )}
            <button
              type="button"
              className="mcp-btn-action mcp-btn-action-primary"
              disabled={!importContent || !importPreview || busy || (!!importHeader?.encrypted && !passphrase)}
              onClick={runImport}
            >
              <Upload size={13} />
              {busy ? t('common.loading') : t('mcp.portability.importAction')}
            </button>
            {report && (
              <div className="mcp-portability-report">
                <strong>{t('mcp.portability.report')}</strong>
                <span>{t('mcp.portability.importedCount', report.imported_config_ids.length)}</span>
                {report.imported_configs.length > 0 && (
                  <div className="mcp-portability-scope-editor">
                    <div className="mcp-portability-scope-heading">
                      <strong>{t('mcp.portability.scopeTitle')}</strong>
                      <span>{t('mcp.portability.scopeHint')}</span>
                    </div>
                    <div className="mcp-portability-scope-table" role="table">
                      <div className="mcp-portability-scope-header" role="row">
                        <span role="columnheader">{t('mcp.portability.pluginColumn')}</span>
                        <span role="columnheader">{t('mcp.portability.projectsColumn')}</span>
                      </div>
                      {report.imported_configs.map(item => {
                        const scope = scopeFor(item);
                        const definition = definitionFor(item);
                        const setScope = (next: ImportScope) => setImportScopes(previous => ({
                          ...previous,
                          [item.config_id]: next,
                        }));
                        return (
                          <div className="mcp-portability-scope-row" role="row" key={item.config_id}>
                            <span role="cell" className="mcp-portability-scope-plugin">
                              <strong>{item.label}</strong>
                              <small>{item.server_name}</small>
                            </span>
                            <span role="cell" className="mcp-portability-scope-choices">
                              <PluginScopeEditor
                                t={t}
                                projects={projects}
                                isGlobal={scope.global}
                                onToggleGlobal={() => setScope({
                                  ...scope,
                                  global: !scope.global,
                                })}
                                includeGeneral={scope.includeGeneral}
                                onToggleGeneral={() => setScope({ ...scope, includeGeneral: !scope.includeGeneral })}
                                projectIds={scope.projectIds}
                                onToggleProject={(projectId, isLinked) => setScope({
                                  ...scope,
                                  projectIds: isLinked
                                    ? scope.projectIds.filter(id => id !== projectId)
                                    : [...scope.projectIds, projectId],
                                })}
                                supportsHostSync={!!definition && pluginKind(definition) !== 'api'}
                                hostSync={scope.hostSync}
                                onSetHostSync={(hostSync) => setScope({ ...scope, hostSync })}
                                hostSyncNote={definition && pluginKind(definition) === 'hybrid' ? 'hybrid' : undefined}
                                testIdPrefix={`mcp-import-scope-${item.config_id}`}
                              />
                            </span>
                          </div>
                        );
                      })}
                    </div>
                    <button
                      type="button"
                      className="mcp-btn-action mcp-btn-action-primary"
                      disabled={busy}
                      onClick={saveImportScopes}
                    >
                      <Check size={13} />
                      {busy ? t('common.loading') : t('mcp.portability.applyScope')}
                    </button>
                  </div>
                )}
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

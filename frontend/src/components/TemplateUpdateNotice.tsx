import { useRef, useState } from 'react';
import { projects } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { errorText } from '../lib/templateMerge';
import type { PlaceholderDoc, TemplateUpdate } from '../types/generated';
import { PlaceholderDocsBadge } from './TemplateReinstall';
import './TemplateMergePanel.css';

export function TemplateUpdateBadge({ update }: { update?: TemplateUpdate | null }) {
  const { t } = useT();
  if (!update?.available) return null;
  const paths = [...update.missing_files.map(f => f.path), ...update.renames.map(r => `${r.from} → ${r.to}`)];
  return <span className="dash-badge-drift" title={paths.join('\n')}>{t('templateUpdate.available')}</span>;
}

export function TemplateUpdateNotice({ projectId, update, onRunSteps, onChanged }: {
  projectId: string;
  update?: TemplateUpdate | null;
  onRunSteps: (steps: number[]) => void;
  onChanged?: () => void;
}) {
  const { t } = useT();
  const busy = useRef(false);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [filled, setFilled] = useState<PlaceholderDoc[] | null>(null);
  const [installed, setInstalled] = useState(false);
  if (!update || (!update.available && filled === null)) return null;
  const install = async () => {
    if (busy.current) return;
    busy.current = true;
    setRunning(true);
    setError(null);
    try {
      await projects.installTemplate(projectId);
      const drift = await projects.checkDrift(projectId);
      const paths = new Set(update.missing_files.map(f => f.path));
      // Reuse the live KT-1203 step mapping, restricted to the files this update added.
      setFilled(drift.placeholder_docs.filter(d => paths.has(d.path)));
      setInstalled(true);
      onChanged?.();
    } catch (e) {
      setError(errorText(e));
    } finally {
      busy.current = false;
      setRunning(false);
    }
  };
  return (
    <section className="tm-panel" data-testid="template-update-notice" aria-label={t('templateUpdate.available')}>
      <strong>{t(installed ? 'templateUpdate.installed' : 'templateUpdate.available')}</strong>
      <p className="tm-small">{t('templateUpdate.versions', update.installed_version ?? t('templateUpdate.unknown'), update.current_version ?? t('templateUpdate.unknown'))}</p>
      {update.missing_files.length > 0 && <ul className="tm-path-list">{update.missing_files.map(file => <li key={file.path}><code>{file.path}</code></li>)}</ul>}
      {update.renames.length > 0 && <ul className="tm-path-list">{update.renames.map(rename => (
        <li key={rename.from}><code>{rename.from}</code> → <code>{rename.to}</code>{rename.conflict && <span data-testid="template-update-conflict"> · {t('templateUpdate.conflict')}</span>}</li>
      ))}</ul>}
      {update.unresolved_files.length > 0 && <p className="tm-small">{t('templateUpdate.unresolved')} {update.unresolved_files.map(path => <code key={path}>{path}</code>)}</p>}
      {update.spec_modified && <p className="tm-small" data-testid="template-update-spec-modified">{t('templateUpdate.specModified')}</p>}
      {error && <p role="alert">{error}</p>}
      {!installed && <button type="button" className="tm-btn" data-testid="template-update-install" disabled={running} onClick={() => { void install(); }}>{t(running ? 'templateUpdate.installing' : 'templateUpdate.install')}</button>}
      {filled !== null && <PlaceholderDocsBadge docs={filled} onRun={onRunSteps} />}
      {filled?.some(f => f.step === null) && <div data-testid="template-update-unowned">
        <p className="tm-small">{t('templateUpdate.unowned')}</p>
        <ul className="tm-path-list">{filled.filter(f => f.step === null).map(f => <li key={f.path}><code>{f.path}</code></li>)}</ul>
      </div>}
    </section>
  );
}

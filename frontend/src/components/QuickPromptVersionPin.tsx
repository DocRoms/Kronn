/**
 * The Quick Prompt version an Agent step loads: its latest one, or a pinned
 * one, so later edits of the Quick Prompt reach the step only when chosen.
 */
import { useEffect, useState } from 'react';
import { useT } from '../lib/I18nContext';
import { quickPrompts as quickPromptsApi } from '../lib/api';
import type { QuickPromptVersion } from '../types/generated';
import './SkillVersionPins.css';

interface Props {
  qpId: string;
  qpName: string;
  value: number | null | undefined;
  onChange: (version: number | null) => void;
}

export default function QuickPromptVersionPin({ qpId, qpName, value, onChange }: Props) {
  const { t } = useT();
  const [loaded, setLoaded] = useState<{ id: string; versions: QuickPromptVersion[]; failed: boolean } | null>(null);
  useEffect(() => {
    let cancelled = false;
    quickPromptsApi.history(qpId)
      .then(list => { if (!cancelled) setLoaded({ id: qpId, versions: list ?? [], failed: false }); })
      .catch(() => { if (!cancelled) setLoaded({ id: qpId, versions: [], failed: true }); });
    return () => { cancelled = true; };
  }, [qpId]);
  const versions = loaded?.id === qpId ? loaded.versions : null;
  const pinned = value ?? null;
  const latest = versions?.reduce((max, v) => Math.max(max, v.version_index), 0) || null;
  const pinnedVersion = pinned != null ? versions?.find(v => v.version_index === pinned) : undefined;
  const behind = pinned != null && latest != null && latest > pinned;
  return (
    <div className="skill-version-pins" data-testid="qp-version-pin">
      <small>{t('wiz.agentQpVersionHint')}</small>
      <div className="skill-version-pin">
        <label>
          <span className="skill-version-pin-name">{t('wiz.agentQpVersion')}</span>
          <select
            value={pinned ?? ''}
            disabled={versions === null}
            onChange={e => onChange(e.target.value ? Number(e.target.value) : null)}
            aria-label={t('skills.pin.aria', qpName)}
          >
            <option value="">
              {versions === null ? t('common.loading')
                : latest != null ? t('skills.pin.latestNumbered', latest) : t('skills.pin.latest')}
            </option>
            {pinned != null && !versions?.some(v => v.version_index === pinned) && (
              <option value={pinned}>{t('skills.pin.pinned', pinned)}</option>
            )}
            {(versions ?? []).map(v => (
              <option key={v.version_index} value={v.version_index}>
                {t('skills.pin.pinned', v.version_index)}{v.version_index === latest ? ` · ${t('skills.history.current')}` : ''}
              </option>
            ))}
          </select>
        </label>
        {behind && <small className="skill-version-pin-note" data-testid="qp-version-pin-behind">{t('skills.pin.behind', pinned, latest)}</small>}
        {loaded?.failed && <small className="skill-version-pin-note" role="alert">{t('skills.pin.historyUnavailable')}</small>}
        {pinnedVersion && (
          <code className="wf-qref-field-value" data-testid="qp-version-pin-preview">
            {pinnedVersion.prompt_template.length > 200 ? `${pinnedVersion.prompt_template.slice(0, 200)}…` : pinnedVersion.prompt_template}
          </code>
        )}
      </div>
    </div>
  );
}

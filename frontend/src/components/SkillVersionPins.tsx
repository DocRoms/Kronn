/**
 * KT-1447 — for each custom skill picked, load its latest version (`custom-x`)
 * or pin one (`custom-x@N`). Built-in and repository skills have no versions.
 */
import { useEffect, useState } from 'react';
import { useT } from '../lib/I18nContext';
import { skills as skillsApi } from '../lib/api';
import { skillBaseId, skillPinnedVersion } from '../lib/automationSkills';
import type { Skill, SkillVersion } from '../types/generated';
import './SkillVersionPins.css';

interface Props {
  ids: readonly string[];
  catalog: readonly Skill[];
  onChange: (ids: string[]) => void;
}

function Pin({ id, name, onPick }: { id: string; name: string; onPick: (next: string) => void }) {
  const { t } = useT();
  const base = skillBaseId(id);
  const [versions, setVersions] = useState<SkillVersion[] | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let cancelled = false;
    skillsApi.history(base)
      .then(list => { if (!cancelled) { setVersions(list ?? []); setFailed(false); } })
      .catch(() => { if (!cancelled) { setVersions([]); setFailed(true); } });
    return () => { cancelled = true; };
  }, [base]);
  const pinned = skillPinnedVersion(id);
  const latest = versions?.reduce((max, v) => Math.max(max, v.version_index), 0) || null;
  const behind = pinned != null && latest != null && latest > pinned ? { pinned, latest } : null;
  return (
    <div className="skill-version-pin" data-testid={`skill-version-pin-${base}`}>
      <label>
        <span className="skill-version-pin-name">{name}</span>
        <select
          value={pinned ?? ''}
          disabled={versions === null}
          onChange={e => onPick(e.target.value ? `${base}@${e.target.value}` : base)}
          aria-label={t('skills.pin.aria', name)}
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
              {t('skills.pin.pinned', v.version_index)}{v.current ? ` · ${t('skills.history.current')}` : ''}
            </option>
          ))}
        </select>
      </label>
      {behind && <small className="skill-version-pin-note" data-testid={`skill-version-pin-behind-${base}`}>{t('skills.pin.behind', behind.pinned, behind.latest)}</small>}
      {failed && <small className="skill-version-pin-note" role="alert">{t('skills.pin.historyUnavailable')}</small>}
    </div>
  );
}

export default function SkillVersionPins({ ids, catalog, onChange }: Props) {
  const { t } = useT();
  // One row per skill, even if a stale list holds `custom-x` and `custom-x@2`.
  const custom = [...new Map(ids.filter(id => skillBaseId(id).startsWith('custom-')).map(id => [skillBaseId(id), id])).values()];
  if (custom.length === 0) return null;
  return (
    <div className="skill-version-pins" data-testid="skill-version-pins">
      <small>{t('skills.pin.hint')}</small>
      {custom.map(id => {
        const base = skillBaseId(id);
        const name = catalog.find(skill => skill.id === base)?.name ?? base;
        return (
          <Pin
            key={base}
            id={id}
            name={name}
            onPick={next => onChange([...new Set(ids.map(current => (skillBaseId(current) === base ? next : current)))])}
          />
        );
      })}
    </div>
  );
}

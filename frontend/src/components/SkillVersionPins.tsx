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
  useEffect(() => {
    let cancelled = false;
    skillsApi.history(base)
      .then(list => { if (!cancelled) setVersions(list ?? []); })
      .catch(() => { if (!cancelled) setVersions([]); });
    return () => { cancelled = true; };
  }, [base]);
  const pinned = skillPinnedVersion(id);
  return (
    <label className="skill-version-pin" data-testid={`skill-version-pin-${base}`}>
      <span>{name}</span>
      <select
        value={pinned ?? ''}
        onChange={e => onPick(e.target.value ? `${base}@${e.target.value}` : base)}
        aria-label={t('skills.pin.aria', name)}
      >
        <option value="">{t('skills.pin.latest')}</option>
        {pinned != null && !versions?.some(v => v.version_index === pinned) && (
          <option value={pinned}>v{pinned}</option>
        )}
        {(versions ?? []).map(v => (
          <option key={v.version_index} value={v.version_index}>
            v{v.version_index}{v.current ? ` · ${t('skills.history.current')}` : ''}
          </option>
        ))}
      </select>
    </label>
  );
}

export default function SkillVersionPins({ ids, catalog, onChange }: Props) {
  const { t } = useT();
  const custom = ids.filter(id => skillBaseId(id).startsWith('custom-'));
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
            onPick={next => onChange(ids.map(current => (current === id ? next : current)))}
          />
        );
      })}
    </div>
  );
}

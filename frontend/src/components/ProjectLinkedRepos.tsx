// 0.8.3 — Linked repos (companion projects) editor.
// Lives as a collapsible section in ProjectCard between "Skills" and
// "AI Context". UX kept deliberately minimal: list view + inline add
// form + per-row remove. The backend endpoint is atomic-replace so we
// re-POST the whole list on every change — no per-row CRUD risk.
//
// Why this file is a separate component (rather than inline in
// ProjectCard.tsx like ProjectSkills before it): the add form has
// its own state (kind dropdown, validation, draft mode) that would
// otherwise add ~60 lines of useState to ProjectCard, which is
// already pushing 1500 LoC.

import { useState } from 'react';
import { Plus, Trash2, ExternalLink, Folder, AlertCircle } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { projects as projectsApi } from '../lib/api';
import type { LinkedRepo } from '../types/generated';

/** Browser-native UUID (no dep needed). Modern browsers + Node 19+. */
function newId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  // Fallback for older test environments — RFC4122-ish enough for our needs.
  return `lr-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

const KINDS: Array<{ value: LinkedRepo['kind']; emoji: string; label: string }> = [
  { value: 'api',        emoji: '🔌', label: 'API' },
  { value: 'iac',        emoji: '🏗️', label: 'IaC' },
  { value: 'design',     emoji: '🎨', label: 'Design system' },
  { value: 'shared-lib', emoji: '📦', label: 'Shared lib' },
  { value: 'docs',       emoji: '📚', label: 'Docs' },
  { value: 'other',      emoji: '🔗', label: 'Other' },
];

function kindEmoji(kind: string): string {
  return KINDS.find(k => k.value === kind)?.emoji ?? '🔗';
}

/** Format heuristic: if it starts with `/` or `~` it's a filesystem
 *  path → render as <code>. Otherwise it's a URL → render as a link.
 *  Keeps the UI honest about what each location actually is. */
function isFilesystemPath(location: string): boolean {
  return location.startsWith('/') || location.startsWith('~') || location.startsWith('./');
}

export interface ProjectLinkedReposProps {
  projectId: string;
  currentRepos: LinkedRepo[];
  /** Fires after a successful PUT so the parent can refetch the
   *  project and the new list flows back as `currentRepos`. */
  onUpdate: () => void;
}

export function ProjectLinkedRepos({ projectId, currentRepos, onUpdate }: ProjectLinkedReposProps) {
  const { t } = useT();
  const [drafting, setDrafting] = useState(false);
  const [draftName, setDraftName] = useState('');
  const [draftKind, setDraftKind] = useState<string>('api');
  const [draftLocation, setDraftLocation] = useState('');
  const [draftDescription, setDraftDescription] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function resetDraft() {
    setDrafting(false);
    setDraftName('');
    setDraftKind('api');
    setDraftLocation('');
    setDraftDescription('');
    setError(null);
  }

  async function commitList(next: LinkedRepo[]) {
    setSaving(true);
    setError(null);
    try {
      await projectsApi.setLinkedRepos(projectId, next);
      onUpdate();
      resetDraft();
    } catch (e) {
      setError((e as Error).message || t('linkedRepos.saveError'));
    } finally {
      setSaving(false);
    }
  }

  async function handleAdd() {
    if (!draftName.trim() || !draftLocation.trim()) {
      setError(t('linkedRepos.requiredField'));
      return;
    }
    const next: LinkedRepo[] = [
      ...currentRepos,
      {
        id: newId(),
        name: draftName.trim(),
        kind: draftKind,
        location: draftLocation.trim(),
        description: draftDescription.trim(),
      },
    ];
    await commitList(next);
  }

  async function handleRemove(id: string) {
    if (!window.confirm(t('linkedRepos.confirmRemove'))) return;
    const next = currentRepos.filter(r => r.id !== id);
    await commitList(next);
  }

  return (
    <div style={{ paddingTop: 6 }}>
      <p className="text-xs text-muted" style={{ marginBottom: 8 }}>
        {t('linkedRepos.hint')}
      </p>

      {currentRepos.length === 0 && !drafting && (
        <p className="text-xs text-ghost" style={{ marginBottom: 8 }}>{t('linkedRepos.empty')}</p>
      )}

      {currentRepos.map(repo => (
        <div key={repo.id} className="flex-row gap-3" style={{ alignItems: 'flex-start', padding: '6px 0', borderBottom: '1px solid var(--kr-border-soft)' }}>
          <span style={{ fontSize: '1.1em' }}>{kindEmoji(repo.kind)}</span>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div className="flex-row gap-3" style={{ alignItems: 'center' }}>
              <strong className="text-sm">{repo.name}</strong>
              <span className="text-2xs text-ghost">{repo.kind}</span>
            </div>
            <div style={{ marginTop: 2 }}>
              {isFilesystemPath(repo.location) ? (
                <code className="text-xs" style={{ color: 'var(--kr-text-muted)' }}>
                  <Folder size={10} style={{ display: 'inline', marginRight: 4 }} />
                  {repo.location}
                </code>
              ) : (
                <a href={repo.location} target="_blank" rel="noopener noreferrer" className="text-xs"
                   style={{ color: 'var(--kr-cyan)', textDecoration: 'none' }}>
                  <ExternalLink size={10} style={{ display: 'inline', marginRight: 4 }} />
                  {repo.location}
                </a>
              )}
            </div>
            {repo.description && (
              <p className="text-2xs text-ghost" style={{ marginTop: 2 }}>{repo.description}</p>
            )}
          </div>
          <button
            className="dash-icon-btn dash-btn-cancel"
            onClick={() => handleRemove(repo.id)}
            disabled={saving}
            title={t('linkedRepos.remove')}
            aria-label={t('linkedRepos.remove')}
          >
            <Trash2 size={11} />
          </button>
        </div>
      ))}

      {drafting && (
        <div style={{ padding: '8px 0', borderTop: currentRepos.length > 0 ? '1px solid var(--kr-border-soft)' : 'none', marginTop: 6 }}>
          <div className="flex-row gap-3" style={{ marginBottom: 6 }}>
            <input
              className="dash-input text-sm"
              style={{ flex: 1 }}
              placeholder={t('linkedRepos.namePlaceholder')}
              value={draftName}
              onChange={e => setDraftName(e.target.value)}
              disabled={saving}
            />
            <select
              className="dash-input text-sm"
              style={{ width: 130 }}
              value={draftKind}
              onChange={e => setDraftKind(e.target.value)}
              disabled={saving}
            >
              {KINDS.map(k => (
                <option key={k.value} value={k.value}>{k.emoji} {k.label}</option>
              ))}
            </select>
          </div>
          <input
            className="dash-input text-sm"
            style={{ width: '100%', marginBottom: 6 }}
            placeholder={t('linkedRepos.locationPlaceholder')}
            value={draftLocation}
            onChange={e => setDraftLocation(e.target.value)}
            disabled={saving}
          />
          <input
            className="dash-input text-sm"
            style={{ width: '100%', marginBottom: 6 }}
            placeholder={t('linkedRepos.descriptionPlaceholder')}
            value={draftDescription}
            onChange={e => setDraftDescription(e.target.value)}
            disabled={saving}
          />
          {error && (
            <p className="text-xs" style={{ color: 'var(--kr-error)', marginBottom: 6 }}>
              <AlertCircle size={11} style={{ display: 'inline', marginRight: 4 }} />
              {error}
            </p>
          )}
          <div className="flex-row gap-3">
            <button className="dash-icon-btn" onClick={resetDraft} disabled={saving}>
              {t('common.cancel')}
            </button>
            <button className="dash-icon-btn dash-btn-accent-border" onClick={handleAdd} disabled={saving}>
              {saving ? '…' : t('linkedRepos.addBtn')}
            </button>
          </div>
        </div>
      )}

      {!drafting && (
        <button
          className="dash-icon-btn"
          style={{ marginTop: 8 }}
          onClick={() => setDrafting(true)}
          disabled={saving || currentRepos.length >= 20}
          title={currentRepos.length >= 20 ? t('linkedRepos.maxReached') : undefined}
        >
          <Plus size={11} /> {t('linkedRepos.addLink')}
        </button>
      )}
    </div>
  );
}

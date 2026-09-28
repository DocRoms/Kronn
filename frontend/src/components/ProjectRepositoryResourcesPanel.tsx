import { useEffect, useMemo, useState } from 'react';
import { AlertTriangle, Check, FileCode2, FolderTree, Loader2 } from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import type {
  ProjectRepositoryResourceKind,
  ProjectRepositoryResourceLevel,
  ProjectRepositoryResourceStatus,
  ProjectRepositoryResources,
} from '../types/generated';
import './ProjectRepositoryResourcesPanel.css';

interface Props {
  projectId: string;
}

const STATUS_MARKER: Record<ProjectRepositoryResourceStatus, string> = {
  not_published: '+',
  up_to_date: '',
  repository_modified: '~',
  kronn_modified: '~',
  conflict: '!',
};

const resourceKey = (resource: ProjectRepositoryResources['resources'][number]) => (
  `${resource.kind}:${resource.id}`
);

export function ProjectRepositoryResourcesPanel({ projectId }: Props) {
  const { t } = useT();
  const [data, setData] = useState<ProjectRepositoryResources | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setData(null);
    setError(null);
    projectsApi.repositoryResources(projectId).then(result => {
      if (!active) return;
      setData(result);
      setSelected(new Set(result.resources
        .filter(resource => resource.status !== 'not_published')
        .map(resourceKey)));
    }).catch(reason => {
      if (active) setError(userError(reason));
    });
    return () => { active = false; };
  }, [projectId]);

  const tree = useMemo(() => {
    if (!data) return [];
    return data.resources.flatMap(resource => resource.repository_paths.map(path => ({ resource, path })));
  }, [data]);
  const repositoryScaffoldIncluded = Boolean(data?.kronn_exists || selected.size > 0);

  if (error) return <div className="project-repository-resources-error"><AlertTriangle size={15} /> {t('projects.repositoryResources.error')}: {error}</div>;
  if (!data) return <div className="project-repository-resources-loading"><Loader2 size={15} className="animate-spin" /> {t('projects.repositoryResources.loading')}</div>;

  return (
    <section className="project-repository-resources" data-project-view="automationArtifacts">
      {!data.kronn_exists && (
        <div className="project-repository-resources-banner" role="status">
          <FolderTree size={16} /> {t('projects.repositoryResources.kronnMissing')}
        </div>
      )}
      <div className="project-repository-resources-columns">
        <div className="project-repository-resources-list">
          <h3>{t('projects.repositoryResources.resources')}</h3>
          {data.resources.length === 0 && <p>{t('projects.repositoryResources.empty')}</p>}
          {data.resources.map(resource => {
            const key = resourceKey(resource);
            const checked = selected.has(key);
            return (
              <label key={key} className="project-repository-resource-row">
                <input
                  type="checkbox"
                  checked={checked}
                  disabled={resource.status !== 'not_published'}
                  onChange={() => setSelected(current => {
                    const next = new Set(current);
                    if (next.has(key)) next.delete(key); else next.add(key);
                    return next;
                  })}
                  aria-label={t('projects.repositoryResources.include', resource.name)}
                />
                <FileCode2 size={15} aria-hidden="true" />
                <span className="project-repository-resource-copy">
                  <strong>{resource.name}</strong>
                  <span>{kindLabel(t, resource.kind)} · {levelLabel(t, resource.level)}</span>
                </span>
                <span className="project-repository-resource-status" data-status={resource.status}>
                  {resource.status === 'up_to_date' && <Check size={12} aria-hidden="true" />}
                  {statusLabel(t, resource.status)}
                </span>
              </label>
            );
          })}
        </div>
        <div className="project-repository-resources-tree">
          <h3>{t('projects.repositoryResources.preview')}</h3>
          <div
            className="project-repository-tree-root"
            data-excluded={!repositoryScaffoldIncluded || undefined}
          >
            <span className="project-repository-tree-marker">
              {!data.kronn_exists && repositoryScaffoldIncluded ? '+' : ''}
            </span>
            <span>kronn/</span>
          </div>
          <div
            className="project-repository-tree-entry project-repository-tree-scaffold"
            data-excluded={!repositoryScaffoldIncluded || undefined}
          >
            <span className="project-repository-tree-marker">
              {!data.kronn_exists && repositoryScaffoldIncluded ? '+' : ''}
            </span>
            <span>INDEX.md · kronn.toml · kronn.lock</span>
          </div>
          {tree.map(({ resource, path }) => {
            const included = selected.has(resourceKey(resource));
            const relative = path.replace(/^kronn\//, '');
            return (
              <div key={`${resource.id}:${path}`} className="project-repository-tree-entry" data-excluded={!included || undefined}>
                <span className="project-repository-tree-marker" data-status={resource.status}>
                  {included ? STATUS_MARKER[resource.status] : ''}
                </span>
                <span>{relative}</span>
              </div>
            );
          })}
          <div className="project-repository-resources-legend">
            <span>+ {t('projects.repositoryResources.legend.added')}</span>
            <span>~ {t('projects.repositoryResources.legend.modified')}</span>
            <span>! {t('projects.repositoryResources.legend.conflict')}</span>
            <span className="project-repository-resources-excluded">{t('projects.repositoryResources.legend.excluded')}</span>
          </div>
        </div>
      </div>
    </section>
  );
}

type Translate = (key: string, ...args: Array<string | number>) => string;
const kindLabel = (t: Translate, kind: ProjectRepositoryResourceKind) => t(`projects.repositoryResources.kind.${kind}`);
const levelLabel = (t: Translate, level: ProjectRepositoryResourceLevel) => t(`projects.repositoryResources.level.${level}`);
const statusLabel = (t: Translate, status: ProjectRepositoryResourceStatus) => t(`projects.repositoryResources.status.${status}`);

import { useCallback, useEffect, useMemo, useState } from 'react';
import { AlertTriangle, Check, Download, FileCode2, FolderTree, Loader2, Package, ShieldCheck, Upload, Workflow, Zap } from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import {
  readProjectRepositoryResourcesTab,
  rememberProjectRepositoryResourcesTab,
  type ProjectRepositoryResourcesTab,
} from '../lib/projectRepositoryResourcesTab';
import { userError } from '../lib/userError';
import type {
  ProjectRepositoryResource,
  ProjectRepositoryResourceKind,
  ProjectRepositoryResourceLevel,
  ProjectRepositoryResourceStatus,
  ProjectRepositoryResources,
  ProjectRepositorySkill,
  ProjectRepositorySkillProvenance,
} from '../types/generated';
import './ProjectRepositoryResourcesPanel.css';

interface Props {
  projectId: string;
}

const AUTOMATION_KINDS: ProjectRepositoryResourceKind[] = [
  'workflow',
  'quick_prompt',
  'quick_exec',
  'quick_api',
];

const STATUS_MARKER: Record<ProjectRepositoryResourceStatus, string> = {
  not_published: '+',
  up_to_date: '',
  repository_modified: '~',
  kronn_modified: '~',
  conflict: '!',
};

const resourceKey = (resource: ProjectRepositoryResource) => `${resource.kind}:${resource.id}`;
const skillKey = (skill: ProjectRepositorySkill) => `skill:${skill.id}`;
const skillIsPublished = (skill: ProjectRepositorySkill) => (
  skill.repository_paths.includes(skill.publication_path)
);

type ResourceAction = (
  projectId: string,
  mode: 'publish' | 'import' | 'approve',
  kind: ProjectRepositoryResourceKind,
  id: string,
  slug: string,
  overwrite?: boolean,
) => Promise<unknown>;

export function ProjectRepositoryResourcesPanel({ projectId }: Props) {
  const { t } = useT();
  const [data, setData] = useState<ProjectRepositoryResources | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [activeTab, setActiveTab] = useState<ProjectRepositoryResourcesTab>(
    readProjectRepositoryResourcesTab,
  );
  const [error, setError] = useState<string | null>(null);
  const [busyKey, setBusyKey] = useState<string | null>(null);

  const applyResult = useCallback((result: ProjectRepositoryResources) => {
    setData(result);
    setSelected(new Set([
      ...result.resources
        .filter(resource => resource.status !== 'not_published')
        .map(resourceKey),
      ...result.skills_present
        .filter(skill => skillIsPublished(skill) || (skill.status && skill.status !== 'not_published'))
        .map(skillKey),
    ]));
  }, []);

  useEffect(() => {
    let active = true;
    setData(null);
    setError(null);
    projectsApi.repositoryResources(projectId).then(result => {
      if (!active) return;
      applyResult(result);
    }).catch(reason => {
      if (active) setError(userError(reason));
    });
    return () => { active = false; };
  }, [projectId, applyResult]);

  const runAction: ResourceAction = useAsyncGuard(async (
    targetProjectId,
    mode,
    kind,
    id,
    slug,
    overwrite = false,
  ) => {
    const key = `${mode}:${kind}:${id}`;
    setBusyKey(key);
    setError(null);
    try {
      if (mode === 'publish') {
        await projectsApi.publishRepositoryResource(targetProjectId, {
          kind,
          id,
          overwrite_repository_changes: overwrite,
        });
      } else if (mode === 'import') {
        await projectsApi.importRepositoryResource(targetProjectId, { kind, slug });
      } else {
        await projectsApi.approveRepositoryResource(targetProjectId, { kind, id });
      }
      applyResult(await projectsApi.repositoryResources(targetProjectId));
    } catch (reason) {
      setError(userError(reason));
    } finally {
      setBusyKey(null);
    }
  });

  const automationResources = useMemo(
    () => data?.resources.filter(resource => resource.kind !== 'artifact') ?? [],
    [data],
  );
  const artifacts = useMemo(
    () => data?.resources.filter(resource => resource.kind === 'artifact') ?? [],
    [data],
  );
  const tree = useMemo(() => {
    if (!data) return [];
    return [
      ...data.resources.flatMap(resource => resource.repository_paths.map(path => ({
        key: resourceKey(resource),
        id: resource.id,
        path,
        status: resource.status as ProjectRepositoryResourceStatus | undefined,
      }))),
      ...data.skills_present.map(skill => ({
        key: skillKey(skill),
        id: skill.id,
        path: skill.publication_path,
        status: skill.status,
      })),
    ];
  }, [data]);
  const repositoryScaffoldIncluded = Boolean(data?.kronn_exists || selected.size > 0);
  const selectedForPublication = data ? [
    ...data.resources
      .filter(resource => resource.status === 'not_published' && selected.has(resourceKey(resource)))
      .map(resource => ({ kind: resource.kind, id: resource.id, slug: resource.slug })),
    ...data.skills_present
      .filter(skill => skill.status === 'not_published' && selected.has(skillKey(skill)))
      .map(skill => ({ kind: 'skill' as const, id: skill.id, slug: skill.slug })),
  ] : [];

  const publishSelected = useAsyncGuard(async (
    targetProjectId: string,
    resources: Array<{ kind: ProjectRepositoryResourceKind; id: string; slug: string }>,
  ) => {
    setBusyKey('publish:selected');
    setError(null);
    try {
      for (const resource of resources) {
        await projectsApi.publishRepositoryResource(targetProjectId, {
          kind: resource.kind,
          id: resource.id,
          overwrite_repository_changes: false,
        });
      }
      applyResult(await projectsApi.repositoryResources(targetProjectId));
    } catch (reason) {
      setError(userError(reason));
    } finally {
      setBusyKey(null);
    }
  });

  const selectTab = (tab: ProjectRepositoryResourcesTab) => {
    setActiveTab(tab);
    rememberProjectRepositoryResourcesTab(tab);
  };
  const toggleSelected = (key: string) => setSelected(current => {
    const next = new Set(current);
    if (next.has(key)) next.delete(key); else next.add(key);
    return next;
  });

  if (error) return <div className="project-repository-resources-error"><AlertTriangle size={15} /> {t('projects.repositoryResources.error')}: {error}</div>;
  if (!data) return <div className="project-repository-resources-loading"><Loader2 size={15} className="animate-spin" /> {t('projects.repositoryResources.loading')}</div>;

  const tabs: Array<{ id: ProjectRepositoryResourcesTab; icon: typeof Zap; count: number }> = [
    { id: 'skills', icon: Zap, count: data.skills_present.length },
    { id: 'automation', icon: Workflow, count: automationResources.length },
    { id: 'artifacts', icon: Package, count: artifacts.length },
  ];

  return (
    <section className="project-repository-resources" data-project-view="resources">
      <div className="project-repository-resources-tabs" role="tablist" aria-label={t('projects.repositoryResources.tabs')}>
        {tabs.map(({ id, icon: Icon, count }) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={activeTab === id}
            data-active={activeTab === id}
            onClick={() => selectTab(id)}
          >
            <Icon size={14} aria-hidden="true" />
            {t(`projects.repositoryResources.tab.${id}`)}
            <span>{count}</span>
          </button>
        ))}
      </div>
      {!data.kronn_exists && (
        <div className="project-repository-resources-banner" role="status">
          <FolderTree size={16} /> {t('projects.repositoryResources.kronnMissing')}
        </div>
      )}
      {selectedForPublication.length > 0 && (
        <div className="project-repository-resources-actions">
          <button
            type="button"
            disabled={busyKey !== null}
            onClick={() => publishSelected(projectId, selectedForPublication)}
          >
            {busyKey === 'publish:selected' ? <Loader2 size={14} className="animate-spin" /> : <Upload size={14} />}
            {t('projects.repositoryResources.publishSelected', selectedForPublication.length)}
          </button>
        </div>
      )}
      <div className="project-repository-resources-columns">
        <div className="project-repository-resources-list" role="tabpanel">
          {activeTab === 'skills' && (
            <SkillsTab
              present={data.skills_present}
              available={data.skills_available}
              selected={selected}
              onToggle={toggleSelected}
              onAction={runAction}
              projectId={projectId}
              busyKey={busyKey}
              t={t}
            />
          )}
          {activeTab === 'automation' && (
            <AutomationTab
              resources={automationResources}
              selected={selected}
              onToggle={toggleSelected}
              onAction={runAction}
              projectId={projectId}
              busyKey={busyKey}
              t={t}
            />
          )}
          {activeTab === 'artifacts' && (
            <ResourceList
              resources={artifacts}
              selected={selected}
              onToggle={toggleSelected}
              onAction={runAction}
              projectId={projectId}
              busyKey={busyKey}
              emptyKey="projects.repositoryResources.emptyArtifacts"
              t={t}
            />
          )}
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
          {tree.map(item => {
            const included = selected.has(item.key);
            const relative = item.path.replace(/^kronn\//, '');
            return (
              <div key={`${item.id}:${item.path}`} className="project-repository-tree-entry" data-excluded={!included || undefined}>
                <span className="project-repository-tree-marker" data-status={item.status}>
                  {included && item.status ? STATUS_MARKER[item.status] : ''}
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

interface ResourceListProps {
  resources: ProjectRepositoryResource[];
  selected: Set<string>;
  onToggle: (key: string) => void;
  onAction: ResourceAction;
  projectId: string;
  busyKey: string | null;
  emptyKey: string;
  t: Translate;
}

function ResourceList({ resources, selected, onToggle, onAction, projectId, busyKey, emptyKey, t }: ResourceListProps) {
  if (resources.length === 0) return <p>{t(emptyKey)}</p>;
  return <>{resources.map(resource => (
    <ResourceRow
      key={resourceKey(resource)}
      resource={resource}
      checked={selected.has(resourceKey(resource))}
      onToggle={() => onToggle(resourceKey(resource))}
      onAction={onAction}
      projectId={projectId}
      busyKey={busyKey}
      t={t}
    />
  ))}</>;
}

function ResourceRow({ resource, checked, onToggle, onAction, projectId, busyKey, t }: {
  resource: ProjectRepositoryResource;
  checked: boolean;
  onToggle: () => void;
  onAction: ResourceAction;
  projectId: string;
  busyKey: string | null;
  t: Translate;
}) {
  const actionBusy = busyKey?.endsWith(`:${resource.kind}:${resource.id}`) ?? false;
  return (
    <div className="project-repository-resource-row">
      <input
        type="checkbox"
        checked={checked}
        disabled={resource.status !== 'not_published'}
        onChange={onToggle}
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
      <ResourceActions
        status={resource.status}
        approvalRequired={resource.approval_required}
        diff={resource.diff}
        busy={actionBusy}
        onAction={(mode, overwrite) => onAction(projectId, mode, resource.kind, resource.id, resource.slug, overwrite)}
        t={t}
      />
    </div>
  );
}

function AutomationTab({ resources, selected, onToggle, onAction, projectId, busyKey, t }: Omit<ResourceListProps, 'emptyKey'>) {
  if (resources.length === 0) return <p>{t('projects.repositoryResources.emptyAutomation')}</p>;
  return <div className="project-repository-resource-groups">
    {AUTOMATION_KINDS.map(kind => {
      const group = resources.filter(resource => resource.kind === kind);
      if (group.length === 0) return null;
      return <section key={kind} data-resource-kind={kind}>
        <h3>{kindLabel(t, kind)} <span>{group.length}</span></h3>
        <ResourceList resources={group} selected={selected} onToggle={onToggle} onAction={onAction} projectId={projectId} busyKey={busyKey} emptyKey="" t={t} />
      </section>;
    })}
  </div>;
}

function SkillsTab({ present, available, selected, onToggle, onAction, projectId, busyKey, t }: {
  present: ProjectRepositorySkill[];
  available: ProjectRepositorySkill[];
  selected: Set<string>;
  onToggle: (key: string) => void;
  onAction: ResourceAction;
  projectId: string;
  busyKey: string | null;
  t: Translate;
}) {
  return <div className="project-repository-skill-groups">
    <section>
      <h3>{t('projects.repositoryResources.skills.present')} <span>{present.length}</span></h3>
      {present.length === 0 && <p>{t('projects.repositoryResources.emptySkills')}</p>}
      {present.map(skill => {
        const key = skillKey(skill);
        const published = skillIsPublished(skill);
        return <div key={key} className="project-repository-resource-row">
          <input
            type="checkbox"
            checked={selected.has(key)}
            disabled={published}
            onChange={() => onToggle(key)}
            aria-label={t('projects.repositoryResources.include', skill.name)}
          />
          <Zap size={15} aria-hidden="true" />
          <span className="project-repository-resource-copy">
            <strong>{skill.name}</strong>
            <span>{provenanceLabel(t, skill.provenance)} · {levelLabel(t, 'usable_without_kronn')}</span>
            {skill.description && <small>{skill.description}</small>}
          </span>
          {skill.status && (
            <span className="project-repository-resource-status" data-status={skill.status}>
              {skill.status === 'up_to_date' && <Check size={12} aria-hidden="true" />}
              {statusLabel(t, skill.status)}
            </span>
          )}
          {skill.status && (
            <ResourceActions
              status={skill.status}
              approvalRequired={skill.approval_required}
              diff={skill.diff}
              busy={busyKey?.endsWith(`:skill:${skill.id}`) ?? false}
              onAction={(mode, overwrite) => onAction(projectId, mode, 'skill', skill.id, skill.slug, overwrite)}
              t={t}
            />
          )}
        </div>;
      })}
    </section>
    <section>
      <h3>{t('projects.repositoryResources.skills.available')} <span>{available.length}</span></h3>
      {available.length === 0 && <p>{t('projects.repositoryResources.emptyAvailableSkills')}</p>}
      {available.map(skill => (
        <div key={skillKey(skill)} className="project-repository-resource-row project-repository-skill-available">
          <Zap size={15} aria-hidden="true" />
          <span className="project-repository-resource-copy">
            <strong>{skill.name}</strong>
            <span>{provenanceLabel(t, skill.provenance)}</span>
            {skill.description && <small>{skill.description}</small>}
          </span>
        </div>
      ))}
    </section>
  </div>;
}

function ResourceActions({ status, approvalRequired, diff, busy, onAction, t }: {
  status: ProjectRepositoryResourceStatus;
  approvalRequired: boolean;
  diff?: string;
  busy: boolean;
  onAction: (mode: 'publish' | 'import' | 'approve', overwrite?: boolean) => Promise<unknown>;
  t: Translate;
}) {
  return <div className="project-repository-resource-actions">
    {busy && <Loader2 size={13} className="animate-spin" aria-label={t('common.loading')} />}
    {!busy && status === 'repository_modified' && (
      <button type="button" onClick={() => onAction('import')}>
        <Download size={12} /> {t('projects.repositoryResources.import')}
      </button>
    )}
    {!busy && status === 'kronn_modified' && (
      <button type="button" onClick={() => onAction('publish')}>
        <Upload size={12} /> {t('projects.repositoryResources.publish')}
      </button>
    )}
    {!busy && status === 'conflict' && <>
      <details>
        <summary>{t('projects.repositoryResources.showDiff')}</summary>
        <pre>{diff}</pre>
      </details>
      <button type="button" onClick={() => onAction('import')}>
        <Download size={12} /> {t('projects.repositoryResources.keepRepository')}
      </button>
      <button type="button" onClick={() => onAction('publish', true)}>
        <Upload size={12} /> {t('projects.repositoryResources.keepKronn')}
      </button>
    </>}
    {!busy && approvalRequired && status === 'up_to_date' && (
      <button type="button" onClick={() => onAction('approve')}>
        <ShieldCheck size={12} /> {t('projects.repositoryResources.approve')}
      </button>
    )}
  </div>;
}

const kindLabel = (t: Translate, kind: ProjectRepositoryResourceKind) => t(`projects.repositoryResources.kind.${kind}`);
const levelLabel = (t: Translate, level: ProjectRepositoryResourceLevel) => t(`projects.repositoryResources.level.${level}`);
const statusLabel = (t: Translate, status: ProjectRepositoryResourceStatus) => t(`projects.repositoryResources.status.${status}`);
const provenanceLabel = (t: Translate, provenance: ProjectRepositorySkillProvenance) => t(`projects.repositoryResources.provenance.${provenance}`);

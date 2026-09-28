import { useCallback, useEffect, useMemo, useState } from 'react';
import { AlertTriangle, ArrowRightLeft, GitBranch, Loader2, Package, Search, Workflow, Zap, FolderTree, Lock } from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import {
  readProjectRepositoryResourcesTab,
  rememberProjectRepositoryResourcesTab,
  type ProjectRepositoryResourcesTab,
} from '../lib/projectRepositoryResourcesTab';
import { removeFromKronn } from '../lib/repositoryResourceExecution';
import type { TransferKind, TransferPlan } from '../lib/repositoryResourceEffects';
import {
  alignLines,
  attachedSkillIds,
  attentionItems,
  buildRows,
  matchesPresence,
  matchesQuery,
  writesRepository,
  type AlignLine,
  type AttentionItem,
  type PresenceFilter,
  type RepositoryRows,
  type ResourceRow,
} from '../lib/repositoryResourceRows';
import { userError } from '../lib/userError';
import type {
  ProjectRepositoryResourceKind,
  ProjectRepositoryResourceStatus,
  ProjectRepositoryResources,
} from '../types/generated';
import { RepositoryResourceAlign } from './RepositoryResourceAlign';
import { RepositoryResourceApprove } from './RepositoryResourceApprove';
import { RepositoryResourceCompare } from './RepositoryResourceCompare';
import { RepositoryResourceRow, type RowMenuAction } from './RepositoryResourceRow';
import { RepositoryResourceTransfer } from './RepositoryResourceTransfer';
import './ProjectRepositoryResourcesPanel.css';

interface Props {
  projectId: string;
  /** Called with the number of items waiting for a decision, once known. */
  onAttentionChange?: (count: number) => void;
  onOpenGit?: () => void;
  onAddKey?: () => void;
}

const AUTOMATION_KINDS: ProjectRepositoryResourceKind[] = [
  'workflow',
  'quick_prompt',
  'quick_exec',
  'quick_api',
];

const TAB_ICON = { skills: Zap, automation: Workflow, artifacts: Package } as const;
const FILTERS: PresenceFilter[] = ['all', 'repository', 'kronn', 'both'];
const ATTENTION_VISIBLE = 5;

const STATUS_MARKER: Record<ProjectRepositoryResourceStatus, string> = {
  kronn_only: '+',
  up_to_date: '',
  repository_only: '~',
  repository_newer: '~',
  kronn_newer: '~',
  conflict: '!',
  approval_required: '',
  native_skill: '',
};

const ATTENTION_SENTENCE: Partial<Record<ResourceRow['state'], string>> = {
  conflict: 'conflict',
  approval_required: 'approval',
  repository_newer: 'repositoryNewer',
  kronn_newer: 'kronnNewer',
  repository_only: 'repositoryOnly',
  native_skill: 'nativeSkill',
};

/** Whether a row's file is already part of kronn/ — the preview tree's "included" set. */
const includedByDefault = (row: ResourceRow) => (
  row.state !== 'kronn_only'
  && row.state !== 'native_skill'
  && row.state !== 'catalog'
  && !(row.attachOnly && !row.paths.includes(row.targetPath))
);

type Sheet = { type: 'compare' | 'approve'; key: string };

export function ProjectRepositoryResourcesPanel({ projectId, onAttentionChange, onOpenGit, onAddKey }: Props) {
  const { t } = useT();
  const [data, setData] = useState<ProjectRepositoryResources | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [activeTab, setActiveTab] = useState<ProjectRepositoryResourcesTab>(
    readProjectRepositoryResourcesTab,
  );
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<PresenceFilter>('all');
  const [attentionOpen, setAttentionOpen] = useState(false);
  const [transfer, setTransfer] = useState<TransferPlan | null>(null);
  const [sheet, setSheet] = useState<Sheet | null>(null);
  const [alignOpen, setAlignOpen] = useState(false);

  const applyResult = useCallback((result: ProjectRepositoryResources) => {
    setData(result);
    const rows = buildRows(result);
    setSelected(new Set(
      [...rows.skills, ...rows.automation, ...rows.artifacts]
        .filter(includedByDefault)
        .map(row => row.key),
    ));
  }, []);

  useEffect(() => {
    let active = true;
    setData(null);
    setLoadError(null);
    setActionError(null);
    projectsApi.repositoryResources(projectId).then(result => {
      if (!active) return;
      applyResult(result);
    }).catch(reason => {
      if (active) setLoadError(userError(reason));
    });
    return () => { active = false; };
  }, [projectId, applyResult]);

  const rows: RepositoryRows | null = useMemo(() => (data ? buildRows(data) : null), [data]);
  const attention: AttentionItem[] = useMemo(() => (rows ? attentionItems(rows) : []), [rows]);
  const attentionTotal = attention.length;
  const loaded = data !== null;

  useEffect(() => {
    if (loaded) onAttentionChange?.(attentionTotal);
  }, [loaded, attentionTotal, onAttentionChange]);

  const canWrite = data?.can_write_repository !== false;
  const kronnExists = Boolean(data?.kronn_exists);

  const runTask = useAsyncGuard(async (
    targetProjectId: string,
    busy: string,
    task: () => Promise<unknown>,
  ) => {
    setBusyKey(busy);
    setActionError(null);
    try {
      await task();
      applyResult(await projectsApi.repositoryResources(targetProjectId));
      return true;
    } catch (reason) {
      setActionError(userError(reason));
      try {
        applyResult(await projectsApi.repositoryResources(targetProjectId));
      } catch {
        // The failure above is the one worth showing.
      }
      return false;
    } finally {
      setBusyKey(null);
    }
  });

  const refresh = useAsyncGuard(async (targetProjectId: string) => {
    setActionError(null);
    try {
      applyResult(await projectsApi.repositoryResources(targetProjectId));
    } catch (reason) {
      setActionError(userError(reason));
    }
  });

  const selectTab = (tab: ProjectRepositoryResourcesTab) => {
    setActiveTab(tab);
    setFilter('all');
    rememberProjectRepositoryResourcesTab(tab);
  };
  const toggleSelected = (key: string) => setSelected(current => {
    const next = new Set(current);
    if (next.has(key)) next.delete(key); else next.add(key);
    return next;
  });

  const tree = useMemo(() => {
    if (!data) return [];
    return [
      ...data.resources.flatMap(resource => resource.repository_paths.map(path => ({
        key: `${resource.kind}:${resource.id}`,
        id: resource.id,
        path,
        status: resource.status as ProjectRepositoryResourceStatus | undefined,
      }))),
      ...data.skills_present.map(skill => ({
        key: `skill:${skill.id}`,
        id: skill.id,
        path: skill.publication_path,
        status: skill.status,
      })),
    ];
  }, [data]);

  if (loadError) return <div className="project-repository-resources-error"><AlertTriangle size={15} /> {t('projects.repositoryResources.error')}: {loadError}</div>;
  if (!data || !rows) return <div className="project-repository-resources-loading"><Loader2 size={15} className="animate-spin" /> {t('projects.repositoryResources.loading')}</div>;

  const transferTask = (plan: TransferPlan, nativePath?: string) => async () => {
    const [row] = plan.rows;
    switch (plan.kind) {
      case 'publish':
      case 'update_repository':
        await projectsApi.publishRepositoryResource(projectId, {
          kind: row.kind, id: row.id, overwrite_repository_changes: false,
        });
        break;
      case 'import':
      case 'update_kronn':
        await projectsApi.importRepositoryResource(projectId, {
          kind: row.kind, slug: row.slug, overwrite_kronn_changes: false,
        });
        break;
      case 'use_native':
        await projectsApi.useNativeSkill(projectId, { relative_path: nativePath ?? row.paths[0] });
        break;
      case 'copy_native':
        await projectsApi.copyNativeSkill(projectId, { relative_path: nativePath ?? row.paths[0] });
        break;
      case 'attach':
        await projectsApi.setDefaultSkills(projectId, [...attachedSkillIds(data), row.id]);
        break;
      case 'publish_selected':
        for (const item of plan.rows) {
          await projectsApi.publishRepositoryResource(projectId, {
            kind: item.kind, id: item.id, overwrite_repository_changes: false,
          });
        }
        break;
    }
  };

  const confirmTransfer = async (plan: TransferPlan, nativePath?: string) => {
    await runTask(projectId, `${plan.kind}:${plan.rows[0].key}`, transferTask(plan, nativePath));
    setTransfer(null);
  };

  const startAction = (row: ResourceRow) => {
    if (row.primary === 'compare' || row.primary === 'approve') {
      setSheet({ type: row.primary, key: row.key });
    } else if (row.primary === 'view') {
      setSheet({ type: 'compare', key: row.key });
    } else {
      setTransfer({ kind: row.primary as TransferKind, rows: [row] });
    }
  };

  const onMenu = (row: ResourceRow, action: RowMenuAction) => {
    if (action === 'copy_native') setTransfer({ kind: 'copy_native', rows: [row] });
    else setSheet({ type: 'compare', key: row.key });
  };

  const keepSide = async (row: ResourceRow, side: 'repository' | 'kronn') => {
    const done = await runTask(projectId, `keep:${row.key}`, async () => {
      if (side === 'repository') {
        await projectsApi.importRepositoryResource(projectId, {
          kind: row.kind, slug: row.slug, overwrite_kronn_changes: true,
        });
      } else {
        await projectsApi.publishRepositoryResource(projectId, {
          kind: row.kind, id: row.id, overwrite_repository_changes: true,
        });
      }
    });
    if (done) setSheet(null);
  };

  const approve = async (row: ResourceRow) => {
    const done = await runTask(projectId, `approve:${row.key}`, () => (
      projectsApi.approveRepositoryResource(projectId, { kind: row.kind, id: row.id })
    ));
    if (done) setSheet(null);
  };

  const reject = async (row: ResourceRow) => {
    const done = await runTask(projectId, `reject:${row.key}`, () => removeFromKronn(row.kind, row.id));
    if (done) setSheet(null);
  };

  const alignAll = async (lines: AlignLine[]) => {
    await runTask(projectId, 'align', async () => {
      for (const { row, direction } of lines) {
        if (direction === 'to_kronn') {
          await projectsApi.importRepositoryResource(projectId, {
            kind: row.kind, slug: row.slug, overwrite_kronn_changes: false,
          });
        } else {
          await projectsApi.publishRepositoryResource(projectId, {
            kind: row.kind, id: row.id, overwrite_repository_changes: false,
          });
        }
      }
    });
    setAlignOpen(false);
  };

  const nativeSkillRoots = data.skill_roots ?? [];
  const hasNativeSkills = nativeSkillRoots.some(root => root.path !== 'kronn/skills' && root.skill_count > 0);
  const uncommitted = data.uncommitted_managed_paths ?? [];
  const repositoryScaffoldIncluded = Boolean(data.kronn_exists || selected.size > 0);
  const allLines = alignLines(rows);
  const lineCount = allLines.filter(line => line.direction === 'to_kronn' || canWrite).length;
  const excludedCount = attention.filter(item => item.reason === 'conflict' || item.reason === 'approval').length;
  const selectedForPublication = [...rows.skills, ...rows.automation, ...rows.artifacts]
    .filter(row => row.state === 'kronn_only' && selected.has(row.key));
  const sheetRow = sheet
    ? [...rows.skills, ...rows.automation, ...rows.artifacts].find(row => row.key === sheet.key)
    : undefined;
  const attentionByTab = (tab: ProjectRepositoryResourcesTab) => attention.filter(item => item.row.group === tab).length;
  const visibleAttention = attentionOpen ? attention : attention.slice(0, ATTENTION_VISIBLE);

  const tabs: Array<{ id: ProjectRepositoryResourcesTab; count: number }> = [
    { id: 'skills', count: rows.skills.length },
    { id: 'automation', count: rows.automation.length },
    { id: 'artifacts', count: rows.artifacts.length },
  ];

  const tabRows = activeTab === 'skills'
    ? [...rows.skills, ...rows.catalog]
    : activeTab === 'automation' ? rows.automation : rows.artifacts;
  const searched = tabRows.filter(row => matchesQuery(row, query));
  const visibleRows = searched.filter(row => matchesPresence(row, filter));
  const visibleSet = new Set(visibleRows.map(row => row.key));

  const sections: Array<{ id: string; title: string; kind?: string; rows: ResourceRow[] }> = [];
  if (activeTab === 'skills') {
    sections.push(
      { id: 'present', title: t('projects.repositoryResources.skills.present'), rows: rows.skills.filter(row => visibleSet.has(row.key)) },
      { id: 'available', title: t('projects.repositoryResources.skills.available'), rows: rows.catalog.filter(row => visibleSet.has(row.key)) },
    );
  } else if (activeTab === 'automation') {
    AUTOMATION_KINDS.forEach(kind => {
      sections.push({
        id: kind,
        kind,
        title: t(`projects.repositoryResources.kind.${kind}`),
        rows: rows.automation.filter(row => row.kind === kind && visibleSet.has(row.key)),
      });
    });
  } else {
    sections.push({ id: 'artifacts', title: t('projects.repositoryResources.kind.artifact'), rows: rows.artifacts.filter(row => visibleSet.has(row.key)) });
  }

  const emptyKey = activeTab === 'skills'
    ? 'projects.repositoryResources.emptySkills'
    : activeTab === 'automation'
      ? 'projects.repositoryResources.emptyAutomation'
      : 'projects.repositoryResources.emptyArtifacts';

  return (
    <section className="project-repository-resources" data-project-view="resources">
      <div className="project-repository-resources-tabs" role="tablist" aria-label={t('projects.repositoryResources.tabs')}>
        {tabs.map(({ id, count }) => {
          const Icon = TAB_ICON[id];
          const todo = attentionByTab(id);
          return (
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
              {todo > 0 && (
                <span className="rr-todo" title={t('projects.repositoryResources.tab.todo', todo)}>{todo}</span>
              )}
            </button>
          );
        })}
      </div>

      {!data.kronn_exists && canWrite && (
        <div
          className="project-repository-resources-banner"
          data-tone={hasNativeSkills ? 'secondary' : undefined}
          role="status"
        >
          <FolderTree size={16} />{' '}
          {t(hasNativeSkills
            ? 'projects.repositoryResources.kronnMissingSecondary'
            : 'projects.repositoryResources.kronnMissing')}
        </div>
      )}
      {!canWrite && (
        <div className="project-repository-resources-banner" data-tone="secondary" role="status" data-banner="write-disabled">
          <Lock size={16} aria-hidden="true" />
          <div>
            <strong>{t('projects.repositoryResources.banner.writeDisabled.title')}</strong>
            <p>{t('projects.repositoryResources.banner.writeDisabled.body')}</p>
            {data.can_write_repository_reason && <code>{data.can_write_repository_reason}</code>}
          </div>
        </div>
      )}
      {uncommitted.length > 0 && (
        <div className="project-repository-resources-banner" role="status" data-banner="uncommitted">
          <GitBranch size={16} aria-hidden="true" />
          <span>{t('projects.repositoryResources.banner.uncommitted', uncommitted.length)}</span>
          {onOpenGit && (
            <button type="button" className="rr-link" onClick={onOpenGit}>
              {t('projects.repositoryResources.banner.openGit')}
            </button>
          )}
        </div>
      )}
      {actionError && (
        <div className="project-repository-resources-banner" data-tone="error" role="alert">
          <AlertTriangle size={16} aria-hidden="true" />
          <span>{actionError}</span>
        </div>
      )}

      {attention.length > 0 && (
        <section className="rr-attention" aria-label={t('projects.repositoryResources.attention.title')} data-testid="repository-attention">
          <h3>
            {t('projects.repositoryResources.attention.title')} <span>{attention.length}</span>
          </h3>
          <ul>
            {visibleAttention.map(({ row, reason }) => (
              <li key={row.key} data-reason={reason}>
                <span>
                  {t(`projects.repositoryResources.attention.${row.attachOnly && row.state === 'repository_only' ? 'notAttached' : ATTENTION_SENTENCE[row.state]}`, row.name)}
                </span>
                <button
                  type="button"
                  className="rr-action"
                  data-action={row.primary}
                  disabled={busyKey !== null || (writesRepository(row.primary) && !canWrite)}
                  onClick={() => startAction(row)}
                >
                  {t(`projects.repositoryResources.action.${row.primary}`)}
                </button>
              </li>
            ))}
          </ul>
          {attention.length > ATTENTION_VISIBLE && (
            <button type="button" className="rr-link" onClick={() => setAttentionOpen(open => !open)}>
              {attentionOpen
                ? t('projects.repositoryResources.attention.less')
                : t('projects.repositoryResources.attention.more', attention.length - ATTENTION_VISIBLE)}
            </button>
          )}
        </section>
      )}

      <div className="project-repository-resources-columns">
        <div className="project-repository-resources-list" role="tabpanel">
          {activeTab === 'skills' && (
            <div className="project-repository-skill-roots" data-testid="project-skill-roots">
              <strong>{t('projects.repositoryResources.skillRoots.title')}</strong>
              {nativeSkillRoots.length === 0 ? (
                <span>{t('projects.repositoryResources.skillRoots.none')}</span>
              ) : (
                <ul>
                  {nativeSkillRoots.map(root => (
                    <li key={root.path}>
                      <code>{root.path}/</code>
                      <span>{t('projects.repositoryResources.skillRoots.count', root.skill_count)}</span>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}

          <div className="rr-toolbar">
            <label className="rr-search">
              <Search size={14} aria-hidden="true" />
              <input
                type="search"
                value={query}
                placeholder={t('projects.repositoryResources.search')}
                aria-label={t('projects.repositoryResources.search')}
                onChange={event => setQuery(event.target.value)}
              />
            </label>
            <div className="rr-filters" role="group" aria-label={t('projects.repositoryResources.filters')}>
              {FILTERS.map(option => (
                <button
                  key={option}
                  type="button"
                  className="rr-chip"
                  aria-pressed={filter === option}
                  onClick={() => setFilter(option)}
                >
                  {t(`projects.repositoryResources.filter.${option}`)}
                  <span>{searched.filter(row => matchesPresence(row, option)).length}</span>
                </button>
              ))}
            </div>
            <div className="rr-bulk">
              {selectedForPublication.length > 0 && (
                <button
                  type="button"
                  className="rr-button"
                  disabled={busyKey !== null || !canWrite}
                  title={canWrite ? undefined : t('projects.repositoryResources.banner.writeDisabled.title')}
                  onClick={() => setTransfer({ kind: 'publish_selected', rows: selectedForPublication })}
                >
                  {busyKey?.startsWith('publish_selected') && <Loader2 size={14} className="animate-spin" aria-hidden="true" />}
                  {t('projects.repositoryResources.publishSelected', selectedForPublication.length)}
                </button>
              )}
              {lineCount > 0 && (
                <button
                  type="button"
                  className="rr-button"
                  disabled={busyKey !== null}
                  onClick={() => setAlignOpen(true)}
                >
                  <ArrowRightLeft size={14} aria-hidden="true" />
                  {t('projects.repositoryResources.alignAll', lineCount)}
                </button>
              )}
            </div>
          </div>

          {tabRows.length === 0 ? (
            <p>{t(emptyKey)}</p>
          ) : visibleRows.length === 0 ? (
            <p>{t('projects.repositoryResources.emptyFiltered')}</p>
          ) : (
            <div className="rr-table" role="table" aria-label={t(`projects.repositoryResources.tab.${activeTab}`)}>
              <div className="rr-head" role="row">
                <span role="columnheader" aria-hidden="true" />
                <span role="columnheader">{t('projects.repositoryResources.columns.repository')}</span>
                <span role="columnheader">{t('projects.repositoryResources.columns.sync')}</span>
                <span role="columnheader">{t('projects.repositoryResources.columns.kronn')}</span>
                <span role="columnheader">{t('projects.repositoryResources.columns.action')}</span>
              </div>
              {sections.filter(section => section.rows.length > 0).map(section => (
                <div key={section.id} role="rowgroup" aria-label={section.title} data-resource-kind={section.kind}>
                  {(activeTab !== 'artifacts') && (
                    <h3 role="presentation">{section.title} <span>{section.rows.length}</span></h3>
                  )}
                  {section.rows.map(row => (
                    <RepositoryResourceRow
                      key={row.key}
                      row={row}
                      checked={selected.has(row.key)}
                      busy={busyKey?.endsWith(row.key) ?? false}
                      canWrite={canWrite}
                      onToggle={() => toggleSelected(row.key)}
                      onOpen={() => setSheet({ type: 'compare', key: row.key })}
                      onPrimary={() => startAction(row)}
                      onMenu={action => onMenu(row, action)}
                    />
                  ))}
                </div>
              ))}
            </div>
          )}
        </div>

        <details className="project-repository-resources-tree" data-testid="repository-preview">
          <summary>{t('projects.repositoryResources.preview')}</summary>
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
        </details>
      </div>

      {transfer && (
        <RepositoryResourceTransfer
          plan={transfer}
          kronnExists={kronnExists}
          busy={busyKey !== null}
          onConfirm={nativePath => { void confirmTransfer(transfer, nativePath); }}
          onCancel={() => setTransfer(null)}
        />
      )}
      {sheet?.type === 'compare' && sheetRow && (
        <RepositoryResourceCompare
          row={sheetRow}
          canWrite={canWrite}
          busy={busyKey !== null}
          onKeepRepository={() => { void keepSide(sheetRow, 'repository'); }}
          onKeepKronn={() => { void keepSide(sheetRow, 'kronn'); }}
          onRefresh={() => { void refresh(projectId); }}
          onPrimary={() => { setSheet(null); startAction(sheetRow); }}
          onClose={() => setSheet(null)}
        />
      )}
      {sheet?.type === 'approve' && sheetRow && (
        <RepositoryResourceApprove
          row={sheetRow}
          busy={busyKey !== null}
          onApprove={() => { void approve(sheetRow); }}
          onReject={() => { void reject(sheetRow); }}
          onAddKey={onAddKey}
          onClose={() => setSheet(null)}
        />
      )}
      {alignOpen && (
        <RepositoryResourceAlign
          lines={allLines}
          initiallyChecked={new Set(allLines
            .filter(line => line.direction === 'to_kronn' || selected.has(line.row.key))
            .map(line => line.row.key))}
          excludedCount={excludedCount}
          canWrite={canWrite}
          kronnExists={kronnExists}
          busy={busyKey !== null}
          onConfirm={lines => { void alignAll(lines); }}
          onCancel={() => setAlignOpen(false)}
        />
      )}
    </section>
  );
}

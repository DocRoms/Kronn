import { useMemo, type ReactNode } from 'react';
import { ChevronRight, Folder } from 'lucide-react';
import { getProjectGroup } from '../lib/constants';
import type { Project } from '../types/generated';

export interface CollectionProjectTreeProps<TItem> {
  /** Folders available for grouping — pre-filtered by the caller (e.g. a
   *  hidden-path project is excluded before it ever reaches this component). */
  projects: Project[];
  /** Every item that belongs somewhere in the tree, already narrowed by the
   *  caller's own filters (search, source filter…). */
  items: TItem[];
  /** `null` groups an item under the "no project" bucket, rendered first. */
  getProjectId: (item: TItem) => string | null;
  /** Marks the item whose containing project folder must stay open
   *  regardless of the persisted collapse state — e.g. the active row. */
  isItemActive: (item: TItem) => boolean;
  collapsedGroups: ReadonlySet<string>;
  onToggleGroup: (key: string) => void;
  /** Per-group badge (e.g. unread count). A group without an entry, or at
   *  zero, renders no pastille. Keys: `noProjectGroupKey`, `org::<name>`,
   *  and each project's id. */
  unseenByGroup?: ReadonlyMap<string, number>;
  /** Optional semantic status marker rendered beside a group count. */
  renderGroupStatus?: (groupKey: string) => ReactNode;
  /** Renders the rows for one group — `project` is `null` for the
   *  "no project" bucket. */
  renderGroup: (context: { project: Project | null; items: TItem[] }) => ReactNode;
  labels: {
    noProject: string;
    /** Also used as the "Local" organisation label — sorted last. */
    local: string;
  };
  /** Icon shown on the "no project" bucket's header. Project folders always
   *  use the same folder icon as the rest of the app. */
  noProjectIcon: ReactNode;
  /** Group key for the "no project" bucket — callers keep their own
   *  collapse-state convention (Discussions reuses `'__global__'`). */
  noProjectGroupKey?: string;
  /** Collection pages may keep empty project folders visible as navigation
   * targets even before an item is assigned. Discussions keeps the default. */
  showEmptyProjects?: boolean;
  showEmptyNoProject?: boolean;
  /** Optional project selection layered onto the shared collapse behavior. */
  selectedProjectId?: string | null;
  onSelectProject?: (projectId: string | null) => void;
}

const DEFAULT_NO_PROJECT_KEY = '__global__';

/**
 * Shared "grouped by project" tree: the no-project bucket first, then
 * organisations (color from the org name, "Local" last), then each
 * project's collapsible folder with a count and an optional unseen
 * pastille. Row content is fully delegated to `renderGroup` so campaigns,
 * batches, execution children, or a flat list can all reuse the same
 * skeleton.
 */
export function CollectionProjectTree<TItem>({
  projects, items, getProjectId, isItemActive, collapsedGroups, onToggleGroup,
  unseenByGroup, renderGroupStatus, renderGroup, labels, noProjectIcon, noProjectGroupKey = DEFAULT_NO_PROJECT_KEY,
  showEmptyProjects = false, showEmptyNoProject = false, selectedProjectId, onSelectProject,
}: CollectionProjectTreeProps<TItem>) {
  const itemsByProjectId = useMemo(() => {
    const map = new Map<string | null, TItem[]>();
    for (const item of items) {
      const key = getProjectId(item);
      const list = map.get(key);
      if (list) list.push(item);
      else map.set(key, [item]);
    }
    return map;
  }, [items, getProjectId]);

  const noProjectItems = itemsByProjectId.get(null) ?? [];
  const visibleProjects = useMemo(
    () => showEmptyProjects
      ? projects
      : projects.filter(project => (itemsByProjectId.get(project.id) ?? []).length > 0),
    [projects, itemsByProjectId, showEmptyProjects],
  );

  const localLabel = labels.local;
  const orgGroups = useMemo(() => {
    const map = new Map<string, Project[]>();
    for (const project of visibleProjects) {
      const org = getProjectGroup(project, localLabel, localLabel);
      const list = map.get(org);
      if (list) list.push(project);
      else map.set(org, [project]);
    }
    return [...map.entries()].sort(([a], [b]) => {
      if (a === localLabel) return 1;
      if (b === localLabel) return -1;
      return a.localeCompare(b);
    });
  }, [visibleProjects, localLabel]);

  const unseenFor = (key: string) => unseenByGroup?.get(key) ?? 0;

  return (
    <>
      {(showEmptyNoProject || noProjectItems.length > 0) && (
        <div>
          <button
            type="button"
            className="disc-group-btn"
            data-no-border="true"
            data-selected={selectedProjectId === null}
            aria-current={selectedProjectId === null ? 'page' : undefined}
            onClick={() => {
              onSelectProject?.(null);
              onToggleGroup(noProjectGroupKey);
            }}
            aria-expanded={!collapsedGroups.has(noProjectGroupKey)}
          >
            <ChevronRight size={10} className="disc-chevron" data-expanded={!collapsedGroups.has(noProjectGroupKey)} />
            {noProjectIcon} {labels.noProject}
            <span className="disc-group-count">{noProjectItems.length}</span>
            {renderGroupStatus?.(noProjectGroupKey)}
            {unseenFor(noProjectGroupKey) > 0 && (
              <span className="disc-group-unseen">{unseenFor(noProjectGroupKey)}</span>
            )}
          </button>
          {!collapsedGroups.has(noProjectGroupKey) && renderGroup({ project: null, items: noProjectItems })}
        </div>
      )}
      {orgGroups.map(([orgName, orgProjects]) => {
        const orgKey = `org::${orgName}`;
        const isOrgCollapsed = collapsedGroups.has(orgKey);
        const orgCount = orgProjects.reduce((sum, project) => sum + (itemsByProjectId.get(project.id)?.length ?? 0), 0);
        const orgColor = orgName === labels.local
          ? 'var(--kr-text-dim)'
          : `hsl(${[...orgName].reduce((h, c) => (h * 31 + c.charCodeAt(0)) % 360, 0)}, 50%, 60%)`;
        return (
          <div key={orgKey}>
            {orgGroups.length > 1 && (
              <button
                className="disc-org-header"
                style={{ color: orgColor }}
                onClick={() => onToggleGroup(orgKey)}
                aria-expanded={!isOrgCollapsed}
              >
                <ChevronRight size={9} className="disc-chevron" data-expanded={!isOrgCollapsed} />
                {orgName}
                <span className="disc-group-count">{orgCount}</span>
                {renderGroupStatus?.(orgKey)}
                {unseenFor(orgKey) > 0 && <span className="disc-group-unseen">{unseenFor(orgKey)}</span>}
              </button>
            )}
            {!isOrgCollapsed && orgProjects.map(project => {
              const projectItems = itemsByProjectId.get(project.id) ?? [];
              const containsActive = projectItems.some(isItemActive);
              const isCollapsed = collapsedGroups.has(project.id) && !containsActive;
              return (
                <div key={project.id}>
                  <button
                    className="disc-group-btn"
                    data-selected={selectedProjectId === project.id}
                    aria-current={selectedProjectId === project.id ? 'page' : undefined}
                    onClick={() => {
                      onSelectProject?.(project.id);
                      onToggleGroup(project.id);
                    }}
                    aria-expanded={!isCollapsed}
                  >
                    <ChevronRight size={10} className="disc-chevron" data-expanded={!isCollapsed} />
                    <Folder size={10} /> {project.name}
                    <span className="disc-group-count">{projectItems.length}</span>
                    {renderGroupStatus?.(project.id)}
                    {unseenFor(project.id) > 0 && <span className="disc-group-unseen">{unseenFor(project.id)}</span>}
                  </button>
                  {!isCollapsed && renderGroup({ project, items: projectItems })}
                </div>
              );
            })}
          </div>
        );
      })}
    </>
  );
}

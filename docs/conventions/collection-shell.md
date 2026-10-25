# Collection shell accessibility and persistence contract

Projects, Discussions, Plugins, Pages, Planning, and Automations compose the
shared collection shell, while retaining their domain data and detail actions.
[src: file: frontend/src/components/ProjectList.tsx:313-335#CollectionShell]
[src: file: frontend/src/components/DiscussionSidebar.tsx:1145-1160#CollectionShell]
[src: file: frontend/src/components/plugins/PluginCollectionView.tsx:31-53]
[src: file: frontend/src/pages/PagesPage.tsx:819-831#CollectionShell]
[src: file: frontend/src/pages/PlanningPage.tsx:336-375#CollectionShell]
[src: file: frontend/src/pages/WorkflowsPage.tsx:2379-2399#CollectionShell]

## Shared interaction contract

| Concern | Rule and source |
| --- | --- |
| Keyboard and focus | `/` targets search outside editable controls; arrows, Home, and End move between enabled visible rows. [src: file: frontend/src/components/CollectionShell.tsx:353-363#handleGlobalSearchShortcut] [src: file: frontend/src/components/CollectionShell.tsx:390-412#onSidebarKeyDown] |
| Accessible state | The sidebar and search have labels; the search declares `/`, selected rows expose `aria-current`, and the default multi-select control is a checkbox. [src: file: frontend/src/components/CollectionShell.tsx:451-452#sidebar] [src: file: frontend/src/components/CollectionShell.tsx:502-507#aria-keyshortcuts] [src: file: frontend/src/components/CollectionShell.tsx:435-440#getRowProps] [src: file: frontend/src/components/CollectionShell.tsx:524-526#aria-current] |
| Selection | Opt-in `selectedIds` are pruned against refreshed item identifiers. [src: file: frontend/src/components/CollectionShell.tsx:274-279#pruneCollectionSelection] |
| Dismissal | The action menu focuses its first enabled item; Escape and outside dismissal restore its trigger. Mobile Escape and item selection use the same collapse path, which restores focus to the opener. [src: file: frontend/src/components/CollectionShell.tsx:281-284#closeMenu] [src: file: frontend/src/components/CollectionShell.tsx:231-248#CollectionSidebarRail] [src: file: frontend/src/components/CollectionShell.tsx:335-351#closeMenu] [src: file: frontend/src/components/CollectionShell.tsx:365-374#closeMobileSidebar] [src: file: frontend/src/components/CollectionShell.tsx:376-379#selectItem] [src: file: frontend/src/components/CollectionShell.tsx:288-292#collapseSidebar] |
| Empty state | The default list renders the domain empty-state slot; a custom list renderer owns its equivalent state. [src: file: frontend/src/components/CollectionShell.tsx:519-531#renderEmpty] |

## Six-family surface matrix

| Surface | Keyboard / ARIA | Closure | Persistence | Selection |
| --- | --- | --- | --- | --- |
| Projects | Multi-select project rows use checkbox semantics and names. [src: file: frontend/src/components/ProjectList.tsx:530-538#toggleMultiSelection] | Shared menu/sidebar dismissal. [src: file: frontend/src/components/CollectionShell.tsx:335-351#closeMenu] | Favorites wait for `projectsLoaded`; collapsed sections are stored locally. [src: file: frontend/src/pages/Dashboard.tsx:116] [src: file: frontend/src/routes/ProjectsRoute.tsx:38] [src: file: frontend/src/components/ProjectList.tsx:127-138] | Local state; shell-pruned. [src: file: frontend/src/components/ProjectList.tsx:126-129] |
| Discussions | The shell receives selected identifiers and global keyboard search. [src: file: frontend/src/components/DiscussionSidebar.tsx:1157-1165#globalSearchShortcut] | Header and batch menus close on Escape and restore their triggers. [src: file: frontend/src/components/DiscussionSidebar.tsx:237-255#openBatchMenuRunId] | Search and grouping are domain-owned; favorites are not supplied to the shell. [src: file: frontend/src/components/DiscussionSidebar.tsx:1150-1156#persistence] | Local state; shell-pruned. [src: file: frontend/src/components/DiscussionSidebar.tsx:229#selectedIds] [src: file: frontend/src/components/DiscussionSidebar.tsx:1159-1160#onSelectedIdsChange] |
| Plugins | The add-plugin panel is a modal dialog. [src: file: frontend/src/components/plugins/AddPluginModal.tsx:116-123] | Its close button/backdrop invoke the page reset path; shared menu/sidebar dismissal also applies. [src: file: frontend/src/components/plugins/AddPluginModal.tsx:108-131] [src: file: frontend/src/components/CollectionShell.tsx:335-351#closeMenu] | Favorites wait for a non-null MCP overview; sort and collapsed groups are local. [src: file: frontend/src/pages/Dashboard.tsx:644] [src: file: frontend/src/routes/PluginsRoute.tsx:28#overviewLoaded] [src: file: frontend/src/components/plugins/usePluginListState.ts:47-73] | Local state; shell-pruned. [src: file: frontend/src/components/plugins/usePluginListState.ts:65#selectedConfigIds] |
| Pages | In selection mode, custom rows are named checkboxes using the select action, not the open action. [src: file: frontend/src/pages/PagesPage.tsx:948-954#toggleMultiSelection] | Shared mobile/sidebar dismissal; page-specific dialogs own their closure. [src: file: frontend/src/components/CollectionShell.tsx:365-379#closeMobileSidebar] | Search and validated collapsed sections are local. [src: file: frontend/src/pages/PagesPage.tsx:97-105#readCollapsedPageSections] [src: file: frontend/src/pages/PagesPage.tsx:282-288#PAGE_COLLAPSED_STORAGE_KEY] | Local state; shell-pruned. [src: file: frontend/src/pages/PagesPage.tsx:205#selectedIds] [src: file: frontend/src/pages/PagesPage.tsx:831-832#onSelectedIdsChange] |
| Planning | The shell receives task labels, selection and selection callbacks. [src: file: frontend/src/pages/PlanningPage.tsx:363-375#getLabel] | The create panel is modal and routes keyboard events to its handler. [src: file: frontend/src/pages/PlanningPage.tsx:699-707#handleCreateModalKeyDown] | Favorites wait for the first successful list response, but are never pruned from a filtered or paginated response; collapsed sections are local. [src: file: frontend/src/pages/PlanningPage.tsx:142-149#usePersistentIdSet] [src: file: frontend/src/pages/PlanningPage.tsx:175-195#fetchTasks] [src: file: frontend/src/pages/PlanningPage.tsx:153-159#PLANNING_COLLAPSED_STORAGE_KEY] | Local state; shell-pruned. [src: file: frontend/src/pages/PlanningPage.tsx:135#selectedIds] [src: file: frontend/src/pages/PlanningPage.tsx:374-375#onSelectedIdsChange] |
| Automations | The shell receives its active resource selection. [src: file: frontend/src/pages/WorkflowsPage.tsx:2439-2447#selectedId] | The actions panel closes on Escape and is a modal dialog. [src: file: frontend/src/pages/WorkflowsPage.tsx:652-659#closeOnEscape] [src: file: frontend/src/pages/WorkflowsPage.tsx:2667-2673#aria-modal] | Navigation and valid collapsed-section state are restored locally. [src: file: frontend/src/pages/WorkflowsPage.tsx:257-267#readCollapsedAutomationSections] [src: file: frontend/src/lib/automationNavigation.ts:20-33#readAutomationLastVisit] | Local `automationSelection` state, shell-pruned; the bulk delete keeps the selection when a deletion fails. [src: file: frontend/src/pages/WorkflowsPage.tsx:1598#automationSelection] [src: file: frontend/src/pages/WorkflowsPage.tsx:2411-2412#onSelectedIdsChange] [src: file: frontend/src/pages/WorkflowsPage.tsx:2400-2432#deleteAutomationResource] |

## Visual review matrix

The review matrix is executed through each surface's rendered component or
page suite. It does not use a source-text inspection or a generic
`CollectionShell` fixture as adapter evidence.

| Surface | Desktop and narrow viewport | Empty state | Selection | Open menu |
| --- | --- | --- | --- | --- |
| Discussions | Desktop collapse: [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:72-78]. Narrow sidebar and close path: [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:81-96]. | Narrow empty list: [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:84-89]. | Narrow bulk selection: [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:91-96]. | Narrow header action disclosure: [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:91-94]. |
| Automations | Desktop collapse: [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:275-285]. Narrow rail: [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:247-273]. | Narrow empty resources: [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:247-250]. | Narrow selected workflow: [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:258-266]. | Narrow action dialog: [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:269-270]. |
| Pages | Desktop collapse: [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:323-334]. Narrow rail: [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:428-432]. | Narrow empty list: [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:419-422]. | Narrow checkbox selection: [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:433-436]. | Narrow mosaic menu: [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:437-438]. |
| Projects | Desktop sidebar: [src: file: frontend/src/components/__tests__/ProjectList.missing-path.test.tsx:84-94]. Narrow rail: [src: file: frontend/src/components/__tests__/ProjectList.missing-path.test.tsx:113-135]. | Narrow empty renderer: [src: file: frontend/src/components/__tests__/ProjectList.missing-path.test.tsx:113-116]. | Narrow checkbox selection: [src: file: frontend/src/components/__tests__/ProjectList.missing-path.test.tsx:132-134]. | Narrow shared title menu: [src: file: frontend/src/components/__tests__/ProjectList.missing-path.test.tsx:127-128]. |
| Planning | Desktop sidebar: [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:116-128]. Narrow rail: [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:136-156]. | Narrow empty backlog: [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:136-141]. | Narrow checkbox selection: [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:153-155]. | Narrow shared title menu: [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:148-149]. |
| Plugins | Desktop selected detail: [src: file: frontend/src/pages/__tests__/McpPage.test.tsx:786-793]. Narrow rail: [src: file: frontend/src/pages/__tests__/McpPage.test.tsx:329-350]. | Narrow empty overview: [src: file: frontend/src/pages/__tests__/McpPage.test.tsx:331-335]. | Narrow checkbox selection: [src: file: frontend/src/pages/__tests__/McpPage.test.tsx:347-349]. | Narrow shared title menu: [src: file: frontend/src/pages/__tests__/McpPage.test.tsx:342-343]. |

## KT-495 closure evidence chain

The KT-495 closure review has six retained evidence lots. The lot ordering and
the principal's responsibility for the atomic epic closure are supplied by the
task owner; this document records the repository evidence and does not mutate
the Planning record. [src: user: 2026-08-30: KT-508 generation 2 reassignment]

| KT-495 DoD | Linked child lots and repository evidence retained for principal review |
| --- | --- |
| 1. Responsive master/detail shell across the six surfaces | **KT-504 + KT-505 + KT-506/KT-523 + KT-508**. The shared shell owns selection, sidebar open/close, and mobile collapse; the six real-surface scenarios are listed in the executable matrix. [src: user: 2026-08-30: KT-508 generation 3 reassignment] [src: file: frontend/src/components/CollectionShell.tsx:274-279#pruneCollectionSelection] [src: file: frontend/src/components/CollectionShell.tsx:288-297#collapseSidebar] [src: file: frontend/src/components/CollectionShell.tsx:365-379#closeMobileSidebar] [src: file: docs/conventions/collection-shell.md:39-46] |
| 2. Search, filters, favorites, selection, shortcuts, and persistence conventions | **KT-507 + KT-508**. The shell implements search and row keyboard navigation; the persistence and integration matrices retain the cross-family evidence. [src: user: 2026-08-30: KT-508 generation 3 reassignment] [src: file: frontend/src/components/CollectionShell.tsx:353-363#handleGlobalSearchShortcut] [src: file: frontend/src/components/CollectionShell.tsx:390-412#onSidebarKeyDown] [src: file: docs/conventions/collection-shell.md:152-187] |
| 3. Shared components, menu, focus, and closure | **KT-504 + KT-507**. The shell opens and closes its action menu, restores its trigger focus, and applies the mobile Escape closure path. [src: user: 2026-08-30: KT-508 generation 3 reassignment] [src: file: frontend/src/components/CollectionShell.tsx:281-284#closeMenu] [src: file: frontend/src/components/CollectionShell.tsx:335-351#closeMenu] [src: file: frontend/src/components/CollectionShell.tsx:365-374#closeMobileSidebar] |
| 4. Business slots and surface-specific behavior for Projects, Planning, and Plugins | **KT-506/KT-523**. Each surface supplies its own state and data to the common shell: Projects, Planning, and Plugins retain independent selected-id state. [src: user: 2026-08-30: KT-508 generation 3 reassignment] [src: file: frontend/src/components/ProjectList.tsx:313-335#CollectionShell] [src: file: frontend/src/pages/PlanningPage.tsx:336-375#CollectionShell] [src: file: frontend/src/components/plugins/usePluginListState.ts:65-67#selectedConfigIds] |
| 5. Keyboard, focus, accessible labels, and tests for all six families | **KT-507 + KT-508**. Keyboard handling is in the shared shell and every family has a rendered narrow-viewport regression in the visual matrix. [src: user: 2026-08-30: KT-508 generation 3 reassignment] [src: file: frontend/src/components/CollectionShell.tsx:390-412#onSidebarKeyDown] [src: file: frontend/src/components/CollectionShell.tsx:451-452#sidebar] [src: file: frontend/src/components/CollectionShell.tsx:502-507#aria-keyshortcuts] [src: file: docs/conventions/collection-shell.md:39-46] |
| 6. Desktop and responsive visual parity | **KT-508**. The matrix cites the desktop behavior and the narrow rail/sidebar, empty, selection, and open-menu assertions for Discussions, Automations, Pages, Projects, Planning, and Plugins. [src: user: 2026-08-30: KT-508 generation 3 reassignment] [src: file: docs/conventions/collection-shell.md:39-46] |

The six child lots are KT-504, KT-505, KT-506, KT-523, KT-507, and KT-508.
The principal reviews the six rows above and performs the requested atomic epic
closure after integration.
[src: user: 2026-08-30: KT-508 generation 2 reassignment]

## Automation sidebar (KT-904, moved to the sidebar by KT-912, regrouped by KT-916)

The global Automation page puts its search, "Group by" and filters in the
sidebar, from top to bottom: the title row (`Automatisation · N`, the `+`, help,
the ⋯ menu and the collapse button), the search, a "Group by" control, the
filter chips, then the list. The shell draws the title row and the search field
(`globalSearchShortcut`, `showSearchClear`); the page fills three slots:
`afterSidebarHeader` for the control and the chips, `moreActionsMenuExtra` for
the sort, and `renderList` for the groups. The search has the row to itself: the
old Filter and Sort icons, the panel of three `<select>` (KT-912) and
`AutomationToolbar` are gone, as `AutomationFilterBar` was before them. Nothing
sits above the list: the main column is the viewer only.
[src: file: frontend/src/pages/WorkflowsPage.tsx:2454-2455#globalSearchShortcut]
[src: file: frontend/src/pages/WorkflowsPage.tsx:2494-2506#afterSidebarHeader]
[src: file: frontend/src/components/AutomationSidebarControls.tsx:43-47#AutomationSidebarControls]

One filter set narrows the list (`itemFilter`, the search being applied by the
shell). `/` focuses the search from anywhere on the page, sidebar included. The
pure rules stay in `lib/automationFilters.ts`.
[src: file: frontend/src/pages/WorkflowsPage.tsx:2400-2401#itemFilter]
[src: file: frontend/src/lib/automationFilters.ts:97-108#matchesAutomationFilters]

"Group by" is a segmented control, **Type / Project / None**, Type by default,
remembered per user in this browser (`kronn:automationGroupBy`, every access
guarded by try/catch). The groups come from `groupAutomations`; each header has
a dot, the name, the count and a fold, and the rows under it are 44 px tall (name,
then trigger, steps and the state of the last run for a workflow). A group's
fold is kept like the old sections (`kronn:automationCollapsedSections`, keys
namespaced by grouping: `kind:<type>`, `project:<id>`); a search and a multiple
selection keep it open, but the group of the open automation folds like the
others.
[src: file: frontend/src/lib/automationFilters.ts:170-210#groupAutomations]
[src: file: frontend/src/lib/automationSidebarPrefs.ts:12-32#AUTOMATION_GROUP_BY_STORAGE_KEY]
[src: file: frontend/src/pages/WorkflowsPage.tsx:257-267#readCollapsedAutomationSections]
[src: file: frontend/src/pages/WorkflowsPage.tsx:2585-2590#isGroupCollapsed]

| Grouping | Rule |
| --- | --- |
| Type | One group per type, in the order Workflows / Quick Prompts / Quick APIs / Quick Execs / Skills, empty ones left out. The row no longer repeats its type. [src: file: frontend/src/lib/automationFilters.ts:12-13#AUTOMATION_KIND_FILTERS] [src: file: frontend/src/lib/automationFilters.ts:191-198#AUTOMATION_KIND_FILTERS] |
| Project | One group per project (by name), then "No project". A skill is attached to every project that lists it in `default_skill_ids`, so it sits under each of them (KT-914). Automations whose only projects are hidden from the page are not listed, as in the old project tree. [src: file: frontend/src/lib/automationSkills.ts:17-21#projectsUsingSkill] [src: file: frontend/src/lib/automationFilters.ts:199-209#automationProjectIds] |
| None | One flat list, in the sort order. The row names its type again. |

The filters are chips on a line that wraps:

| Chip | Rule |
| --- | --- |
| Type | Reads "All" until a type is picked. A click opens the list of types (All, then the five types), each with its count; choosing one also opens its list in the main column, "All" lifts the filter. It carries the tour anchor `automation-filter-type` (and `data-value`, the chosen type), which the four automation type steps of the tour target: they open the list and pick their type. [src: file: frontend/src/components/AutomationSidebarControls.tsx:143-157#kindChipRef] [src: file: frontend/src/pages/WorkflowsPage.tsx:2357-2360#selectAutomationKindFilter] [src: file: frontend/src/components/tour/tourSteps.ts:256-271#openAutomationKind] |
| Pinned, Active, Recent | Independent toggles that stack. "Active" leaves out only a disabled workflow: a Quick Prompt, API, Exec or Skill has no off switch, so it always counts as active. "Recent" keeps what was opened lately and orders it by last opening, favorites not pulled up. [src: file: frontend/src/lib/automationFilters.ts:74-76#matchesAutomationActive] [src: file: frontend/src/lib/automationSort.ts:66-95#sortAutomationResources] |
| Project | "+ Project" opens the list of the projects that hold something (and "No project"), each with its count; the choice shows as a removable chip `name ×`, whose name reopens the list. [src: file: frontend/src/lib/automationFilters.ts:138-156#countAutomationProjects] |
| Counts | Every count reads what choosing it would show given the other filters and the search. [src: file: frontend/src/lib/automationFilters.ts:123-136#countAutomationKinds] |
| Clear | "Clear filters" shows only while a chip is set (a type other than "All" counts), resets every chip and leaves the search alone (it has its own ✕). The empty list offers it too. [src: file: frontend/src/lib/automationFilters.ts:110-121#activeAutomationFilterCount] [src: file: frontend/src/pages/WorkflowsPage.tsx:2364-2370#clearAutomationFilters] |

The "Recent" history is this browser's (`kronn:automationLastOpened`): opening
an automation from the sidebar stamps it, and only the latest 20 are kept.
[src: file: frontend/src/lib/automationSidebarPrefs.ts:59-73#withAutomationOpened]

The sort is set from the header's ⋯ menu, above "Select multiple": Name, Last
modified, Last opened (`menuitemradio`) and a reverse toggle (`menuitemcheckbox`),
which the shell's arrow keys walk like any menu item. Favorites stay first
whatever the criterion; the direction only flips each side of that line.
[src: file: frontend/src/components/AutomationSidebarControls.tsx:272-307#AutomationSortMenuItems]
[src: file: frontend/src/lib/automationSort.ts:66-95#sortAutomationResources]
[src: file: frontend/src/components/CollectionShell.tsx:414-426#onMenuKeyDown]

Skills (KT-914) are read from the catalog, not created here: the row opens a
read-only sheet (`SkillSheet`) whose `SKILL.md` reuses the safe Markdown of the
repository resources, and editing stays in Config › Skills. A skill has no
server-side pin or modification date, so its favorites are kept in the browser
and it ranks after every dated automation when sorted by last modification.
[src: file: frontend/src/lib/automationSkills.ts:9-13#SKILL_UPDATED_AT] [src: file: frontend/src/lib/automationSkills.ts:23-33#readSkillFavorites] [src: file: frontend/src/components/SkillSheet.tsx:136-139#SkillSheet] [src: file: frontend/src/components/SkillSheet.tsx:217-219#FileText]

At 400 px, where the sidebar is the usual drawer, nothing scrolls sideways: the
chips wrap onto new lines, "Group by" takes the full width, and a list opens over
the width of the controls instead of hanging off its chip. There is no "Filters
(n)" fold on this page any more (`FilterFold` remains for the project tab below).

The project tab's Automation sub-tab (`ProjectRepositoryResourcesPanel`) is
unchanged: it keeps the fold and the tab's own `rr-chip` for a type filter, All
/ QP / QA / QE / Workflow, that stacks with the location filter and the search.
[src: file: frontend/src/lib/repositoryResourceRows.ts:341-349#AutomationTypeFilter]
[src: file: frontend/src/components/ProjectRepositoryResourcesPanel.tsx:396-400#matchesAutomationType]
[src: file: frontend/src/components/ProjectRepositoryResourcesPanel.tsx:592-625#FilterFold]

## Favorite restoration rule

`usePersistentIdSet` restores saved ids but neither writes nor prunes before its
caller reports readiness. A caller that supplies readiness only after an
authoritative successful response, together with a
complete authoritative identifier set enables stale-id pruning. A caller backed
by a filtered or paginated response disables that pruning, because an omitted
item is not evidence that its saved favorite is stale. Projects use their loaded
state, Plugins require a non-null overview, and Planning keeps favorites through
every partial task response.
[src: file: frontend/src/hooks/usePersistentIdSet.ts:13-40#usePersistentIdSet]
[src: file: frontend/src/hooks/useApi.ts:9-10#hasLoaded]
[src: file: frontend/src/hooks/useApi.ts:38-44#setHasLoaded]
[src: file: frontend/src/pages/Dashboard.tsx:116#projectsLoaded]
[src: file: frontend/src/routes/ProjectsRoute.tsx:38#favoritesReady]
[src: file: frontend/src/pages/Dashboard.tsx:644#mcpOverviewLoaded]
[src: file: frontend/src/routes/PluginsRoute.tsx:28#overviewLoaded]
[src: file: frontend/src/pages/PlanningPage.tsx:142-149#usePersistentIdSet]

## Executable regression matrix

The shared shell suite exercises the reusable keyboard, checkbox, browser
storage, filter-reset, and Escape-focus contract. Each family then exercises
its own integration rather than six aliases of a generic fixture:

| Surface | Integration coverage |
| --- | --- |
| Projects | Favorite/filter persistence, multi-selection, deletion, and sidebar collapse. [src: file: frontend/src/components/__tests__/ProjectList.missing-path.test.tsx:254-283] |
| Discussions | Real discussion bulk-selection, batch action, and Escape-trigger restoration. [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:150-172] [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:215-229] |
| Plugins | Favorite persistence, checkbox selection, deletion, collapse, and ready-state restore. [src: file: frontend/src/pages/__tests__/McpPage.test.tsx:352-388] [src: file: frontend/src/pages/__tests__/McpPage.test.tsx:390-407] |
| Pages | Search, pinning, custom checkbox rows, archive selection, and keyboard navigation. [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:874-901] [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:408-416] [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:973-991] |
| Planning | Local favorite restore survives an excluded filtered, paginated list and remount; the page separately exercises bulk archive and keyboard navigation. [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:158-188] [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:246-265] [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:287-306] |
| Automations | Real workflow/quick-resource favorites, sidebar collapse, and action-dialog dismissal. [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:275-285] [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:301-314] [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:1147-1245] |

The hook suite covers restore-before-load and stale-id cleanup for authoritative
callers. [src: file: frontend/src/hooks/__tests__/usePersistentIdSet.test.tsx:5-25]

## Contrast scope

Preformatted descendants using the reusable `text-dim` utility are promoted to
the primary text token. [src: file: frontend/src/styles/utilities.css:50-58]

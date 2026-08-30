# Collection shell accessibility and persistence contract

Projects, Discussions, Plugins, Pages, Planning, and Automations compose the
shared collection shell, while retaining their domain data and detail actions.
[src: file: frontend/src/components/ProjectList.tsx:110-121]
[src: file: frontend/src/components/DiscussionSidebar.tsx:761-785]
[src: file: frontend/src/pages/McpPage.tsx:299-303]
[src: file: frontend/src/pages/PagesPage.tsx:600-617]
[src: file: frontend/src/pages/PlanningPage.tsx:300-340]
[src: file: frontend/src/pages/WorkflowsPage.tsx:1821-1840]

## Shared interaction contract

| Concern | Rule and source |
| --- | --- |
| Keyboard and focus | `/` targets search outside editable controls; arrows, Home, and End move between enabled visible rows. [src: file: frontend/src/components/CollectionShell.tsx:326-336] [src: file: frontend/src/components/CollectionShell.tsx:362-419] |
| Accessible state | The sidebar and search have labels; the search declares `/`, selected rows expose `aria-current`, and the default multi-select control is a checkbox. [src: file: frontend/src/components/CollectionShell.tsx:423-485] |
| Selection | Opt-in `selectedIds` are pruned against refreshed item identifiers. [src: file: frontend/src/components/CollectionShell.tsx:242-252] |
| Dismissal | The action menu focuses its first enabled item; Escape and outside dismissal restore its trigger. Mobile Escape and item selection use the same collapse path, which restores focus to the opener. [src: file: frontend/src/components/CollectionShell.tsx:254-270] [src: file: frontend/src/components/CollectionShell.tsx:308-324] [src: file: frontend/src/components/CollectionShell.tsx:338-352] [src: file: frontend/src/components/CollectionShell.tsx:495-504] |
| Empty state | The default list renders the domain empty-state slot; a custom list renderer owns its equivalent state. [src: file: frontend/src/components/CollectionShell.tsx:478-490] |

## Six-family surface matrix

| Surface | Keyboard / ARIA | Closure | Persistence | Selection |
| --- | --- | --- | --- | --- |
| Projects | Multi-select project rows use checkbox semantics and names. [src: file: frontend/src/components/ProjectList.tsx:467-499] | Shared menu/sidebar dismissal. [src: file: frontend/src/components/CollectionShell.tsx:308-324] | Favorites wait for `projectsLoaded`; collapsed sections are stored locally. [src: file: frontend/src/pages/Dashboard.tsx:144] [src: file: frontend/src/pages/Dashboard.tsx:1322-1325] [src: file: frontend/src/components/ProjectList.tsx:118-129] | Local state; shell-pruned. [src: file: frontend/src/components/ProjectList.tsx:117-120] |
| Discussions | The shell receives selected identifiers and global keyboard search. [src: file: frontend/src/components/DiscussionSidebar.tsx:761-785] | Header and batch menus close on Escape and restore their triggers. [src: file: frontend/src/components/DiscussionSidebar.tsx:230-265] | Search and grouping are domain-owned; favorites are not supplied to the shell. [src: file: frontend/src/components/DiscussionSidebar.tsx:768-773] | Local state; shell-pruned. [src: file: frontend/src/components/DiscussionSidebar.tsx:215-218] [src: file: frontend/src/components/DiscussionSidebar.tsx:774-777] |
| Plugins | The add-plugin panel is a modal dialog. [src: file: frontend/src/pages/McpPage.tsx:1791-1815] | Its close button/backdrop invoke the page reset path; shared menu/sidebar dismissal also applies. [src: file: frontend/src/pages/McpPage.tsx:1792-1815] [src: file: frontend/src/components/CollectionShell.tsx:308-324] | Favorites wait for a non-null MCP overview; sort and collapsed groups are local. [src: file: frontend/src/pages/Dashboard.tsx:1383-1387] [src: file: frontend/src/pages/McpPage.tsx:290-312] | Local state; shell-pruned. [src: file: frontend/src/pages/McpPage.tsx:301-303] |
| Pages | In selection mode, custom rows are named checkboxes using the select action, not the open action. [src: file: frontend/src/pages/PagesPage.tsx:697-704] | Shared mobile/sidebar dismissal; page-specific dialogs own their closure. [src: file: frontend/src/components/CollectionShell.tsx:338-352] | Search and validated collapsed sections are local. [src: file: frontend/src/pages/PagesPage.tsx:151-154] | Local state; shell-pruned. [src: file: frontend/src/pages/PagesPage.tsx:600-617] |
| Planning | The shell receives task labels, selection and selection callbacks. [src: file: frontend/src/pages/PlanningPage.tsx:310-347] | The create panel is modal and routes keyboard events to its handler. [src: file: frontend/src/pages/PlanningPage.tsx:665-681] | Favorites wait for the first successful list response, but are never pruned from a filtered or paginated response; collapsed sections are local. [src: file: frontend/src/pages/PlanningPage.tsx:108-124] [src: file: frontend/src/pages/PlanningPage.tsx:151-168] | Local state; shell-pruned. [src: file: frontend/src/pages/PlanningPage.tsx:109-110] [src: file: frontend/src/pages/PlanningPage.tsx:332-334] |
| Automations | The shell receives its active resource selection. [src: file: frontend/src/pages/WorkflowsPage.tsx:1821-1840] | The actions panel closes on Escape and is a modal dialog. [src: file: frontend/src/pages/WorkflowsPage.tsx:407-414] [src: file: frontend/src/pages/WorkflowsPage.tsx:2170-2180] | Navigation and valid collapsed-section state are restored locally. [src: file: frontend/src/pages/WorkflowsPage.tsx:148-175] | No shell bulk-selection props are supplied. [src: file: frontend/src/pages/WorkflowsPage.tsx:1821-1840] |

## Visual review matrix

The review matrix is executed through each surface's rendered component or
page suite. It does not use a source-text inspection or a generic
`CollectionShell` fixture as adapter evidence.

| Surface | Desktop and narrow viewport | Empty state | Selection | Open menu |
| --- | --- | --- | --- | --- |
| Discussions | Desktop collapse: [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:72-78]. Narrow sidebar and close path: [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:81-96]. | Narrow empty list: [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:84-89]. | Narrow bulk selection: [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:91-96]. | Narrow header action disclosure: [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:91-94]. |
| Automations | Desktop collapse: [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:211-220]. Narrow rail: [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:189-208]. | Narrow empty resources: [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:189-192]. | Narrow selected workflow: [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:199-205]. | Narrow action dialog: [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:203-206]. |
| Pages | Desktop collapse: [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:134-144]. Narrow rail: [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:180-189]. | Narrow custom empty renderer: [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:169-173]. | Narrow checkbox selection: [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:184-187]. | Narrow mosaic menu: [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:188-189]. |
| Projects | Desktop sidebar: [src: file: frontend/src/components/__tests__/ProjectList.missing-path.test.tsx:80-90]. Narrow rail: [src: file: frontend/src/components/__tests__/ProjectList.missing-path.test.tsx:97-119]. | Narrow empty renderer: [src: file: frontend/src/components/__tests__/ProjectList.missing-path.test.tsx:97-100]. | Narrow checkbox selection: [src: file: frontend/src/components/__tests__/ProjectList.missing-path.test.tsx:116-118]. | Narrow shared title menu: [src: file: frontend/src/components/__tests__/ProjectList.missing-path.test.tsx:111-112]. |
| Planning | Desktop sidebar: [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:115-127]. Narrow rail: [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:135-154]. | Narrow empty backlog: [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:135-140]. | Narrow checkbox selection: [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:151-154]. | Narrow shared title menu: [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:147-148]. |
| Plugins | Desktop selected detail: [src: file: frontend/src/pages/__tests__/McpPage.test.tsx:183-193]. Narrow rail: [src: file: frontend/src/pages/__tests__/McpPage.test.tsx:196-216]. | Narrow empty overview: [src: file: frontend/src/pages/__tests__/McpPage.test.tsx:196-202]. | Narrow checkbox selection: [src: file: frontend/src/pages/__tests__/McpPage.test.tsx:213-216]. | Narrow shared title menu: [src: file: frontend/src/pages/__tests__/McpPage.test.tsx:209-210]. |

## KT-495 closure evidence chain

The KT-495 closure review has six retained evidence lots. The lot ordering and
the principal's responsibility for the atomic epic closure are supplied by the
task owner; this document records the repository evidence and does not mutate
the Planning record. [src: user: 2026-08-30: KT-508 generation 2 reassignment]

| KT-495 DoD | Linked child lots and repository evidence retained for principal review |
| --- | --- |
| 1. Responsive master/detail shell across the six surfaces | **KT-504 + KT-505 + KT-506/KT-523 + KT-508**. The shared shell owns selection, sidebar open/close, and mobile collapse; the six real-surface scenarios are listed in the executable matrix. [src: user: 2026-08-30: KT-508 generation 3 reassignment] [src: file: frontend/src/components/CollectionShell.tsx:242-270] [src: file: frontend/src/components/CollectionShell.tsx:338-352] [src: file: docs/conventions/collection-shell.md:39-46] |
| 2. Search, filters, favorites, selection, shortcuts, and persistence conventions | **KT-507 + KT-508**. The shell implements search and row keyboard navigation; the persistence and integration matrices retain the cross-family evidence. [src: user: 2026-08-30: KT-508 generation 3 reassignment] [src: file: frontend/src/components/CollectionShell.tsx:326-336] [src: file: frontend/src/components/CollectionShell.tsx:362-419] [src: file: docs/conventions/collection-shell.md:68-101] |
| 3. Shared components, menu, focus, and closure | **KT-504 + KT-507**. The shell opens and closes its action menu, restores its trigger focus, and applies the mobile Escape closure path. [src: user: 2026-08-30: KT-508 generation 3 reassignment] [src: file: frontend/src/components/CollectionShell.tsx:254-257] [src: file: frontend/src/components/CollectionShell.tsx:308-324] [src: file: frontend/src/components/CollectionShell.tsx:338-347] |
| 4. Business slots and surface-specific behavior for Projects, Planning, and Plugins | **KT-506/KT-523**. Each surface supplies its own state and data to the common shell: Projects, Planning, and Plugins retain independent selected-id state. [src: user: 2026-08-30: KT-508 generation 3 reassignment] [src: file: frontend/src/components/ProjectList.tsx:109-121] [src: file: frontend/src/pages/PlanningPage.tsx:307-348] [src: file: frontend/src/pages/McpPage.tsx:288-303] |
| 5. Keyboard, focus, accessible labels, and tests for all six families | **KT-507 + KT-508**. Keyboard handling is in the shared shell and every family has a rendered narrow-viewport regression in the visual matrix. [src: user: 2026-08-30: KT-508 generation 3 reassignment] [src: file: frontend/src/components/CollectionShell.tsx:362-419] [src: file: frontend/src/components/CollectionShell.tsx:423-485] [src: file: docs/conventions/collection-shell.md:39-46] |
| 6. Desktop and responsive visual parity | **KT-508**. The matrix cites the desktop behavior and the narrow rail/sidebar, empty, selection, and open-menu assertions for Discussions, Automations, Pages, Projects, Planning, and Plugins. [src: user: 2026-08-30: KT-508 generation 3 reassignment] [src: file: docs/conventions/collection-shell.md:39-46] |

The six child lots are KT-504, KT-505, KT-506, KT-523, KT-507, and KT-508.
The principal reviews the six rows above and performs the requested atomic epic
closure after integration.
[src: user: 2026-08-30: KT-508 generation 2 reassignment]

## Favorite restoration rule

`usePersistentIdSet` restores saved ids but neither writes nor prunes before its
caller reports readiness. A caller that supplies readiness only after an
authoritative successful response, together with a
complete authoritative identifier set enables stale-id pruning. A caller backed
by a filtered or paginated response disables that pruning, because an omitted
item is not evidence that its saved favorite is stale. Projects use their loaded
state, Plugins require a non-null overview, and Planning keeps favorites through
every partial task response.
[src: file: frontend/src/hooks/usePersistentIdSet.ts:19-43]
[src: file: frontend/src/hooks/useApi.ts:26-75]
[src: file: frontend/src/pages/Dashboard.tsx:144]
[src: file: frontend/src/pages/Dashboard.tsx:1322-1325]
[src: file: frontend/src/pages/Dashboard.tsx:1383-1387]
[src: file: frontend/src/pages/PlanningPage.tsx:116-124]

## Executable regression matrix

The shared shell suite exercises the reusable keyboard, checkbox, browser
storage, filter-reset, and Escape-focus contract. Each family then exercises
its own integration rather than six aliases of a generic fixture:

| Surface | Integration coverage |
| --- | --- |
| Projects | Favorite/filter persistence, multi-selection, deletion, and sidebar collapse. [src: file: frontend/src/components/__tests__/ProjectList.missing-path.test.tsx:155-181] |
| Discussions | Real discussion bulk-selection, batch action, and Escape-trigger restoration. [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:57-91] [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:168-182] |
| Plugins | Favorite persistence, checkbox selection, deletion, collapse, and ready-state restore. [src: file: frontend/src/pages/__tests__/McpPage.test.tsx:194-248] |
| Pages | Search, pinning, custom checkbox rows, archive selection, and keyboard navigation. [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:492-551] |
| Planning | Local favorite restore survives an excluded filtered, paginated list and remount; the page separately exercises bulk archive and keyboard navigation. [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:123-152] [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:208-267] |
| Automations | Real workflow/quick-resource favorites, sidebar collapse, and action-dialog dismissal. [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:182-218] [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:642-706] |

The hook suite covers restore-before-load and stale-id cleanup for authoritative
callers. [src: file: frontend/src/hooks/__tests__/usePersistentIdSet.test.tsx:5-25]

## Contrast scope

Preformatted descendants using the reusable `text-dim` utility are promoted to
the primary text token. [src: file: frontend/src/styles/utilities.css:50-58]

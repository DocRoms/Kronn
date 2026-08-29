# Collection shell accessibility and persistence contract

The shared collection shell is the common interaction boundary for Projects,
Discussions, MCP configurations, Pages, Planning tasks, and Automation
resources. Each surface supplies its own item model and business actions, while
the shell owns the common list interaction contract. [src: file: frontend/src/components/CollectionShell.tsx:160-184]

| Concern | Shared rule |
| --- | --- |
| Focus order | Native controls remain in DOM order: search, filters, rows, row actions, then detail actions. [src: file: frontend/src/components/CollectionShell.tsx:325-370] |
| Keyboard | `/` focuses search outside an editable field. `ArrowUp`, `ArrowDown`, `Home`, and `End` move only among enabled, visible row controls; action menus implement the same directional keys for enabled menu items. [src: file: frontend/src/components/CollectionShell.tsx:229-239] [src: file: frontend/src/components/CollectionShell.tsx:265-300] |
| Selection | A row exposes `aria-current` when it is the active item. Consumers that supply bulk-selection props have their selected identifiers pruned against refreshed items, and custom Page rows expose `role="checkbox"` plus `aria-checked` during bulk selection. [src: file: frontend/src/components/CollectionShell.tsx:179-184] [src: file: frontend/src/components/CollectionShell.tsx:303-314] [src: file: frontend/src/pages/PagesPage.tsx:561-574] |
| Menus and mobile sidebar | The action menu declares `aria-haspopup="menu"`, focuses its first enabled item when opened, and restores the trigger after Escape, an action, or outside dismissal. Escape closes an open mobile sidebar once no action menu owns the key. [src: file: frontend/src/components/CollectionShell.tsx:211-227] [src: file: frontend/src/components/CollectionShell.tsx:241-250] [src: file: frontend/src/components/CollectionShell.tsx:365-368] |
| Labels and focus indicator | Search, sidebar, action-menu, favorite, selection, and mobile controls receive accessible labels from the surface. The focused search container, shared controls, rows, and menu items provide an explicit indicator using the theme accent token. [src: file: frontend/src/components/CollectionShell.tsx:325-350] [src: file: frontend/src/components/CollectionShell.css:6-22] |
| Empty state | The default flat-list renderer shows the domain-specific empty-state slot when the filtered item set is empty. A custom `renderList` owns its empty state, because it replaces that default list branch. [src: file: frontend/src/components/CollectionShell.tsx:343-363] |

## Persistence policy

Persistence remains domain-owned so it can follow each resource's source of
truth. The shell has no browser-storage policy for favorites: it only renders
the domain callbacks it receives. Query and filter state stay local unless a
surface deliberately persists a validated navigation preference. Bulk selection
is intentionally transient. Only consumers that pass both `selectedIds` and
`onSelectedIdsChange` receive shell-managed cleanup after refresh, so stale
identifiers cannot reach their bulk action. [src: file: frontend/src/components/CollectionShell.tsx:179-184] [src: file: frontend/src/components/CollectionShell.tsx:343-355]

| Surface | Domain-owned state |
| --- | --- |
| Projects | The selected project is derived from the current filtered list. [src: file: frontend/src/components/ProjectList.tsx:163-171] |
| Discussions | Bulk selection is local to selection mode and cleared on exit; the shell receives that live selection for refresh cleanup. [src: file: frontend/src/components/DiscussionSidebar.tsx:212-280] [src: file: frontend/src/components/DiscussionSidebar.tsx:761-777] |
| MCP configurations | The page clears a selected configuration when the loaded kind-filtered list or its search query no longer contains it. [src: file: frontend/src/pages/McpPage.tsx:1066-1108] |
| Pages | Navigation and collapsed sections are restored from browser storage, with collapsed-section values whitelisted and the selected identifier validated against the fetched Page list. [src: file: frontend/src/pages/PagesPage.tsx:41-61] [src: file: frontend/src/pages/PagesPage.tsx:205-228] |
| Planning tasks | A selected task is loaded by identifier and cleared only when that exact detail request fails; absence from a filtered or paginated list does not prove that the task is invalid. [src: file: frontend/src/pages/PlanningPage.tsx:107-125] |
| Automation resources | The active tab/resource and collapsed sections are restored from browser storage only when their values match the current tab and section vocabulary; invalid selected resources are cleared after loading. [src: file: frontend/src/pages/WorkflowsPage.tsx:140-174] [src: file: frontend/src/pages/WorkflowsPage.tsx:230-248] [src: file: frontend/src/pages/WorkflowsPage.tsx:651-669] |

The contract is implemented by [CollectionShell](../../frontend/src/components/CollectionShell.tsx), whose consumers are [ProjectList](../../frontend/src/components/ProjectList.tsx), [DiscussionSidebar](../../frontend/src/components/DiscussionSidebar.tsx), [McpPage](../../frontend/src/pages/McpPage.tsx), [PagesPage](../../frontend/src/pages/PagesPage.tsx), [PlanningPage](../../frontend/src/pages/PlanningPage.tsx), and [WorkflowsPage](../../frontend/src/pages/WorkflowsPage.tsx). [src: file: frontend/src/components/ProjectList.tsx:230-260] [src: file: frontend/src/components/DiscussionSidebar.tsx:761-810] [src: file: frontend/src/pages/McpPage.tsx:2838-2860] [src: file: frontend/src/pages/PagesPage.tsx:524-615] [src: file: frontend/src/pages/PlanningPage.tsx:324-350] [src: file: frontend/src/pages/WorkflowsPage.tsx:1792-1859]

## Behavioral regression evidence

The six consumer regressions exercise the real surface markup and state rather
than relabeling one generic fixture. The shell suite separately covers its
flat-list and custom-renderer contracts, including keyboard navigation without
adding a wrapper around a custom list. [src: file: frontend/src/components/__tests__/CollectionShell.test.tsx:152-239]

| Surface | Executable behavior covered |
| --- | --- |
| Projects | The real flat-list row marks the current project and selects another project. [src: file: frontend/src/components/__tests__/ProjectList.missing-path.test.tsx:80-111] |
| Discussions | A refreshed list prunes a deleted bulk-selected discussion before a subsequent archive action. [src: file: frontend/src/components/__tests__/DiscussionSidebar.bulkActions.test.tsx:135-157] |
| Plugins | Real plugin rows move with ArrowDown and expose `aria-current` after selection. [src: file: frontend/src/pages/__tests__/McpPage.test.tsx:320-342] |
| Pages | The custom sidebar renderer retains keyboard movement, `aria-current`, and its own empty state. [src: file: frontend/src/pages/__tests__/PagesPage.test.tsx:387-404] |
| Planning | Real task rows move with ArrowDown and expose `aria-current`; a valid direct link outside the first result page stays selected, while an exact detail failure clears it. [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:113-132] [src: file: frontend/src/pages/__tests__/PlanningPage.test.tsx:208-247] |
| Automations | Grouped resource rows rendered by the Automation surface move with ArrowDown. [src: file: frontend/src/pages/__tests__/WorkflowsPage.test.tsx:225-242] |

## Contrast scope

Preformatted descendants using the reusable `text-dim` utility are promoted to
the primary text token. The selector is global rather than collection-specific,
so it applies to matching code-like content on every surface.
[src: file: frontend/src/styles/utilities.css:50-58]

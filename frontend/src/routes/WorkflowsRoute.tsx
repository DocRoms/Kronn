import { useCallback, useEffect, useMemo, useLayoutEffect, useRef } from 'react';
import { useLocation, useParams } from 'react-router';
import { useKronnNavigate } from '../hooks/useKronnNavigate';
import { readAutomationLastVisit } from '../lib/automationNavigation';
import { isUsable } from '../lib/constants';
import { useDashboardContext } from '../lib/dashboardContext';
import {
  automationEditorFromPath, automationPath, automationTabFromPath, sameAutomationSelection,
  type AutomationIntent, type AutomationSelection, type SelectionReason,
} from '../lib/routes';
import { WorkflowsPage } from '../pages/WorkflowsPage';
import { useLocationIntent } from './useLocationIntent';

type AutomationParams = {
  workflowId?: string; runId?: string; qpId?: string; qaId?: string; qeId?: string; skillId?: string;
};

export function WorkflowsRoute() {
  const ctx = useDashboardContext();
  const nav = useKronnNavigate();
  // The address owns the tab and the resource open in it; the page reports
  // every change and follows what it is then given.
  const location = useLocation();
  const { pathname } = location;
  const params = useParams<AutomationParams>();
  const tab = automationTabFromPath(pathname);
  const resourceId = params.workflowId ?? params.qpId ?? params.qaId ?? params.qeId ?? params.skillId ?? null;
  const runId = params.runId ?? null;
  // `/workflows/new` and `/workflows/<id>/edit`: the workflow wizard has the pane.
  const editor = automationEditorFromPath(pathname);
  const [intent, consumeIntent] = useLocationIntent<AutomationIntent>();
  // The bare `/workflows` names nothing: it reopens where the previous visit
  // left off, in place of itself. An explicit address — a tab, a resource,
  // the wizard — and Back/Forward are what they say. A preset arriving at the
  // bare address opens the wizard on the workflows list instead.
  const bare = tab === 'workflows' && !resourceId && !editor;
  const restored = useMemo(
    () => (bare && !intent?.preset ? readAutomationLastVisit() : null),
    [bare, intent?.preset],
  );
  const selection = useMemo<AutomationSelection>(() => {
    if (restored) return { tab: restored.tab, resourceId: restored.resourceId, runId: null };
    return editor ? { tab, resourceId, runId, editor } : { tab, resourceId, runId };
  }, [restored, tab, resourceId, runId, editor]);
  useEffect(() => {
    if (restored && automationPath(restored) !== pathname) nav.toAutomation(restored, { replace: true });
  }, [restored, pathname, nav]);
  // A choice is a step Back can undo; what the page restores or lets go on
  // its own replaces the address instead. The address is read through a ref:
  // the page lists this callback in an effect's dependencies.
  const addressed = useRef(selection);
  useLayoutEffect(() => { addressed.current = selection; }, [selection]);
  const handleSelectionChange = useCallback((next: AutomationSelection, reason: SelectionReason) => {
    if (sameAutomationSelection(next, addressed.current)) return;
    nav.toAutomation(next, { replace: reason === 'restore' });
  }, [nav]);
  return (
    <WorkflowsPage
      projects={ctx.projects}
      installedAgentTypes={ctx.agents.filter(isUsable).map(a => a.agent_type)}
      agentAccess={ctx.agentAccess ?? undefined}
      configLanguage={ctx.configLanguage ?? undefined}
      toast={ctx.toast}
      selection={selection}
      onSelectionChange={handleSelectionChange}
      addressToken={location}
      pendingPreset={intent?.preset ?? null}
      onPendingPresetConsumed={consumeIntent}
      highlightQuickPromptId={intent?.highlightQuickPrompt && tab === 'quickPrompts' ? resourceId : null}
      onHighlightConsumed={consumeIntent}
      onNavigateToBatch={(batchRunId) => nav.toDiscussions({ focusBatch: { id: batchRunId, mode: 'batch' } })}
      onNavigateDiscussion={(discussionId) => nav.toDiscussion(discussionId, { autoRun: true })}
      onNavigatePage={nav.toLivePage}
      onNavigateMcp={() => nav.toPage('mcps')}
      onNavigateSettings={() => nav.toPage('settings')}
      onNavigateSettingsAnchor={anchorId => nav.toSettingsSection(anchorId)}
      onBatchLaunched={(discussionIds, batchRunId, mode = 'batch') => {
        ctx.markBatchSending(discussionIds);
        // Land on the first child and focus its batch group in the sidebar:
        // the Discussions page expands and scrolls to it once the refetch
        // below has brought the new discussions in.
        if (discussionIds.length > 0) {
          nav.toDiscussion(discussionIds[0], { focusBatch: { id: batchRunId, mode } });
        }
        ctx.refetchDiscussions();
      }}
    />
  );
}

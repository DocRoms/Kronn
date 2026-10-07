import { useCallback, useMemo, useLayoutEffect, useRef } from 'react';
import { useLocation, useParams } from 'react-router';
import { useKronnNavigate } from '../hooks/useKronnNavigate';
import { isUsable } from '../lib/constants';
import { useDashboardContext } from '../lib/dashboardContext';
import {
  automationTabFromPath, sameAutomationSelection, type AutomationIntent, type AutomationSelection,
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
  const selection = useMemo<AutomationSelection>(() => ({ tab, resourceId, runId }), [tab, resourceId, runId]);
  // A choice is a step Back can undo; what the page restores on its own at
  // the bare address replaces it instead. The address is read through a ref:
  // the page lists this callback in an effect's dependencies.
  const addressed = useRef(selection);
  useLayoutEffect(() => { addressed.current = selection; }, [selection]);
  const handleSelectionChange = useCallback((next: AutomationSelection, reason: 'restore' | 'change') => {
    if (sameAutomationSelection(next, addressed.current)) return;
    nav.toAutomation(next, { replace: reason === 'restore' });
  }, [nav]);
  const [intent, consumeIntent] = useLocationIntent<AutomationIntent>();
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
      onNavigateToBatch={(batchRunId) => nav.toDiscussions({ focusBatch: { id: batchRunId, mode: 'batch' } })}
      onNavigateDiscussion={(discussionId) => nav.toDiscussion(discussionId, { autoRun: true })}
      onNavigatePage={nav.toLivePage}
      onNavigateMcp={() => nav.toPage('mcps')}
      onNavigateSettings={() => nav.toPage('settings')}
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

import { useOutletContext } from 'react-router';
import type { Dispatch, SetStateAction } from 'react';
import type {
  AgentDetection, AgentsConfig, AuditProgress, DiscussionListItem, DriftCheckResponse,
  LivePagesCapability, McpDefinition, McpOverview, Project, Skill, WorkflowSummary,
} from '../types/generated';
import type { ToastFn } from '../hooks/useToast';
import type { DiscussionsPageProps } from '../pages/DiscussionsPage';

export interface DiscussionPrefill {
  projectId: string;
  title: string;
  prompt: string;
  locked?: boolean;
}

/** State that outlives a page change because the dashboard shell owns it. */
type LiftedDiscussionState = Pick<DiscussionsPageProps,
  | 'sendingMap' | 'setSendingMap'
  | 'queuedMap' | 'setQueuedMap'
  | 'sendingStartMap' | 'setSendingStartMap'
  | 'streamingMap' | 'setStreamingMap'
  | 'noteStreamTick' | 'abortControllers' | 'cleanupStream'
  | 'lastSeenMsgCount' | 'markDiscussionSeen' | 'markAllDiscussionsSeen'
>;

/**
 * What the dashboard shell hands to the page rendered in its outlet.
 *
 * The shell fetches and polls the fleet once, whatever the page; a route
 * component reads what its page needs from here and maps it to props.
 */
export interface DashboardOutletContext extends LiftedDiscussionState {
  // ─── Fleet data ─────────────────────────────────────────────────────────
  projects: Project[];
  projectsLoading: boolean;
  projectsLoaded: boolean;
  agents: AgentDetection[];
  allDiscussions: DiscussionListItem[];
  discussionsByProject: Record<string, DiscussionListItem[]>;
  allSkills: Skill[];
  workflowList: WorkflowSummary[];
  activeAudits: AuditProgress[];
  driftByProject: Record<string, DriftCheckResponse>;
  configLanguage: string | null;
  agentAccess: AgentsConfig | null;
  mcpOverview: McpOverview;
  mcpOverviewLoaded: boolean;
  mcpRegistry: McpDefinition[];
  pagesCapability: LivePagesCapability | null;

  refetchProjects: () => void;
  refetchDiscussions: () => void;
  refetchSkills: () => void;
  refetchAgents: () => void;
  refetchAgentAccess: () => void;
  refetchLanguage: () => void;
  refetchMcps: () => void;
  refetchDrift: (projectId: string) => void;

  // ─── Shell services ─────────────────────────────────────────────────────
  toast: ToastFn;
  onReset: () => void;
  openAddProject: () => void;
  /** Lights the sidebar spinner of every child of a freshly launched batch. */
  markBatchSending: (discussionIds: string[]) => void;

  // ─── Cross-page handoffs ────────────────────────────────────────────────
  // Set by the page a navigation starts from, read once by the page it lands
  // on. Each one is a selection that has no address yet: it leaves this
  // context the day its page gets a `/:id` route.
  discPrefill: DiscussionPrefill | null;
  setDiscPrefill: Dispatch<SetStateAction<DiscussionPrefill | null>>;
  /** Where the previous visit left Discussions, when it still exists. */
  restorableDiscussionId: string | null;
  setActiveDiscussionId: (id: string | null) => void;
}

export function useDashboardContext(): DashboardOutletContext {
  return useOutletContext<DashboardOutletContext>();
}

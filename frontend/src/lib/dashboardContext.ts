import { useOutletContext } from 'react-router';
import type { Dispatch, MutableRefObject, SetStateAction } from 'react';
import type {
  Project, AgentDetection, Discussion, Skill,
  WorkflowSummary, AuditProgress, DriftCheckResponse,
  McpDefinition, McpOverview, AgentsConfig,
} from '../types/generated';
import type { ToastFn } from '../hooks/useToast';

export interface DashboardOutletContext {
  projects: Project[];
  projectsLoading: boolean;
  agents: AgentDetection[];
  allDiscussions: Discussion[];
  allSkills: Skill[];
  workflowList: WorkflowSummary[];
  activeAudits: AuditProgress[];
  configLanguage: string | null;
  agentAccess: AgentsConfig | null;
  mcpOverview: McpOverview;
  mcpRegistry: McpDefinition[];
  discussionsByProject: Record<string, Discussion[]>;
  driftByProject: Record<string, DriftCheckResponse>;
  lastSeenMsgCount: Record<string, number>;

  // Streaming state (DiscussionsPage)
  sendingMap: Record<string, boolean>;
  setSendingMap: Dispatch<SetStateAction<Record<string, boolean>>>;
  queuedMap: Record<string, boolean>;
  setQueuedMap: Dispatch<SetStateAction<Record<string, boolean>>>;
  sendingStartMap: Record<string, number>;
  setSendingStartMap: Dispatch<SetStateAction<Record<string, number>>>;
  streamingMap: Record<string, string>;
  setStreamingMap: Dispatch<SetStateAction<Record<string, string>>>;
  noteStreamTick: (discId: string) => void;
  abortControllers: MutableRefObject<Record<string, AbortController>>;
  cleanupStream: (discId: string) => void;

  // UI state
  expandedId: string | null;
  setExpandedId: (id: string | null) => void;
  discPrefill: { projectId: string; title: string; prompt: string; locked?: boolean } | null;
  setDiscPrefill: Dispatch<SetStateAction<{ projectId: string; title: string; prompt: string; locked?: boolean } | null>>;
  handlePrefillConsumed: () => void;
  activeDiscussionId: string | null;
  setActiveDiscussionId: Dispatch<SetStateAction<string | null>>;

  // Callbacks
  toast: ToastFn;
  refetch: () => void;
  refetchDiscussions: () => void;
  refetchSkills: () => void;
  refetchAgents: () => void;
  refetchAgentAccess: () => void;
  refetchLanguage: () => void;
  refetchMcps: () => void;
  handleRefetchDrift: (projectId: string) => void;
  markDiscussionSeen: (discId: string, msgCount: number) => void;
  markAllDiscussionsSeen: () => void;
  onReset: () => void;
  onBatchSendingMark: (discIds: string[]) => void;
}

export function useDashboardContext() {
  return useOutletContext<DashboardOutletContext>();
}

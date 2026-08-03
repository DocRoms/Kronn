const WORKSPACE_TARGET_KEY = 'kronn:discussion-workspace-target';

interface DiscussionWorkspaceTarget {
  discussionId: string;
  workspaceId: string;
}

export function queueDiscussionWorkspaceTarget(
  discussionId: string,
  workspaceId: string,
): void {
  try {
    sessionStorage.setItem(
      WORKSPACE_TARGET_KEY,
      JSON.stringify({ discussionId, workspaceId } satisfies DiscussionWorkspaceTarget),
    );
  } catch {
    // Navigation still works when storage is unavailable; only the panel hint is lost.
  }
}

export function consumeDiscussionWorkspaceTarget(
  discussionId: string,
): string | undefined {
  try {
    const raw = sessionStorage.getItem(WORKSPACE_TARGET_KEY);
    if (!raw) return undefined;
    const target = JSON.parse(raw) as Partial<DiscussionWorkspaceTarget>;
    if (
      typeof target.discussionId !== 'string'
      || typeof target.workspaceId !== 'string'
    ) {
      sessionStorage.removeItem(WORKSPACE_TARGET_KEY);
      return undefined;
    }
    if (target.discussionId !== discussionId) return undefined;
    sessionStorage.removeItem(WORKSPACE_TARGET_KEY);
    return target.workspaceId;
  } catch {
    try {
      sessionStorage.removeItem(WORKSPACE_TARGET_KEY);
    } catch {
      // Storage can be fully unavailable in hardened browser contexts.
    }
    return undefined;
  }
}

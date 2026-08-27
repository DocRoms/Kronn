export function orchestrationResolution(message: string): string {
  const lower = message.toLowerCase();
  if (lower.includes('conflict') || lower.includes('fast-forward') || lower.includes('head drift')) {
    return 'resolve_git';
  }
  if (lower.includes('test') || lower.includes('validation')) return 'fix_tests';
  if (lower.includes('quota') || lower.includes('unavailable') || lower.includes('expired')) {
    return 'reassign_agent';
  }
  if (lower.includes('workspace') || lower.includes('worktree') || lower.includes('project')) {
    return 'restore_workspace';
  }
  return 'retry';
}

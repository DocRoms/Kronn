// KT-1017 — the Exec lines of an agent's proposal, shown before a human
// accepts it: every one carrying run values waits for an approval anyway.

type Json = Record<string, unknown>;

const describe = (command: unknown, args: unknown) =>
  `${String(command)} ${JSON.stringify(Array.isArray(args) ? args : [])}`;

/** One line per Exec command of `steps` (main, setup, stdin, inline sources). */
export function agentExecLines(steps: unknown): string[] {
  if (!Array.isArray(steps)) return [];
  const lines: string[] = [];
  for (const raw of steps) {
    if (!raw || typeof raw !== 'object') continue;
    const step = raw as Json;
    const name = String(step.name ?? '?');
    if (typeof step.exec_command === 'string' && step.exec_command.trim()) {
      lines.push(`${name}: ${describe(step.exec_command, step.exec_args)}`);
    }
    if (typeof step.exec_setup_command === 'string' && step.exec_setup_command.trim()) {
      lines.push(`${name} (setup): ${describe(step.exec_setup_command, step.exec_setup_args)}`);
    }
    if (typeof step.exec_stdin === 'string') {
      lines.push(`${name} (stdin): ${step.exec_stdin}`);
    }
    const collect = step.collect_api_data as Json | undefined;
    const sources = Array.isArray(collect?.sources) ? collect.sources as Json[] : [];
    for (const source of sources) {
      const exec = source?.quick_exec as Json | undefined;
      if (exec && typeof exec.command === 'string') {
        lines.push(`${name} (${String(source.alias ?? '?')}): ${describe(exec.command, exec.args)}`);
      }
    }
  }
  return lines;
}

/** The Exec lines of a workflow-shaped payload, its rollback chain included. */
export function workflowExecLines(workflow: unknown): string[] {
  if (!workflow || typeof workflow !== 'object') return [];
  const value = workflow as Json;
  return [...agentExecLines(value.steps), ...agentExecLines(value.on_failure)];
}

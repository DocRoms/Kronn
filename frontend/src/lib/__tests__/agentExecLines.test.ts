import { describe, it, expect } from 'vitest';
import { agentExecLines, workflowExecLines } from '../agentExecLines';

describe('agentExecLines', () => {
  it('lists every Exec line of a proposal, sources and rollback included', () => {
    const lines = workflowExecLines({
      steps: [
        { name: 'plan', exec_command: 'terraform', exec_args: ['plan', '{{x}}'], exec_stdin: '{{y}}' },
        { name: 'agent' },
        {
          name: 'collect',
          collect_api_data: { sources: [{ alias: 'logs', quick_exec: { command: 'aws', args: ['logs', '{{q}}'] } }] },
        },
      ],
      on_failure: [{ name: 'undo', exec_command: 'bash', exec_args: ['-c', 'echo "$1"', '_', '{{x}}'] }],
    });
    expect(lines).toEqual([
      'plan: terraform ["plan","{{x}}"]',
      'plan (stdin): {{y}}',
      'collect (logs): aws ["logs","{{q}}"]',
      'undo: bash ["-c","echo \\"$1\\"","_","{{x}}"]',
    ]);
    expect(agentExecLines('nope')).toEqual([]);
  });
});

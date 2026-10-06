import { describe, it, expect, vi } from 'vitest';

vi.mock('../api', () => ({
  workflows: {
    get: vi.fn().mockResolvedValue({
      id: 'wf', name: 'x', trigger: { type: 'Manual' }, actions: [], workspace_config: null,
      steps: [{ name: 'plan', step_type: { type: 'Exec' }, exec_command: 'bash', exec_args: ['-c', 'terraform plan "$1"', '_', '{{x}}'] }],
    }),
  },
  quickApis: {}, quickExecs: {}, quickPrompts: {},
}));

import { loadExecutionSummary } from '../repositoryResourceExecution';

describe('loadExecutionSummary', () => {
  it('shows every Exec line a repository workflow runs on the approve sheet', async () => {
    const summary = await loadExecutionSummary('workflow', 'wf');
    const exec = summary?.fields.find(field => field.field === 'exec');
    expect(exec?.values).toEqual(['plan: bash ["-c","terraform plan \\"$1\\"","_","{{x}}"]']);
  });
});

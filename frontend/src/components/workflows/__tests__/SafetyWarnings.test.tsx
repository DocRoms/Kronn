import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

const { safetyCheckMock } = vi.hoisted(() => ({ safetyCheckMock: vi.fn() }));
vi.mock('../../../lib/api', () => ({ workflows: { safetyCheck: safetyCheckMock } }));
vi.mock('../../../lib/I18nContext', () => ({ useT: () => ({ t: (key: string) => key }) }));

import { SafetyWarnings } from '../SafetyWarnings';

const request = {
  workflow_id: 'wf-1',
  project_id: 'proj-1',
  per_run_project: false,
  safety: { sandbox: true, require_approval: true, max_files: 2, max_lines: null },
};

describe('SafetyWarnings', () => {
  beforeEach(() => { safetyCheckMock.mockReset(); });

  it('says which stored settings every run here would be refused for', async () => {
    safetyCheckMock.mockResolvedValue(['sandbox_outside_container', 'approval_on_sub_workflow']);
    render(<SafetyWarnings request={request} />);
    expect(await screen.findByText('wf.safetyWarning.sandbox_outside_container')).toBeInTheDocument();
    expect(screen.getByText('wf.safetyWarning.approval_on_sub_workflow')).toBeInTheDocument();
    expect(safetyCheckMock).toHaveBeenCalledWith(request);
  });

  it('shows nothing when the check finds nothing or fails', async () => {
    safetyCheckMock.mockRejectedValue(new Error('offline'));
    render(<SafetyWarnings request={request} />);
    await waitFor(() => expect(safetyCheckMock).toHaveBeenCalled());
    expect(screen.queryByTestId('wf-safety-warnings')).not.toBeInTheDocument();
  });
});

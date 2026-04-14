// Smoke tests for large workflow components (0.3.7 stability).
// Verify they mount without crashing given minimal props.

import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';

vi.mock('../../../lib/api', () => buildApiMock());
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));
vi.mock('../../../hooks/useMediaQuery', () => ({
  useIsMobile: () => false,
}));

import { WorkflowWizard } from '../WorkflowWizard';

const noop = () => {};

describe('Workflow smoke tests', () => {
  it('WorkflowWizard renders step 1 without crashing', () => {
    render(
      <WorkflowWizard
        projects={[]}
        onDone={noop}
        onCancel={noop}
        installedAgentTypes={['ClaudeCode']}
      />
    );
    // The wizard mounted without crashing — verify some content rendered
    const text = document.body.textContent ?? '';
    expect(text.length).toBeGreaterThan(10);
  });
});

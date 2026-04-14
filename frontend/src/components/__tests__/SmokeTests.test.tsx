// Smoke tests for large untested components (0.3.7 stability pass).
// Goal: verify each component renders without crashing given minimal props.
// These are NOT interaction tests — just "does it mount?" guards.

import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));
vi.mock('../../hooks/useMediaQuery', () => ({
  useIsMobile: () => false,
}));

import { ChatInput } from '../ChatInput';
import { ProjectCard } from '../ProjectCard';

const noop = () => {};
const t = (key: string) => key;

describe('Smoke tests — large components', () => {
  it('ChatInput renders without crashing (no discussion)', () => {
    render(
      <ChatInput
        discussion={null}
        agents={[]}
        sending={false}
        disabled={false}
        ttsEnabled={false}
        ttsState="idle"
        worktreeError={null}
        availableSkills={[]}
        availableDirectives={[]}
        onSend={noop}
        onStop={noop}
        onOrchestrate={noop}
        onTtsToggle={noop}
        onWorktreeErrorDismiss={noop}
        onWorktreeRetry={noop}
        isAgentRestricted={() => false}
        toast={vi.fn()}
        t={t}
      />
    );
    // Should render the textarea
    expect(document.querySelector('textarea')).toBeTruthy();
  });

  it('ProjectCard renders without crashing (collapsed)', () => {
    const proj = {
      id: 'p-1', name: 'TestProject', path: '/tmp/test',
      repo_url: null, token_override: null,
      ai_config: { detected: false, configs: [] },
      audit_status: 'NoTemplate' as const, ai_todo_count: 0,
      created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    };
    render(
      <ProjectCard
        project={proj as any}
        isOpen={false}
        onToggleOpen={noop}
        discussions={[]}
        driftStatus={undefined}
        agents={[]}
        allSkills={[]}
        mcpConfigs={[]}
        workflows={[]}
        configLanguage="fr"
        toast={vi.fn()}
        onNavigate={noop}
        onSetDiscPrefill={noop}
        onAutoRunDiscussion={noop}
        onOpenDiscussion={noop}
        onRefetch={noop}
        onRefetchDiscussions={noop}
        onRefetchSkills={noop}
        onRefetchDrift={noop}
      />
    );
    expect(screen.getByText('TestProject')).toBeInTheDocument();
  });

  // ChatHeader + WorkflowDetail: covered by their respective page integration
  // tests (DiscussionsPage.test.tsx, WorkflowsPage.test.tsx). They have complex
  // module-level dependencies that break in isolated happy-dom smoke tests.
});

// Unit tests for BatchItemsList — the per-item expand + dry-run panel on
// the BatchQuickPrompt step's test view.
//
// Scope: prompt expansion toggle, dry-run launch forwarding the rendered
// prompt to testStepStream, disabled-while-running state, abort on
// relaunch.
//
// Uses the shared apiMock helper — see `src/test/apiMock.ts`.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';

// Mock api BEFORE importing the component. vi.mock is hoisted above
// all imports, so the factory captures mocks via vi.hoisted() — regular
// top-level consts would be undefined at mock-evaluation time.
const { testStepStreamMock } = vi.hoisted(() => ({
  testStepStreamMock: vi.fn(),
}));
vi.mock('../../../lib/api', () => buildApiMock({
  workflows: { testStepStream: testStepStreamMock as never },
}));

// i18n: echo the key so assertions can match on stable identifiers.
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

import { BatchItemsList } from '../WorkflowDetail';

const t = (key: string) => key;

describe('BatchItemsList', () => {
  beforeEach(() => {
    testStepStreamMock.mockReset();
  });

  it('renders one row per item with a collapsed prompt body', () => {
    render(
      <BatchItemsList
        items={['TICKET-1', 'TICKET-2', 'TICKET-3']}
        renderedPrompts={['rendered for 1', 'rendered for 2', 'rendered for 3']}
        quickPromptAgent="ClaudeCode"
        projectId="proj-1"
        t={t}
      />
    );
    expect(screen.getByText('TICKET-1')).toBeInTheDocument();
    expect(screen.getByText('TICKET-2')).toBeInTheDocument();
    expect(screen.getByText('TICKET-3')).toBeInTheDocument();
    // Rendered prompts are hidden until expand — they should NOT be in the DOM.
    expect(screen.queryByText('rendered for 1')).not.toBeInTheDocument();
  });

  it('reveals the rendered prompt when the item is toggled open', () => {
    render(
      <BatchItemsList
        items={['TICKET-1']}
        renderedPrompts={['this is the rendered prompt for ticket 1']}
        quickPromptAgent="ClaudeCode"
        projectId={null}
        t={t}
      />
    );
    const toggle = screen.getByRole('button', { name: /TICKET-1/ });
    fireEvent.click(toggle);
    expect(screen.getByText('this is the rendered prompt for ticket 1')).toBeInTheDocument();
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    // Second click collapses again.
    fireEvent.click(toggle);
    expect(screen.queryByText('this is the rendered prompt for ticket 1')).not.toBeInTheDocument();
  });

  it('clicking dry-run forwards the rendered prompt to testStepStream', async () => {
    render(
      <BatchItemsList
        items={['ABC-99']}
        renderedPrompts={['Prompt résolu pour ABC-99']}
        quickPromptAgent="Codex"
        projectId="proj-42"
        t={t}
      />
    );
    // The dry-run button is labelled via t() → 'wiz.testBatchItemDryRunBtn'.
    const dryRunBtn = screen.getByRole('button', { name: /testBatchItemDryRunBtn/ });
    fireEvent.click(dryRunBtn);

    await waitFor(() => expect(testStepStreamMock).toHaveBeenCalledTimes(1));
    const [req] = testStepStreamMock.mock.calls[0];
    expect(req.step.prompt_template).toBe('Prompt résolu pour ABC-99');
    expect(req.step.step_type.type).toBe('Agent');
    expect(req.step.agent).toBe('Codex');
    expect(req.dry_run).toBe(true);
    expect(req.project_id).toBe('proj-42');
  });

  it('omits the dry-run button entirely when the Quick Prompt has no agent', () => {
    render(
      <BatchItemsList
        items={['X-1']}
        renderedPrompts={['p']}
        quickPromptAgent={null}
        projectId={null}
        t={t}
      />
    );
    // The 🧪 trigger must be absent — the QP didn't pin an agent, so we
    // wouldn't know which backend to spawn.
    expect(screen.queryByRole('button', { name: /testBatchItemDryRunBtn/ })).not.toBeInTheDocument();
  });

  it('running state disables the button and prevents double-trigger', async () => {
    // Never-resolving stream — simulates "agent is thinking".
    testStepStreamMock.mockImplementation(() => new Promise(() => { /* never resolves */ }));
    render(
      <BatchItemsList
        items={['T-1']}
        renderedPrompts={['p']}
        quickPromptAgent="ClaudeCode"
        projectId={null}
        t={t}
      />
    );
    const btn = screen.getByRole('button', { name: /testBatchItemDryRunBtn/ });
    fireEvent.click(btn);
    // Button flips to the "loading" label + disabled.
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /testBatchItemDryRunLoading/ })).toBeDisabled();
    });
    expect(testStepStreamMock).toHaveBeenCalledTimes(1);
  });

  it('skips items without a rendered prompt (defensive when arrays are misaligned)', async () => {
    render(
      <BatchItemsList
        items={['T-1']}
        renderedPrompts={[]} // misaligned — component must not crash or call API
        quickPromptAgent="ClaudeCode"
        projectId={null}
        t={t}
      />
    );
    const btn = screen.getByRole('button', { name: /testBatchItemDryRunBtn/ });
    fireEvent.click(btn);
    // No call — the guard returned early.
    await new Promise((r) => setTimeout(r, 50));
    expect(testStepStreamMock).not.toHaveBeenCalled();
  });
});

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { LearningsModal } from '../LearningsModal';
import type { Learning } from '../../types/generated';

const t = (key: string) => key;

const mk = (over: Partial<Learning> & { id: string }): Learning => ({
  claim: 'uses pnpm strict',
  evidence: [{ kind: 'file', ref: 'package.json:2', quote: '"packageManager": "pnpm"' }],
  kind: 'fact',
  status: 'pending',
  created_at: '2026-06-01T00:00:00Z',
  ...over,
});

describe('LearningsModal', () => {
  it('renders a card per learning with claim, kind chip and evidence', () => {
    render(
      <LearningsModal
        learnings={[
          mk({ id: 'a' }),
          mk({
            id: 'b',
            claim: 'hooks in src/hooks',
            kind: 'inference',
            evidence: [{ kind: 'file', ref: 'src/hooks/index.ts:1' }],
          }),
        ]}
        onValidate={() => {}}
        onReject={() => {}}
        onClose={() => {}}
        t={t}
      />
    );
    expect(screen.getByText('uses pnpm strict')).toBeInTheDocument();
    expect(screen.getByText('hooks in src/hooks')).toBeInTheDocument();
    expect(screen.getByText('package.json:2')).toBeInTheDocument();
    // kind chips (fact + inference) via i18n keys
    expect(screen.getByText('disc.learningKindFact')).toBeInTheDocument();
    expect(screen.getByText('disc.learningKindInference')).toBeInTheDocument();
  });

  it('shows the empty state when there are no learnings', () => {
    render(
      <LearningsModal learnings={[]} onValidate={() => {}} onReject={() => {}} onClose={() => {}} t={t} />
    );
    expect(screen.getByText('disc.learningsEmpty')).toBeInTheDocument();
  });

  it('fires onValidate / onReject with the learning id', () => {
    const onValidate = vi.fn();
    const onReject = vi.fn();
    render(
      <LearningsModal
        learnings={[mk({ id: 'x42' })]}
        onValidate={onValidate}
        onReject={onReject}
        onClose={() => {}}
        t={t}
      />
    );
    fireEvent.click(screen.getByText('disc.learningValidate'));
    expect(onValidate).toHaveBeenCalledWith('x42');
    fireEvent.click(screen.getByText('disc.learningReject'));
    expect(onReject).toHaveBeenCalledWith('x42');
  });

  it('renders the Gate-2 faithfulness chip when present (informative, posture B)', () => {
    render(
      <LearningsModal
        learnings={[mk({ id: 'c', faithfulness: 'contradiction' })]}
        onValidate={() => {}}
        onReject={() => {}}
        onClose={() => {}}
        t={t}
      />
    );
    expect(screen.getByText('disc.learningFaithContradiction')).toBeInTheDocument();
  });

  it('disables actions for the busy row only', () => {
    render(
      <LearningsModal
        learnings={[mk({ id: 'busy' })]}
        onValidate={() => {}}
        onReject={() => {}}
        onClose={() => {}}
        busyId="busy"
        t={t}
      />
    );
    expect(screen.getByText('disc.learningValidate').closest('button')).toBeDisabled();
  });

  it('closes on backdrop click but not on dialog click', () => {
    const onClose = vi.fn();
    render(
      <LearningsModal learnings={[mk({ id: 'a' })]} onValidate={() => {}} onReject={() => {}} onClose={onClose} t={t} />
    );
    fireEvent.click(screen.getByRole('dialog'));
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'disc.learningClose' }));
    expect(onClose).toHaveBeenCalled();
  });
});

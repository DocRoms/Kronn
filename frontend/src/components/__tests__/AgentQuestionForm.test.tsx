import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { AgentQuestionForm } from '../AgentQuestionForm';

const t = (key: string) => key;

describe('AgentQuestionForm', () => {
  it('renders one labelled input per question', () => {
    render(
      <AgentQuestionForm
        questions={[
          { var: 'priority', question: 'Priorité ?' },
          { var: 'scope', question: 'Scope ?' },
        ]}
        discussionId="d-1"
        onSubmit={() => {}}
        t={t}
      />
    );
    expect(screen.getByRole('textbox', { name: 'Priorité ?' })).toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: 'Scope ?' })).toBeInTheDocument();
    // Var name rendered alongside the question for disambiguation
    expect(screen.getByText('priority')).toBeInTheDocument();
    expect(screen.getByText('scope')).toBeInTheDocument();
  });

  it('submit is disabled until every field is non-empty', () => {
    render(
      <AgentQuestionForm
        questions={[
          { var: 'a', question: 'A ?' },
          { var: 'b', question: 'B ?' },
        ]}
        discussionId="d-1"
        onSubmit={() => {}}
        t={t}
      />
    );
    const submit = screen.getByRole('button', { name: /agentQuestions.submit/ });
    expect(submit).toBeDisabled();
    fireEvent.change(screen.getByRole('textbox', { name: 'A ?' }), { target: { value: 'high' } });
    expect(submit).toBeDisabled(); // still one empty
    fireEvent.change(screen.getByRole('textbox', { name: 'B ?' }), { target: { value: 'front' } });
    expect(submit).not.toBeDisabled();
  });

  it('submits composed "var: value" lines on click', () => {
    const onSubmit = vi.fn();
    render(
      <AgentQuestionForm
        questions={[
          { var: 'priority', question: 'Priorité ?' },
          { var: 'scope', question: 'Scope ?' },
        ]}
        discussionId="d-1"
        onSubmit={onSubmit}
        t={t}
      />
    );
    fireEvent.change(screen.getByRole('textbox', { name: 'Priorité ?' }), { target: { value: 'high' } });
    fireEvent.change(screen.getByRole('textbox', { name: 'Scope ?' }), { target: { value: '  full-stack  ' } });
    fireEvent.click(screen.getByRole('button', { name: /agentQuestions.submit/ }));
    expect(onSubmit).toHaveBeenCalledWith('priority: high\nscope: full-stack');
  });

  it('resets answers when discussionId changes', () => {
    const { rerender } = render(
      <AgentQuestionForm
        questions={[{ var: 'a', question: 'A ?' }]}
        discussionId="d-1"
        onSubmit={() => {}}
        t={t}
      />
    );
    const input = screen.getByRole('textbox', { name: 'A ?' }) as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'typed' } });
    expect(input.value).toBe('typed');

    rerender(
      <AgentQuestionForm
        questions={[{ var: 'a', question: 'A ?' }]}
        discussionId="d-2"
        onSubmit={() => {}}
        t={t}
      />
    );
    const input2 = screen.getByRole('textbox', { name: 'A ?' }) as HTMLInputElement;
    expect(input2.value).toBe('');
  });

  it('resets answers when the question set changes on the same discussion', () => {
    const { rerender } = render(
      <AgentQuestionForm
        questions={[{ var: 'old', question: 'Old ?' }]}
        discussionId="d-1"
        onSubmit={() => {}}
        t={t}
      />
    );
    fireEvent.change(screen.getByRole('textbox', { name: 'Old ?' }), { target: { value: 'answered' } });

    // New agent turn on same disc asks different questions — old answers cleared.
    rerender(
      <AgentQuestionForm
        questions={[{ var: 'newvar', question: 'New ?' }]}
        discussionId="d-1"
        onSubmit={() => {}}
        t={t}
      />
    );
    const newInput = screen.getByRole('textbox', { name: 'New ?' }) as HTMLInputElement;
    expect(newInput.value).toBe('');
  });
});

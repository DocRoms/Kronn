import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { DecideStepForm } from '../DecideStepForm';
import { blankDecideConfig, decideConfigProblem, decideSignals } from '../../../lib/decideStep';
import type { DecideConfig } from '../../../types/generated';

const t = (key: string, ...args: (string | number)[]) => args.length ? `${key}:${args.join(',')}` : key;

describe('DecideStepForm (KT-1367)', () => {
  it('names the signals each question emits', () => {
    expect(decideSignals({ id: 'ship', kind: 'noul', instructions: 'x' })).toEqual(['ship_yes', 'ship_no']);
    expect(decideSignals({ id: 'route', kind: 'choice', instructions: 'x', options: { local: 'a', advanced: 'b' } }))
      .toEqual(['route_local', 'route_advanced']);
    expect(decideSignals({ id: 'risk', kind: 'score', instructions: 'x', levels: ['low', 'mid', 'high'] }))
      .toEqual(['risk_1', 'risk_2', 'risk_3']);
  });

  it('says why a config cannot be saved, like the backend', () => {
    const ok: DecideConfig = { state: '{{steps.tests.output}}', questions: [{ id: 'ship', kind: 'noul', instructions: 'Ready?' }] };
    expect(decideConfigProblem(ok)).toBeNull();
    expect(decideConfigProblem(blankDecideConfig())).toBe('instructions');
    expect(decideConfigProblem({ ...ok, state: ' ' })).toBe('state');
    expect(decideConfigProblem({ ...ok, questions: [] })).toBe('questions');
    expect(decideConfigProblem({ ...ok, questions: [{ ...ok.questions[0], id: 'Ship!' }] })).toBe('id');
    expect(decideConfigProblem({ ...ok, questions: [{ id: 'r', kind: 'choice', instructions: 'x', options: { a: 'only one' } }] })).toBe('options');
    expect(decideConfigProblem({ ...ok, questions: [{ id: 's', kind: 'score', instructions: 'x', levels: ['one'] }] })).toBe('levels');
    expect(decideConfigProblem(null)).toBe('missing');
  });

  it('turns a question into a choice with options and lists its signals', () => {
    const onChange = vi.fn();
    const config: DecideConfig = { state: 's', questions: [{ id: 'route', kind: 'noul', instructions: 'Which tier?' }] };
    const { rerender } = render(<DecideStepForm config={config} previousSteps={['tests']} onChange={onChange} t={t} />);
    fireEvent.change(screen.getByLabelText('wiz.decideKind'), { target: { value: 'choice' } });
    const choice = onChange.mock.calls[0][0] as DecideConfig;
    expect(choice.questions[0].kind).toBe('choice');
    expect(Object.keys(choice.questions[0].options ?? {})).toHaveLength(2);
    rerender(<DecideStepForm config={choice} previousSteps={['tests']} onChange={onChange} t={t} />);
    expect(screen.getByTestId('decide-signals')).toHaveTextContent('route_a');
    expect(screen.getByTestId('decide-signals')).toHaveTextContent('route_unsure');
    fireEvent.click(screen.getByText('{{steps.tests.output}}'));
    expect((onChange.mock.calls.at(-1)![0] as DecideConfig).state).toBe('{{steps.tests.output}}');
  });
});

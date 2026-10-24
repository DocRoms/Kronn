import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import { WorkflowRefsCard } from '../WorkflowRefsCard';
import { isSafeRefId, isValidRefName, type RefOptions } from '../../../lib/workflowRefs';

const t = (key: string, ...args: (string | number)[]) => (args.length ? `${key}:${args.join(',')}` : key);
const options: RefOptions = {
  page: [{ id: 'page-1', label: 'Équipe 🦀' }, { id: 'repository:p:x', label: 'Not referable' }],
  workflow: [{ id: 'wf-1', label: 'Nightly' }],
  quick_prompt: [], quick_api: [], quick_exec: [], skill: [{ id: 'custom-review', label: 'Review' }],
};

afterEach(() => cleanup());

describe('WorkflowRefsCard', () => {
  it('adds a named ref to the picked object and shows how steps read it', () => {
    const onChange = vi.fn();
    render(<WorkflowRefsCard value={{}} onChange={onChange} options={options} t={t} />);
    fireEvent.click(screen.getByRole('button', { name: /wiz\.refs\.title/ }));
    const add = screen.getByRole('button', { name: /wiz\.refs\.add/ });
    expect(add).toBeDisabled();
    fireEvent.change(screen.getByLabelText('wiz.refs.name'), { target: { value: 'board' } });
    const object = screen.getByLabelText('wiz.refs.object');
    expect(screen.queryByRole('option', { name: 'Not referable' })).toBeNull();
    fireEvent.change(object, { target: { value: 'page-1' } });
    fireEvent.click(add);
    expect(onChange).toHaveBeenLastCalledWith({ board: { kind: 'page', id: 'page-1' } });
  });

  it('lists existing refs with their placeholder and env name, and removes one', () => {
    const onChange = vi.fn();
    render(<WorkflowRefsCard
      value={{ board: { kind: 'page', id: 'page-1' }, review: { kind: 'skill', id: 'custom-gone' } }}
      onChange={onChange} options={options} t={t} />);
    expect(screen.getByText('{{refs.board}}')).toBeInTheDocument();
    expect(screen.getByText('KRONN_REF_BOARD')).toBeInTheDocument();
    expect(screen.getByText('Équipe 🦀', { selector: 'span' })).toBeInTheDocument();
    expect(screen.getByText('custom-gone')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'wiz.refs.remove:board' }));
    expect(onChange).toHaveBeenLastCalledWith({ review: { kind: 'skill', id: 'custom-gone' } });
  });

  it('refuses an invalid or duplicate name', () => {
    render(<WorkflowRefsCard value={{ board: { kind: 'page', id: 'page-1' } }} onChange={vi.fn()} options={options} t={t} />);
    const name = screen.getByLabelText('wiz.refs.name');
    fireEvent.change(name, { target: { value: 'Board' } });
    expect(screen.getByRole('alert')).toHaveTextContent('wiz.refs.invalidName');
    fireEvent.change(name, { target: { value: 'board' } });
    expect(screen.getByRole('alert')).toHaveTextContent('wiz.refs.duplicate');
    expect(screen.getByRole('button', { name: /wiz\.refs\.add/ })).toBeDisabled();
  });

  it('follows the backend charset rules', () => {
    expect(isValidRefName('page_2')).toBe(true);
    expect(isValidRefName('2page')).toBe(false);
    expect(isValidRefName(`a${'b'.repeat(41)}`)).toBe(false);
    expect(isSafeRefId('550e8400-e29b-41d4-a716-446655440000')).toBe(true);
    expect(isSafeRefId('-rf')).toBe(false);
    expect(isSafeRefId('a;b')).toBe(false);
  });
});

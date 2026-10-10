import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DefaultTodoStatus } from '../../types/generated';

vi.mock('../../lib/api', () => ({ pages: { defaultTodo: vi.fn(), installDefaultTodo: vi.fn() } }));
vi.mock('../../lib/I18nContext', () => ({ useT: () => ({ t: (key: string) => key }) }));

import { pages as pagesApi } from '../../lib/api';
import { DefaultTodoOffer } from '../DefaultTodoOffer';

function status(overrides: Partial<DefaultTodoStatus> = {}): DefaultTodoStatus {
  return { state: 'installed', page_id: 'page-1', workflow_ids: [], own_page_id: null, ...overrides };
}

beforeEach(() => {
  vi.mocked(pagesApi.defaultTodo).mockReset();
  vi.mocked(pagesApi.installDefaultTodo).mockReset();
});

describe('DefaultTodoOffer', () => {
  it('offers nothing while Kronn’s board is there', async () => {
    vi.mocked(pagesApi.defaultTodo).mockResolvedValue(status());
    const { container } = render(<DefaultTodoOffer refreshKey={0} onInstalled={vi.fn()} />);
    await waitFor(() => expect(pagesApi.defaultTodo).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });

  it('reinstalls a deleted board on request and opens it', async () => {
    vi.mocked(pagesApi.defaultTodo).mockResolvedValue(status({ state: 'removed', page_id: null }));
    vi.mocked(pagesApi.installDefaultTodo).mockResolvedValue(status({ page_id: 'page-2' }));
    const onInstalled = vi.fn();
    render(<DefaultTodoOffer refreshKey={0} onInstalled={onInstalled} />);
    fireEvent.click(await screen.findByText('pages.defaultTodo.reinstall'));
    await waitFor(() => expect(onInstalled).toHaveBeenCalledWith('page-2'));
    expect(pagesApi.installDefaultTodo).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId('default-todo-offer')).toBeNull();
  });

  it('proposes to add Kronn’s board next to a todo of one’s own', async () => {
    vi.mocked(pagesApi.defaultTodo).mockResolvedValue(status({ state: 'kept_existing', page_id: null, own_page_id: 'mine' }));
    render(<DefaultTodoOffer refreshKey={0} onInstalled={vi.fn()} />);
    expect(await screen.findByText('pages.defaultTodo.kept')).toBeInTheDocument();
    expect(screen.getByText('pages.defaultTodo.installAlongside')).toBeInTheDocument();
    expect(pagesApi.installDefaultTodo).not.toHaveBeenCalled();
  });

  it('shows why a reinstall was refused', async () => {
    vi.mocked(pagesApi.defaultTodo).mockResolvedValue(status({ state: 'removed', page_id: null }));
    vi.mocked(pagesApi.installDefaultTodo).mockRejectedValue(new Error('already installed'));
    render(<DefaultTodoOffer refreshKey={0} onInstalled={vi.fn()} />);
    fireEvent.click(await screen.findByText('pages.defaultTodo.reinstall'));
    expect(await screen.findByRole('alert')).toHaveTextContent('already installed');
  });
});

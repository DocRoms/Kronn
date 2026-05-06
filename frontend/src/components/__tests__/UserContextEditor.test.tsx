// UserContextEditor — CRUD flow against ~/.kronn/user-context/.
//
// Coverage : list → expand → edit → save → delete + error/no-op states.
// The editor sits in Settings, but it's testable in isolation : its only
// external dependency is the `userContext` API namespace, fully mocked
// here. I18n is stubbed so test assertions match key names verbatim.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

vi.mock('../../lib/api', () => ({
  userContext: {
    list: vi.fn(),
    get: vi.fn(),
    put: vi.fn(),
    delete: vi.fn(),
  },
}));

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    // Test stub: append every interpolation arg after the key so assertions
    // can match `keyName arg1` without the test having to know each key's
    // {0}/{1} structure (real i18n strings live in i18n.ts and are covered
    // by their own parity test).
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
  }),
}));

import { UserContextEditor } from '../UserContextEditor';
import { userContext as api } from '../../lib/api';

type Mock = ReturnType<typeof vi.fn>;
const mockApi = api as unknown as {
  list: Mock; get: Mock; put: Mock; delete: Mock;
};

describe('UserContextEditor', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders empty state when no files', async () => {
    mockApi.list.mockResolvedValue([]);
    render(<UserContextEditor />);
    await waitFor(() => {
      expect(screen.getByText('userContext.empty')).toBeInTheDocument();
    });
  });

  it('lists files alphabetically with size hint', async () => {
    mockApi.list.mockResolvedValue([
      { name: 'about-me.md', size: 42 },
      { name: 'conventions.md', size: 100 },
    ]);
    render(<UserContextEditor />);
    await waitFor(() => {
      expect(screen.getByText('about-me.md')).toBeInTheDocument();
      expect(screen.getByText('conventions.md')).toBeInTheDocument();
      expect(screen.getByText('42 B')).toBeInTheDocument();
      expect(screen.getByText('100 B')).toBeInTheDocument();
    });
  });

  it('expands a row and loads the file content', async () => {
    mockApi.list.mockResolvedValue([{ name: 'about-me.md', size: 12 }]);
    mockApi.get.mockResolvedValue({ name: 'about-me.md', size: 12, content: '# Hello\n' });
    render(<UserContextEditor />);
    await waitFor(() => expect(screen.getByText('about-me.md')).toBeInTheDocument());
    fireEvent.click(screen.getByText('about-me.md'));
    await waitFor(() => {
      const ta = screen.getByLabelText(/userContext.editingAria/) as HTMLTextAreaElement;
      expect(ta.value).toBe('# Hello\n');
    });
    expect(mockApi.get).toHaveBeenCalledWith('about-me.md');
  });

  it('disables Save when content is unchanged (no-op detection)', async () => {
    mockApi.list.mockResolvedValue([{ name: 'about-me.md', size: 5 }]);
    mockApi.get.mockResolvedValue({ name: 'about-me.md', size: 5, content: 'hello' });
    render(<UserContextEditor />);
    await waitFor(() => expect(screen.getByText('about-me.md')).toBeInTheDocument());
    fireEvent.click(screen.getByText('about-me.md'));
    await waitFor(() => screen.getByLabelText(/userContext.editingAria/));
    const saveBtn = screen.getByText('userContext.save').closest('button')!;
    expect(saveBtn).toBeDisabled();
  });

  it('enables Save when content is edited and calls put', async () => {
    mockApi.list.mockResolvedValue([{ name: 'about-me.md', size: 5 }]);
    mockApi.get.mockResolvedValue({ name: 'about-me.md', size: 5, content: 'hello' });
    mockApi.put.mockResolvedValue({ name: 'about-me.md', size: 11, content: 'hello world' });
    render(<UserContextEditor />);
    await waitFor(() => expect(screen.getByText('about-me.md')).toBeInTheDocument());
    fireEvent.click(screen.getByText('about-me.md'));
    const ta = await waitFor(() => screen.getByLabelText(/userContext.editingAria/) as HTMLTextAreaElement);
    fireEvent.change(ta, { target: { value: 'hello world' } });
    const saveBtn = screen.getByText('userContext.save').closest('button')!;
    expect(saveBtn).not.toBeDisabled();
    fireEvent.click(saveBtn);
    await waitFor(() => {
      expect(mockApi.put).toHaveBeenCalledWith('about-me.md', 'hello world');
    });
  });

  it('creates a new file with .md auto-appended when missing', async () => {
    mockApi.list.mockResolvedValue([]);
    mockApi.put.mockResolvedValue({ name: 'foo.md', size: 8 });
    render(<UserContextEditor />);
    await waitFor(() => expect(screen.getByText('userContext.empty')).toBeInTheDocument());
    const input = screen.getByPlaceholderText('userContext.newNamePlaceholder') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'foo' } });
    fireEvent.click(screen.getByText('userContext.add').closest('button')!);
    await waitFor(() => {
      expect(mockApi.put).toHaveBeenCalledWith('foo.md', expect.stringContaining('# foo'));
    });
  });

  it('does not double-append .md if already present', async () => {
    mockApi.list.mockResolvedValue([]);
    mockApi.put.mockResolvedValue({ name: 'foo.md', size: 8 });
    render(<UserContextEditor />);
    await waitFor(() => expect(screen.getByText('userContext.empty')).toBeInTheDocument());
    const input = screen.getByPlaceholderText('userContext.newNamePlaceholder') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'foo.md' } });
    fireEvent.click(screen.getByText('userContext.add').closest('button')!);
    await waitFor(() => {
      const [name] = mockApi.put.mock.calls[0];
      expect(name).toBe('foo.md');
    });
  });

  it('calls delete after confirmation', async () => {
    mockApi.list.mockResolvedValue([{ name: 'about-me.md', size: 5 }]);
    mockApi.delete.mockResolvedValue(undefined);
    const original = window.confirm;
    window.confirm = vi.fn().mockReturnValue(true);
    try {
      render(<UserContextEditor />);
      await waitFor(() => expect(screen.getByText('about-me.md')).toBeInTheDocument());
      const delBtn = screen.getByLabelText(/userContext.deleteAria.*about-me.md/);
      fireEvent.click(delBtn);
      await waitFor(() => {
        expect(mockApi.delete).toHaveBeenCalledWith('about-me.md');
      });
    } finally {
      window.confirm = original;
    }
  });

  it('does NOT delete when confirm is cancelled', async () => {
    mockApi.list.mockResolvedValue([{ name: 'about-me.md', size: 5 }]);
    const original = window.confirm;
    window.confirm = vi.fn().mockReturnValue(false);
    try {
      render(<UserContextEditor />);
      await waitFor(() => expect(screen.getByText('about-me.md')).toBeInTheDocument());
      const delBtn = screen.getByLabelText(/userContext.deleteAria.*about-me.md/);
      fireEvent.click(delBtn);
      expect(mockApi.delete).not.toHaveBeenCalled();
    } finally {
      window.confirm = original;
    }
  });

  it('shows the error banner when list() fails', async () => {
    mockApi.list.mockRejectedValue(new Error('connection refused'));
    render(<UserContextEditor />);
    await waitFor(() => {
      expect(screen.getByText(/connection refused/)).toBeInTheDocument();
    });
  });

  it('shows the error banner when save fails and keeps the edit buffer', async () => {
    mockApi.list.mockResolvedValue([{ name: 'about-me.md', size: 5 }]);
    mockApi.get.mockResolvedValue({ name: 'about-me.md', size: 5, content: 'hello' });
    mockApi.put.mockRejectedValue(new Error('disk full'));
    render(<UserContextEditor />);
    await waitFor(() => expect(screen.getByText('about-me.md')).toBeInTheDocument());
    fireEvent.click(screen.getByText('about-me.md'));
    const ta = await waitFor(() => screen.getByLabelText(/userContext.editingAria/) as HTMLTextAreaElement);
    fireEvent.change(ta, { target: { value: 'hello updated' } });
    fireEvent.click(screen.getByText('userContext.save').closest('button')!);
    await waitFor(() => {
      expect(screen.getByText(/disk full/)).toBeInTheDocument();
    });
    // Buffer is preserved so the user doesn't lose work after a transient error.
    expect((screen.getByLabelText(/userContext.editingAria/) as HTMLTextAreaElement).value)
      .toBe('hello updated');
  });
});

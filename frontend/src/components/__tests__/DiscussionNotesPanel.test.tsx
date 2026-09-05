import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { notesMock, sendNoteMock, reviseNoteMock, deleteMessageMock } = vi.hoisted(() => ({
  notesMock: vi.fn(),
  sendNoteMock: vi.fn(),
  reviseNoteMock: vi.fn(),
  deleteMessageMock: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  discussions: {
    notes: notesMock,
    sendNote: sendNoteMock,
    reviseNote: reviseNoteMock,
    deleteMessage: deleteMessageMock,
  },
}));

import { DiscussionNotesPanel } from '../DiscussionNotesPanel';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}(${args.join(',')})` : key;

function note(id: string, content: string, sortOrder = 1) {
  return {
    sort_order: sortOrder,
    attachments: [],
    message: {
      id,
      role: 'User',
      channel: 'note',
      content,
      agent_type: null,
      timestamp: '2026-09-05T08:00:00Z',
      tokens_used: 0,
    },
  };
}

function page(notes: ReturnType<typeof note>[], nextCursor: number | null = null, total?: number) {
  return {
    discussion_id: 'd-1',
    total_notes: total ?? notes.length,
    notes,
    next_cursor: nextCursor,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  notesMock.mockResolvedValue(page([note('n-1', 'première note')]));
  sendNoteMock.mockResolvedValue(undefined);
  reviseNoteMock.mockResolvedValue('2026-09-05T09:00:00Z');
  deleteMessageMock.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
});

function renderPanel(discussionId = 'd-1') {
  return render(
    <DiscussionNotesPanel
      discussionId={discussionId}
      onClose={vi.fn()}
      toast={vi.fn()}
      t={t}
    />,
  );
}

describe('DiscussionNotesPanel', () => {
  /// KT-580 — the inversion this panel exists to fix: the route served agents
  /// and nothing on this side ever called it.
  it('lists the notes a human could not read back before', async () => {
    renderPanel();
    expect(await screen.findByText('première note')).toBeInTheDocument();
    expect(notesMock).toHaveBeenCalledWith('d-1');
  });

  it('writes a note from the panel, where the list already is', async () => {
    renderPanel();
    await screen.findByText('première note');

    fireEvent.change(screen.getByTestId('disc-note-draft'), { target: { value: '  une décision  ' } });
    fireEvent.click(screen.getByTestId('disc-note-add'));

    // Trimmed: trailing whitespace is not part of what was written.
    await waitFor(() => expect(sendNoteMock).toHaveBeenCalledWith('d-1', 'une décision'));
    // And the list is re-read, so the note appears without reopening the panel.
    await waitFor(() => expect(notesMock).toHaveBeenCalledTimes(2));
  });

  it('refuses to send an empty note', async () => {
    renderPanel();
    await screen.findByText('première note');

    fireEvent.change(screen.getByTestId('disc-note-draft'), { target: { value: '   ' } });
    expect(screen.getByTestId('disc-note-add')).toBeDisabled();
  });

  it('corrects a note in place', async () => {
    renderPanel();
    await screen.findByText('première note');

    fireEvent.click(screen.getByTestId('disc-note-edit-n-1'));
    fireEvent.change(screen.getByTestId('disc-note-edit-field'), { target: { value: 'note corrigée' } });
    fireEvent.click(screen.getByTestId('disc-note-save'));

    await waitFor(() => expect(reviseNoteMock).toHaveBeenCalledWith('d-1', 'n-1', 'note corrigée'));
  });

  /// Deletion reuses the tombstone endpoint: a note is a message, and Kronn
  /// already knew how to remove one without losing the trace.
  it('deletes through the existing tombstone, after asking', async () => {
    // happy-dom has no `confirm`, so it is provided rather than spied on.
    window.confirm = () => true;
    renderPanel();
    await screen.findByText('première note');

    fireEvent.click(screen.getByTestId('disc-note-delete-n-1'));
    await waitFor(() => expect(deleteMessageMock).toHaveBeenCalledWith('d-1', 'n-1'));
  });

  it('does not delete when the question is answered no', async () => {
    window.confirm = () => false;
    renderPanel();
    await screen.findByText('première note');

    fireEvent.click(screen.getByTestId('disc-note-delete-n-1'));
    expect(deleteMessageMock).not.toHaveBeenCalled();
  });

  /// A room accumulates notes for months. One request for all of them is a
  /// panel that opens slowly and then scrolls forever.
  it('loads the rest a page at a time', async () => {
    notesMock.mockResolvedValueOnce(page([note('n-1', 'première note', 1)], 1, 3));
    notesMock.mockResolvedValueOnce(page([note('n-2', 'deuxième', 2), note('n-3', 'troisième', 3)], null, 3));

    renderPanel();
    await screen.findByText('première note');
    // The header counts what exists, not what is on screen.
    expect(screen.getByText('3')).toBeInTheDocument();

    fireEvent.click(screen.getByTestId('disc-notes-more'));

    await waitFor(() => expect(screen.getByText('deuxième')).toBeInTheDocument());
    expect(screen.getByText('troisième')).toBeInTheDocument();
    // Appended, never replacing what was already read.
    expect(screen.getByText('première note')).toBeInTheDocument();
    expect(notesMock).toHaveBeenLastCalledWith('d-1', 1);
    expect(screen.queryByTestId('disc-notes-more')).toBeNull();
  });

  it('says so when the discussion has no note', async () => {
    notesMock.mockResolvedValue(page([]));
    renderPanel();
    expect(await screen.findByTestId('disc-notes-empty')).toBeInTheDocument();
  });

  /// Switching rooms must not show the previous room's notes while the new
  /// ones load — they would read as belonging to the room on screen.
  it('shows nothing from the previous discussion while the next loads', async () => {
    const { rerender } = renderPanel('d-1');
    await screen.findByText('première note');

    notesMock.mockImplementation(() => new Promise(() => {}));
    rerender(
      <DiscussionNotesPanel discussionId="d-2" onClose={vi.fn()} toast={vi.fn()} t={t} />,
    );

    expect(screen.queryByText('première note')).toBeNull();
  });

  it('reports a failure instead of looking empty', async () => {
    notesMock.mockRejectedValue(new Error('backend down'));
    renderPanel();
    expect(await screen.findByTestId('disc-notes-error')).toHaveTextContent('backend down');
    expect(screen.queryByTestId('disc-notes-empty')).toBeNull();
  });
});

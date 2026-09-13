import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ImportantPublicationStrip } from '../ImportantPublicationStrip';

const t = (key: string) => key;

describe('ImportantPublicationStrip', () => {
  it('creates a bounded information card with an optional existing task reference', async () => {
    const publish = vi.fn();
    const user = userEvent.setup();
    render(<ImportantPublicationStrip grant="test-grant" onGrantChange={vi.fn()} onPublish={publish}
      onOpenSettings={vi.fn()} tasks={[{ reference: 'KT-643', title: 'Important messages' }]} t={t} />);

    await user.click(screen.getByRole('button', { name: 'disc.important.openForm' }));
    await user.type(screen.getByLabelText('disc.important.contentLabel'), 'A clear update');
    await user.selectOptions(screen.getByLabelText('disc.important.taskLabel'), 'KT-643');
    await user.click(screen.getByRole('button', { name: 'disc.important.publish' }));

    const body = publish.mock.calls[0][0] as string;
    expect(body).toMatch(/^```kronn-important\n/);
    const spec = JSON.parse(body.split('\n')[1]);
    expect(spec.category).toBe('information');
    expect(spec.references.task_ref).toBe('KT-643');
    expect(spec.action_required).toEqual({ required: false });
  });

  it('does not expose a credential until the important-message action is opened', async () => {
    render(<ImportantPublicationStrip grant="" onGrantChange={vi.fn()} onPublish={vi.fn()}
      onOpenSettings={vi.fn()} tasks={[]} t={t} />);
    expect(screen.queryByLabelText('disc.important.grantLabel')).toBeNull();
  });
});

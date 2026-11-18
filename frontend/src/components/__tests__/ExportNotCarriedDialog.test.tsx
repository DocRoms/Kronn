import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { ExportNotCarriedDialog } from '../ExportNotCarriedDialog';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    locale: 'fr',
    t: (key: string, ...args: (string | number)[]) => args.length > 0 ? `${key}:${args.join(',')}` : key,
  }),
}));

const items = [
  { kind: 'skill', id: 'custom-gone', named_in: 'PR review › publish' },
  { kind: 'page', id: '0b6f6a52-5a0f-4a43-9c2e-0f5e6c0f7a11', named_in: 'PR review › fetch' },
  { kind: 'future_kind', id: 'x-1', named_in: 'Daily' },
];

const open = () => {
  const onDownload = vi.fn();
  const onCancel = vi.fn();
  render(<ExportNotCarriedDialog items={items} onDownload={onDownload} onCancel={onCancel} />);
  return { onDownload, onCancel };
};

describe('ExportNotCarriedDialog', () => {
  it('names each item by kind, id and place, and counts them in the title', () => {
    open();
    expect(screen.getByRole('dialog', { name: 'exp.notCarried.title:3' })).toBeInTheDocument();
    const rows = screen.getAllByTestId('export-not-carried-row');
    expect(rows.map(row => row.textContent)).toEqual([
      'exp.notCarried.kind.skillcustom-goneexp.notCarried.namedIn:PR review › publish',
      'exp.notCarried.kind.page0b6f6a52-5a0f-4a43-9c2e-0f5e6c0f7a11exp.notCarried.namedIn:PR review › fetch',
      // A kind this build does not know is shown as sent, not as a missing key.
      'future_kindx-1exp.notCarried.namedIn:Daily',
    ]);
  });

  it('downloads only from its own button', () => {
    const { onDownload, onCancel } = open();
    fireEvent.click(screen.getByTestId('export-not-carried-download'));
    expect(onDownload).toHaveBeenCalledTimes(1);
    expect(onCancel).not.toHaveBeenCalled();
  });

  it('cancels from the button, the cross, the backdrop and Escape, never from a click inside', () => {
    const { onDownload, onCancel } = open();
    fireEvent.click(screen.getByTestId('export-not-carried'));
    expect(onCancel).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'common.cancel' }));
    fireEvent.click(screen.getByRole('button', { name: 'common.close' }));
    fireEvent.click(screen.getByTestId('export-not-carried').parentElement!);
    fireEvent.keyDown(screen.getByTestId('export-not-carried'), { key: 'Escape' });
    expect(onCancel).toHaveBeenCalledTimes(4);
    expect(onDownload).not.toHaveBeenCalled();
  });
});

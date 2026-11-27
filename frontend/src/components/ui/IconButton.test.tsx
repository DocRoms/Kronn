// KT-1880 — IconButton: its accessible name is mandatory, by the type
// checker (the @ts-expect-error lines below fail `tsc -b` if the type ever
// accepts them), and it is shown as a tooltip on hover and keyboard focus.
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { X } from 'lucide-react';
import { IconButton } from './IconButton';

/** Never rendered: the type checker reads it. Each line must be a compile error. */
export function iconButtonsThatDoNotCompile(dynamicLabel: string) {
  return [
    // @ts-expect-error — no label: an IconButton without an accessible name does not compile.
    <IconButton key="a" icon={<X />} />,
    // @ts-expect-error — an empty label is no name.
    <IconButton key="b" label="" icon={<X />} />,
    // @ts-expect-error — aria-label is not a second way to name it.
    <IconButton key="c" label="Close" aria-label="Other" icon={<X />} />,
    // @ts-expect-error — nor aria-labelledby.
    <IconButton key="d" label="Close" aria-labelledby="elsewhere" icon={<X />} />,
    // @ts-expect-error — nor a native title tooltip.
    <IconButton key="f" label="Close" title="Close" icon={<X />} />,
    // A label computed at run time (a translation) compiles.
    <IconButton key="e" label={dynamicLabel} icon={<X />} />,
  ];
}

describe('IconButton', () => {
  it('is named by its label, the icon hidden from assistive technology', () => {
    render(<IconButton label="Close the panel" icon={<X data-testid="icon" />} />);
    const button = screen.getByRole('button', { name: 'Close the panel' });
    expect(button).toHaveAttribute('aria-label', 'Close the panel');
    expect(button).toHaveAttribute('type', 'button');
    expect(button).not.toHaveAttribute('title');
    expect(screen.getByTestId('icon').closest('[aria-hidden="true"]')).toBeTruthy();
  });

  it('defaults to ghost at size md, and takes the other variants and sizes', () => {
    const { rerender } = render(<IconButton label="Delete" icon={<X />} />);
    expect(screen.getByRole('button')).toHaveAttribute('data-variant', 'ghost');
    expect(screen.getByRole('button')).toHaveAttribute('data-size', 'md');
    rerender(<IconButton label="Delete" icon={<X />} variant="danger" size="sm" />);
    expect(screen.getByRole('button')).toHaveAttribute('data-variant', 'danger');
    expect(screen.getByRole('button')).toHaveAttribute('data-size', 'sm');
  });

  it('shows its label as a tooltip on hover, not wired as a duplicate description', async () => {
    const user = userEvent.setup();
    render(<IconButton label="Copy the id" icon={<X />} />);
    await user.hover(screen.getByRole('button'));
    const tip = await screen.findByText('Copy the id', { selector: '[role="tooltip"]' });
    expect(tip).toHaveAttribute('aria-hidden', 'true');
    expect(screen.getByRole('button')).not.toHaveAttribute('aria-describedby');
  });

  it('shows the tooltip when reached with Tab', async () => {
    const user = userEvent.setup();
    render(<IconButton label="Copy the id" icon={<X />} />);
    await user.tab();
    expect(screen.getByRole('button')).toHaveFocus();
    expect(await screen.findByText('Copy the id', { selector: '[role="tooltip"]' })).toBeInTheDocument();
  });

  it('can go without the tooltip, and runs on click', async () => {
    const onClick = vi.fn();
    render(<IconButton label="Clear" icon={<X />} tooltip={false} onClick={onClick} />);
    await userEvent.hover(screen.getByRole('button'));
    expect(document.querySelector('[role="tooltip"]')).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});

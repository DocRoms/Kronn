// KT-1880 — Button: four variants, three sizes, a real <button> that never
// submits a form by accident, and an icon that is not part of its name.
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createRef } from 'react';
import { Button, type ButtonSize, type ButtonVariant } from './Button';

describe('Button', () => {
  it('is a button named by its label, of type button by default', () => {
    render(<Button>Save the plan</Button>);
    const button = screen.getByRole('button', { name: 'Save the plan' });
    expect(button).toHaveAttribute('type', 'button');
    expect(button).toHaveAttribute('data-variant', 'neutral');
    expect(button).toHaveAttribute('data-size', 'md');
  });

  it.each<[ButtonVariant, ButtonSize]>([
    ['primary', 'sm'], ['neutral', 'md'], ['ghost', 'lg'], ['danger', 'sm'],
  ])('renders the %s variant at size %s', (variant, size) => {
    render(<Button variant={variant} size={size}>Go</Button>);
    const button = screen.getByRole('button', { name: 'Go' });
    expect(button).toHaveAttribute('data-variant', variant);
    expect(button).toHaveAttribute('data-size', size);
  });

  it('does not submit the form it sits in unless asked', async () => {
    const onSubmit = vi.fn((e: Event) => e.preventDefault());
    const { container } = render(
      <form onSubmit={e => onSubmit(e.nativeEvent)}>
        <Button>Plain</Button>
        <Button type="submit">Send</Button>
      </form>,
    );
    expect(container.querySelector('form')).toBeTruthy();
    await userEvent.click(screen.getByRole('button', { name: 'Plain' }));
    expect(onSubmit).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Send' }));
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });

  it('runs on click, Enter and Space, and not when disabled', async () => {
    const onClick = vi.fn();
    const { rerender } = render(<Button onClick={onClick}>Run</Button>);
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Run' }));
    screen.getByRole('button', { name: 'Run' }).focus();
    await user.keyboard('{Enter}');
    await user.keyboard(' ');
    expect(onClick).toHaveBeenCalledTimes(3);

    rerender(<Button onClick={onClick} disabled>Run</Button>);
    await user.click(screen.getByRole('button', { name: 'Run' }));
    expect(onClick).toHaveBeenCalledTimes(3);
  });

  it('keeps the icon out of the accessible name', () => {
    render(<Button icon={<svg data-testid="icon" />}>Copy</Button>);
    expect(screen.getByRole('button', { name: 'Copy' })).toBeInTheDocument();
    expect(screen.getByTestId('icon').parentElement).toHaveAttribute('aria-hidden', 'true');
  });

  it('passes the ref and the native attributes through, and adds a class', () => {
    const ref = createRef<HTMLButtonElement>();
    render(<Button ref={ref} className="extra" aria-pressed="true" data-testid="b">On</Button>);
    expect(ref.current).toBe(screen.getByTestId('b'));
    expect(ref.current).toHaveClass('ui-button', 'extra');
    expect(ref.current).toHaveAttribute('aria-pressed', 'true');
  });
});

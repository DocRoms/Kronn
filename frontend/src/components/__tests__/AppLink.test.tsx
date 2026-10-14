import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AppLink } from '../AppLink';

afterEach(cleanup);

describe('AppLink', () => {
  it('is a real link to the address, with the attributes it is given', () => {
    render(<AppLink to="/discussions/d-1" className="row" aria-label="Open it">Open</AppLink>);
    const link = screen.getByRole('link', { name: 'Open it' });
    expect(link).toHaveAttribute('href', '/discussions/d-1');
    expect(link).toHaveClass('row');
    expect(link).toHaveTextContent('Open');
  });

  it('follows a plain click in the app, not through a page load', () => {
    const onNavigate = vi.fn();
    render(<AppLink to="/discussions/d-1" onNavigate={onNavigate}>Open</AppLink>);
    const link = screen.getByRole('link');

    const click = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 });
    link.dispatchEvent(click);

    expect(onNavigate).toHaveBeenCalledTimes(1);
    expect(click.defaultPrevented).toBe(true);
  });

  it.each([
    ['Ctrl-click', { button: 0, ctrlKey: true }],
    ['Cmd-click', { button: 0, metaKey: true }],
    ['Shift-click', { button: 0, shiftKey: true }],
    ['Alt-click', { button: 0, altKey: true }],
    ['middle click', { button: 1 }],
  ])('leaves a %s to the browser: a new tab, this one where it is', (_name, init) => {
    const onNavigate = vi.fn();
    render(<AppLink to="/discussions/d-1" onNavigate={onNavigate}>Open</AppLink>);
    const link = screen.getByRole('link');

    const click = new MouseEvent('click', { bubbles: true, cancelable: true, ...init });
    link.dispatchEvent(click);

    expect(onNavigate).not.toHaveBeenCalled();
    expect(click.defaultPrevented).toBe(false);
  });

  it('lets the browser follow the link when nothing handles the click in the app', () => {
    render(<AppLink to="/discussions/d-1">Open</AppLink>);
    const click = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 });

    screen.getByRole('link').dispatchEvent(click);

    expect(click.defaultPrevented).toBe(false);
  });
});

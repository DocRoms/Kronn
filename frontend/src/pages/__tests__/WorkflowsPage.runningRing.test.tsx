// Which automation is running, told on the card.
//
// Kronn already knew: `ActiveRunsPopover` lists the active runs. But it is a
// popover you have to open, so the sidebar answered "something is running" and
// never "which one" — which is the question actually being asked.

import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { isWorkflowRunning } from '../../lib/runFilters';

const CSS = readFileSync('src/pages/WorkflowsPage.css', 'utf8');

describe('the running ring on an automation card', () => {
  it('counts a claimed run as running, not only a streaming one', () => {
    // A run that is Pending is already the answer to "which one is going?".
    // Excluding it would leave the card blank for exactly the seconds the
    // operator is looking at it.
    expect(isWorkflowRunning('Running')).toBe(true);
    expect(isWorkflowRunning('Pending')).toBe(true);

    expect(isWorkflowRunning('Success')).toBe(false);
    expect(isWorkflowRunning('Failed')).toBe(false);
    expect(isWorkflowRunning('Cancelled')).toBe(false);
    // A workflow that never ran has no last run at all.
    expect(isWorkflowRunning(null)).toBe(false);
    expect(isWorkflowRunning(undefined)).toBe(false);
  });

  it('draws the ring around the icon rather than replacing it', () => {
    // Replacing the icon with a spinner would hide the one thing that
    // identifies the card — the running one would become the only card you
    // could no longer recognise. The ring is a separate layer for the same
    // reason the icon cannot simply be rotated: it may be a circle.
    expect(CSS).toMatch(/\.automation-resource-icon\[data-running\]::after/);
    expect(CSS).toMatch(/animation: spin/);
  });

  it('keeps the state when motion is reduced', () => {
    // The state matters, the motion does not. Dropping the ring under reduced
    // motion would take the information away with the animation.
    const reduced = CSS.slice(CSS.indexOf('@media (prefers-reduced-motion: reduce)'));
    expect(reduced).toMatch(/animation: none/);
    expect(reduced).toMatch(/border-color: var\(--kr-accent\)/);
  });

  it('gives a screen reader something to read', () => {
    // The icon is aria-hidden, so the ring announces nothing on its own.
    expect(CSS).toMatch(/\.automation-resource-running-label/);
    expect(CSS).toMatch(/clip-path: inset\(50%\)/);
  });
});

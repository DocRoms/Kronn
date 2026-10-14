import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { Outlet } from 'react-router';
import { RouterProvider } from 'react-router/dom';
import { afterEach, describe, expect, it, vi } from 'vitest';

const app = vi.hoisted(() => ({ crash: false }));

// The root route under test is `App`; a stand-in keeps the real one, and the
// backend it boots against, out of a routing test.
vi.mock('../App', () => ({
  App: () => {
    if (app.crash) throw new Error('shell exploded');
    return <div data-testid="app"><Outlet context={{ pagesCapability: null }} /></div>;
  },
}));

// The dashboard layout and its pages are not under test either: a stub that
// keeps the outlet so the route table underneath still resolves.
vi.mock('../routes/DashboardLayout', () => ({
  DashboardLayout: () => <div data-testid="layout"><Outlet /></div>,
}));

vi.mock('../pages/StandaloneLivePage', () => ({
  StandaloneLivePage: ({ pageId }: { pageId: string }) => <div data-testid="standalone-page">{pageId}</div>,
}));

import { createAppRouter } from '../router';

type AppRouter = ReturnType<typeof createAppRouter>;
let router: AppRouter | null = null;

async function boot(path: string) {
  window.history.replaceState(null, '', path);
  router = createAppRouter();
  const mounted = router;
  await act(async () => { render(<RouterProvider router={mounted} useTransitions={false} />); });
}

afterEach(() => {
  cleanup();
  router?.dispose();
  router = null;
  app.crash = false;
  window.history.replaceState(null, '', '/');
  vi.restoreAllMocks();
});

describe('app router', () => {
  it('renders the app shell at the root and sends the bare address to Projects', async () => {
    await boot('/');

    expect(screen.getByTestId('app')).toBeInTheDocument();
    await waitFor(() => expect(window.location.pathname).toBe('/projects'));
    expect(screen.getByTestId('layout')).toBeInTheDocument();
  });

  it('keeps the app shell mounted on a deep address', async () => {
    // `/pages` renders nothing while its capability is unknown: the address
    // must be left alone, with the shell around it.
    await boot('/pages');

    expect(screen.getByTestId('app')).toBeInTheDocument();
    expect(window.location.pathname).toBe('/pages');
  });

  it('keeps the app shell on an address that names no page', async () => {
    await boot('/nope');

    expect(screen.getByTestId('app')).toBeInTheDocument();
    await waitFor(() => expect(window.location.pathname).toBe('/projects'));
  });

  it('renders a whole-window view without the dashboard layout', async () => {
    await boot('/standalone/pages/page-1');

    expect(screen.getByTestId('app')).toBeInTheDocument();
    expect(await screen.findByTestId('standalone-page')).toHaveTextContent('page-1');
    expect(screen.queryByTestId('layout')).not.toBeInTheDocument();
    expect(window.location.pathname).toBe('/standalone/pages/page-1');
  });

  it('shows the app error screen, not the router developer page, when the shell crashes', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    app.crash = true;

    await boot('/projects');

    expect(screen.getByText('shell exploded')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Reload' })).toBeInTheDocument();
    expect(screen.queryByText(/Unexpected Application Error/)).not.toBeInTheDocument();
  });
});

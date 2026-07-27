import { createMemoryRouter, RouterProvider } from 'react-router';

export function TestRouter({ children, initialPath = '/projects' }: { children: React.ReactNode; initialPath?: string }) {
  const router = createMemoryRouter(
    [{ path: '*', element: children }],
    { initialEntries: [initialPath] },
  );
  return <RouterProvider router={router} />;
}

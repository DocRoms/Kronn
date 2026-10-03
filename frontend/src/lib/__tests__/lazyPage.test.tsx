import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { Suspense } from 'react';
import { lazyPage } from '../lazyPage';

function Hello({ name }: { name: string }) {
  return <p>Hello {name}</p>;
}

describe('lazyPage', () => {
  it('renders a preloaded page without showing the fallback', async () => {
    const Page = lazyPage(() => Promise.resolve(Hello));
    await Page.preload();
    render(<Suspense fallback={<p>fallback</p>}><Page name="Romu" /></Suspense>);
    expect(screen.getByText('Hello Romu')).toBeInTheDocument();
    expect(screen.queryByText('fallback')).toBeNull();
  });

  it('still loads on demand when it was not preloaded', async () => {
    const Page = lazyPage(() => Promise.resolve(Hello));
    render(<Suspense fallback={<p>fallback</p>}><Page name="Romu" /></Suspense>);
    expect(await screen.findByText('Hello Romu')).toBeInTheDocument();
  });

  it('fetches the chunk once, and again after a failure', async () => {
    const load = vi.fn()
      .mockRejectedValueOnce(new Error('chunk failed'))
      .mockResolvedValue(Hello);
    const Page = lazyPage(load);
    await expect(Page.preload()).rejects.toThrow('chunk failed');
    await Page.preload();
    await Page.preload();
    expect(load).toHaveBeenCalledTimes(2);
  });
});

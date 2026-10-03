import { describe, expect, it } from 'vitest';
import type { UserConfig } from 'vite';
import viteConfig from './vite.config';

describe('Vite development proxy', () => {
  it('keeps backend API connections alive', () => {
    const proxy = (viteConfig as UserConfig).server?.proxy?.['/api'];
    expect(proxy).toBeTypeOf('object');
    if (!proxy || typeof proxy === 'string') throw new Error('Missing /api proxy options');

    const agent = proxy.agent as { options?: { keepAlive?: boolean } };
    expect(agent.options?.keepAlive).toBe(true);
  });
});

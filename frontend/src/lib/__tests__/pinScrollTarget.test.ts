import { describe, expect, it, vi } from 'vitest';
import { pinScrollTarget } from '../pinScrollTarget';

const rect = (top: number, height: number) => ({
  top, bottom: top + height, left: 0, right: 100, width: 100, height, x: 0, y: top, toJSON: () => ({}),
}) as DOMRect;
const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

describe('pinScrollTarget', () => {
  it('re-centres a target that keeps moving until the window ends', async () => {
    const container = document.createElement('div');
    const target = document.createElement('div');
    let offset = 1000;
    container.getBoundingClientRect = () => rect(0, 400);
    target.getBoundingClientRect = () => rect(offset - container.scrollTop, 20);
    const onSettled = vi.fn();
    pinScrollTarget(container, () => target, { settleMs: 150, onSettled });
    await wait(40);
    offset = 3000;
    await wait(250);
    expect(onSettled).toHaveBeenCalledTimes(1);
    expect(container.scrollTop).toBe(3000 + 10 - 200);
  });

  it('gives up when the target never renders', async () => {
    const container = document.createElement('div');
    const onMissing = vi.fn();
    pinScrollTarget(container, () => null, { lookupMs: 50, onMissing });
    await wait(150);
    expect(onMissing).toHaveBeenCalledTimes(1);
  });

  it('stops as soon as the user scrolls', async () => {
    const container = document.createElement('div');
    const target = document.createElement('div');
    container.getBoundingClientRect = () => rect(0, 400);
    target.getBoundingClientRect = () => rect(1000 - container.scrollTop, 20);
    const onAbort = vi.fn();
    const onSettled = vi.fn();
    pinScrollTarget(container, () => target, { settleMs: 200, onAbort, onSettled });
    container.dispatchEvent(new Event('wheel'));
    await wait(300);
    expect(onAbort).toHaveBeenCalledTimes(1);
    expect(onSettled).not.toHaveBeenCalled();
    expect(container.scrollTop).toBe(0);
  });
});

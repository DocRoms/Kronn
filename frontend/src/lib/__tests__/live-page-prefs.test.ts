import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { Window as HappyWindow } from 'happy-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  buildSandboxDocument,
  createLivePageOpenLinkRelay,
  readLivePagePrefs,
} from '../live-page-sandbox';

// KT-1030 — the opaque sandbox has no storage: a Page asks the host, through
// its private port, to remember one display flag; the host hands it back.
const BOARD = readFileSync(
  resolve(__dirname, '../../../../backend/src/core/default_todo/board.html'),
  'utf8',
).replace('__KRONN_LANG__', 'en');


async function openFrame(relayPageId: string, channel = 'channel-1') {
  const { Window } = await import('happy-dom');
  const frame: HappyWindow = new Window();
  // As in `sandbox="allow-scripts"`: any storage access throws.
  Object.defineProperty(frame, 'localStorage', { configurable: true, get: () => { throw new Error('SecurityError'); } });
  // happy-dom's own MessagePort cannot post on the host's transferred port; a
  // browser can. Give the frame's prototype the host behaviour before the bridge captures it.
  const hostPort = Object.getPrototypeOf(new MessageChannel().port1) as MessagePort;
  Object.assign(frame, { __hostPost: hostPort.postMessage, __hostStart: hostPort.start });
  (frame as unknown as { eval: (code: string) => void }).eval(
    'MessagePort.prototype.postMessage=function(){return __hostPost.apply(this,arguments)};MessagePort.prototype.start=function(){return __hostStart.apply(this,arguments)};',
  );
  frame.document.write(buildSandboxDocument(BOARD, channel));
  for (const script of Array.from(frame.document.querySelectorAll('script:not([type])'))) {
    (frame as unknown as { eval: (code: string) => void }).eval(script.textContent ?? '');
  }
  const relay = createLivePageOpenLinkRelay(channel, { pageId: () => relayPageId });
  relay.connect({
    postMessage: (data: unknown, _origin: string, ports: MessagePort[]) => {
      frame.dispatchEvent(new frame.MessageEvent('message', { data, ports } as never));
    },
  } as unknown as Window);
  const publish = (pageId: string) => frame.dispatchEvent(new frame.MessageEvent('message', { data: {
    type: 'kronn:page-data', version: 1, channel_id: channel,
    data: { version: 1, page: { id: pageId, slug: 'todo', title: 'Todo', data_revision: 1 }, datasets: { todo: { kind: 'snapshot', current: [], points: [] } }, prefs: readLivePagePrefs(pageId) },
  } }));
  const notice = () => frame.document.getElementById('avis') as unknown as HTMLElement;
  return { frame, relay, publish, notice };
}

beforeEach(() => window.localStorage.clear());
afterEach(() => vi.restoreAllMocks());

describe('Page preferences kept by the host', () => {
  it('remembers a dismissed notice across a rebuilt sandbox document', async () => {
    const first = await openFrame('page-1');
    first.publish('page-1');
    expect(first.notice().hidden).toBe(false);
    (first.frame.document.getElementById('avis-fermer') as unknown as HTMLButtonElement).click();
    expect(first.notice().hidden).toBe(true);
    await vi.waitFor(() => expect(readLivePagePrefs('page-1')).toEqual({ 'notice-dismissed': true }));
    first.relay.dispose();
    await first.frame.happyDOM.close();

    const again = await openFrame('page-1');
    again.publish('page-1');
    expect(again.notice().hidden).toBe(true);
    // Another Page keeps its own notice.
    again.publish('page-2');
    expect(again.notice().hidden).toBe(false);
    again.relay.dispose();
    await again.frame.happyDOM.close();
  });

  it('ignores an unknown key, a non-boolean value and another Page’s message', async () => {
    const { frame, relay, publish } = await openFrame('page-1');
    publish('page-1');
    const pref = (frame as unknown as { KronnPagePref: (key: unknown, value: unknown) => boolean }).KronnPagePref;
    expect(pref('theme', true)).toBe(false);
    expect(pref('notice-dismissed', 'yes')).toBe(false);
    expect(readLivePagePrefs('page-1')).toEqual({});

    // Forged straight onto the port: the host re-checks everything itself.
    const postMessage = vi.fn();
    const host = createLivePageOpenLinkRelay('channel-1', { pageId: () => 'page-1' });
    host.connect({ postMessage } as unknown as Window);
    const port = (postMessage.mock.calls[0][2] as MessagePort[])[0];
    const valid = { type: 'kronn:page-pref', version: 1, channel_id: 'channel-1', page_id: 'page-1', key: 'notice-dismissed', value: true };
    port.postMessage({ ...valid, key: 'theme' });
    port.postMessage({ ...valid, value: 'true' });
    port.postMessage({ ...valid, page_id: 'page-2' });
    port.postMessage({ ...valid, channel_id: 'forged' });
    await new Promise(done => setTimeout(done, 20));
    expect(readLivePagePrefs('page-1')).toEqual({});
    expect(readLivePagePrefs('page-2')).toEqual({});
    port.postMessage(valid);
    await vi.waitFor(() => expect(readLivePagePrefs('page-1')).toEqual({ 'notice-dismissed': true }));
    host.dispose();
    relay.dispose();
    await frame.happyDOM.close();
  });

  it('reads nothing when the host’s storage is refused', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('denied'); });
    expect(readLivePagePrefs('page-1')).toEqual({});
  });
});

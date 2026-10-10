import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

// KT-1030 — the board Kronn ships as a Live Page, run as the sandbox runs it:
// its HTML in the document, its inline script evaluated once.
const BOARD = readFileSync(
  resolve(__dirname, '../../../../backend/src/core/default_todo/board.html'),
  'utf8',
).replace('__KRONN_LANG__', 'en');

type Row = { id: string; ref?: string; title: string; column: string; description?: string; tags?: string[]; sentinel?: boolean; priority?: string; updated_at?: string; created_at?: string; discussion_id?: string };

const markers: Row[] = ['todo', 'in_progress', 'done'].map(c => ({ id: `__col_${c}__`, title: '', column: c, sentinel: true }));
let launched: string[] = [];
const recordLaunch = (event: Event) => {
  const action = (event.target as Element | null)?.closest?.('[data-kronn-action]');
  if (action) launched.push(action.getAttribute('data-kronn-action') ?? '');
};

function mount(rows: Row[]) {
  const parsed = new DOMParser().parseFromString(BOARD, 'text/html');
  const script = Array.from(parsed.querySelectorAll('script')).find(s => !s.getAttribute('type'));
  document.head.innerHTML = parsed.head.innerHTML;
  document.body.innerHTML = parsed.body.innerHTML.replace(script!.outerHTML, '');
  Object.defineProperty(window, 'KronnPageData', { configurable: true, value: { datasets: { todo: { current: [...rows, ...markers] } } } });
  new Function(script!.textContent ?? '')();
}

const flush = () => new Promise(resolveFlush => setTimeout(resolveFlush, 0));
const titlesIn = (column: string) => Array.from(document.querySelectorAll(`.col-${column} .carte .titre`)).map(el => el.textContent);
const markdown = (source: string): string => (window as unknown as { KronnTodoMarkdown: (s: string) => string }).KronnTodoMarkdown(source);

function drag(id: string, column: string) {
  const card = document.querySelector(`li[data-id="${id}"]`)!;
  card.dispatchEvent(new Event('dragstart', { bubbles: true }));
  document.querySelector(`.colonne.col-${column}`)!.dispatchEvent(new Event('drop', { bubbles: true, cancelable: true }));
}

function publish(rows: Row[]) {
  window.dispatchEvent(new CustomEvent('kronn:page-data', { detail: { datasets: { todo: { current: [...rows, ...markers] } } } }));
}

const A: Row = { id: 'a', ref: 'KT-1', title: 'Write the doc', column: 'todo' };
const B: Row = { id: 'b', ref: 'KT-2', title: 'Ship it', column: 'todo' };

beforeEach(() => {
  window.localStorage.clear();
  launched = [];
  document.addEventListener('click', recordLaunch, true);
});
afterEach(() => {
  document.removeEventListener('click', recordLaunch, true);
  delete (window as unknown as { __pwned?: unknown }).__pwned;
});

describe('the Markdown of a card', () => {
  it('renders headings, lists, emphasis, code and http(s) links', () => {
    mount([]);
    const html = markdown('# Title\n\n- one\n- **two**\n\n1. first\n\nSome *soft* and `code`.\n\n```\nlet x = 1 < 2;\n```\n\n[docs](https://example.com/a?b=1&c=2)');
    const root = document.createElement('div');
    root.innerHTML = html;
    expect(root.querySelector('h1')?.textContent).toBe('Title');
    expect(Array.from(root.querySelectorAll('ul li')).map(li => li.textContent)).toEqual(['one', 'two']);
    expect(root.querySelector('ul strong')?.textContent).toBe('two');
    expect(root.querySelector('ol li')?.textContent).toBe('first');
    expect(root.querySelector('p em')?.textContent).toBe('soft');
    expect(root.querySelector('p code')?.textContent).toBe('code');
    expect(root.querySelector('pre code')?.textContent).toBe('let x = 1 < 2;');
    const link = root.querySelector('a')!;
    expect(link.getAttribute('href')).toBe('https://example.com/a?b=1&c=2');
    expect(link.getAttribute('target')).toBe('_blank');
    expect(link.getAttribute('rel')).toContain('noopener');
  });

  it('never renders raw HTML, scripts or script links', () => {
    mount([]);
    const root = document.createElement('div');
    root.innerHTML = markdown('<img src=x onerror="window.__pwned=1">\n<script>window.__pwned=2</script>\n[bad](javascript:alert(1)) [worse](JAVASCRIPT:alert(1)) [data](data:text/html,x)\n"><b onmouseover=x>');
    expect(root.querySelector('img, script, b')).toBeNull();
    expect(root.querySelectorAll('a')).toHaveLength(0);
    expect(root.textContent).toContain('<img src=x onerror="window.__pwned=1">');
    expect(root.textContent).toContain('bad');
  });
});

describe('a card’s details', () => {
  it('are collapsed, render the Markdown safely when opened, and launch nothing', async () => {
    mount([{ ...A, description: '## Why\n- users\n<img src=x onerror="window.__pwned=1"><script>window.__pwned=2</script>\n[x](javascript:alert(1)) [ok](https://example.com)' }]);
    expect(document.querySelector('.details')).toBeNull();
    const chevron = document.querySelector<HTMLButtonElement>('[data-details="a"]')!;
    expect(chevron.getAttribute('aria-expanded')).toBe('false');
    chevron.click();
    await flush();
    const details = document.querySelector('.details')!;
    expect(details.querySelector('h2')?.textContent).toBe('Why');
    expect(details.querySelector('ul li')?.textContent).toBe('users');
    expect(details.querySelector('img, script')).toBeNull();
    expect(Array.from(details.querySelectorAll('a')).map(a => a.getAttribute('href'))).toEqual(['https://example.com']);
    expect((window as unknown as { __pwned?: unknown }).__pwned).toBeUndefined();
    expect(document.querySelector('[data-details="a"]')!.getAttribute('aria-expanded')).toBe('true');
    expect(launched).toEqual([]);
  });
});

describe('a drag and drop', () => {
  it('moves the card at once and clicks the move action inside the gesture', () => {
    mount([A, B]);
    expect(titlesIn('todo')).toEqual(['Write the doc', 'Ship it']);
    drag('a', 'in_progress');
    expect(titlesIn('in_progress')).toEqual(['Write the doc']);
    expect(titlesIn('todo')).toEqual(['Ship it']);
    expect(launched).toEqual(['todo-move']);
    const confirm = document.querySelector('.carte.en-attente .placer')!;
    expect(JSON.parse(confirm.getAttribute('data-kronn-bindings')!)).toEqual({ task: 'a', before: '__col_in_progress__', column: '__col_in_progress__' });
  });

  it('is confirmed by the published board', async () => {
    mount([A, B]);
    drag('a', 'in_progress');
    publish([B, { ...A, column: 'in_progress' }]);
    await flush();
    expect(document.querySelector('.en-attente')).toBeNull();
    expect(titlesIn('in_progress')).toEqual(['Write the doc']);
  });

  it('rolls back with the reason when its launch fails', async () => {
    mount([A, B]);
    drag('a', 'in_progress');
    await flush();
    const confirm = document.querySelector('.carte.en-attente .placer')!;
    confirm.setAttribute('data-kronn-action-launch', 'launch-1');
    confirm.setAttribute('data-kronn-action-state', 'launching');
    await flush();
    confirm.setAttribute('data-kronn-action-state', 'failed');
    await flush();
    expect(titlesIn('todo')).toEqual(['Write the doc', 'Ship it']);
    expect(titlesIn('in_progress')).toEqual([]);
    expect(document.getElementById('statut')!.textContent).toContain('Move not saved');
  });

  it('ignores the state an older launch left on the same button', async () => {
    mount([A, B]);
    drag('a', 'in_progress');
    const confirm = document.querySelector('.carte.en-attente .placer')!;
    // Marked by Kronn right after the render: an earlier, identical move.
    confirm.setAttribute('data-kronn-action-launch', 'old');
    confirm.setAttribute('data-kronn-action-state', 'failed');
    await flush();
    expect(titlesIn('in_progress')).toEqual(['Write the doc']);
    expect(document.getElementById('statut')!.textContent).toBe('');
  });

  it('can be cancelled while it waits for its card', async () => {
    mount([A, B]);
    drag('a', 'in_progress');
    document.querySelector<HTMLButtonElement>('[data-cancel]')!.click();
    await flush();
    expect(titlesIn('todo')).toEqual(['Write the doc', 'Ship it']);
    expect(launched).toEqual(['todo-move']);
  });
});

describe('the first-view notice', () => {
  const notice = () => document.getElementById('avis') as HTMLElement;

  it('shows once, with the trust hint, and stays closed after a reload', async () => {
    mount([A]);
    expect(notice().hidden).toBe(false);
    expect(notice().textContent).toContain('Kronn’s Tasks and Workflows');
    expect(notice().textContent).toContain('todo-move');
    document.querySelector<HTMLButtonElement>('#avis-fermer')!.click();
    await flush();
    expect(notice().hidden).toBe(true);
    expect(launched).toEqual([]);
    mount([A]);
    expect(notice().hidden).toBe(true);
  });

  it('still renders the board, notice shown, when storage throws', () => {
    // An opaque sandbox origin refuses storage outright.
    const original = Object.getOwnPropertyDescriptor(window, 'localStorage');
    Object.defineProperty(window, 'localStorage', { configurable: true, get: () => { throw new Error('SecurityError'); } });
    try {
      mount([A, B]);
      expect(notice().hidden).toBe(false);
      expect(titlesIn('todo')).toEqual(['Write the doc', 'Ship it']);
      document.querySelector<HTMLButtonElement>('#avis-fermer')!.click();
      expect(notice().hidden).toBe(true);
      mount([A]);
      expect(notice().hidden).toBe(false);
    } finally {
      if (original) Object.defineProperty(window, 'localStorage', original);
      else Reflect.deleteProperty(window, 'localStorage');
    }
  });
});

describe('a card', () => {
  it('shows its tags as chips, priority, relative date, preview and discussion link', () => {
    const recent = new Date(Date.now() - 3 * 3600 * 1000).toISOString();
    mount([
      { ...A, tags: ['front', 'ux'], priority: 'high', updated_at: recent, description: '## Why it matters\n- users', discussion_id: 'd-1' },
      { ...B, priority: 'normal' },
    ]);
    const first = document.querySelector('li[data-id="a"]')!;
    expect(Array.from(first.querySelectorAll('.tags .tag')).map(t => t.textContent)).toEqual(['#front', '#ux']);
    expect(first.querySelector('.prio')?.textContent).toBe('High');
    expect(first.querySelector('time')?.textContent).toBe('3 hours ago');
    expect(first.querySelector('.apercu')?.textContent).toBe('Why it matters');
    expect(first.querySelector('.lien-disc')?.getAttribute('href')).toBe('#discussion-d-1');
    expect(first.querySelector('[data-kronn-action="todo-discuss"]')).toBeNull();
    const second = document.querySelector('li[data-id="b"]')!;
    expect(second.querySelector('.prio')).toBeNull();
    expect(second.querySelector('.tags')).toBeNull();
    expect(second.querySelector('[data-kronn-action="todo-discuss"]')).not.toBeNull();
  });

  it('escapes a tag', () => {
    mount([{ ...A, tags: ['<img src=x onerror="window.__pwned=1">'] }]);
    expect(document.querySelector('.tags img')).toBeNull();
    expect(document.querySelector('.tags .tag')?.textContent).toBe('#<img src=x onerror="window.__pwned=1">');
  });

  it('leaves an empty column with a friendly hint', () => {
    mount([A]);
    expect(document.querySelector('.col-in_progress .vide strong')?.textContent).toBe('Nothing here yet');
    expect(document.querySelector('.col-done .vide strong')?.textContent).toBe('Nothing here yet');
  });
});

describe('the search', () => {
  const C: Row = { id: 'c', ref: 'KT-3', title: 'Réunion équipe', column: 'in_progress', tags: ['meeting'] };
  const type = (value: string) => {
    const input = document.getElementById('recherche') as HTMLInputElement;
    input.value = value;
    input.dispatchEvent(new Event('input', { bubbles: true }));
  };
  const all = () => Array.from(document.querySelectorAll('.carte .titre')).map(el => el.textContent);

  it('matches title, description and tags, ignoring case and accents, and counts per column', () => {
    mount([A, { ...B, description: 'Release **notes**' }, C]);
    type('SHIP');
    expect(all()).toEqual(['Ship it']);
    expect(document.querySelector('[data-count="todo"]')?.textContent).toBe('1 of 2');
    expect(document.querySelector('[data-count="in_progress"]')?.textContent).toBe('0 of 1');
    expect(document.querySelector('.col-in_progress .vide')?.textContent).toBe('No card matches the search.');
    type('notes');
    expect(all()).toEqual(['Ship it']);
    type('meeting');
    expect(all()).toEqual(['Réunion équipe']);
    type('reunion EQUIPE');
    expect(all()).toEqual(['Réunion équipe']);
    expect(launched).toEqual([]);
  });

  it('is cleared by its × and by a tag chip sets it', async () => {
    mount([A, B, C]);
    type('ship');
    const clear = document.getElementById('recherche-vider') as HTMLButtonElement;
    expect(clear.hidden).toBe(false);
    clear.click();
    expect(all()).toEqual(['Write the doc', 'Ship it', 'Réunion équipe']);
    expect(clear.hidden).toBe(true);
    document.querySelector<HTMLButtonElement>('[data-tag="meeting"]')!.click();
    expect((document.getElementById('recherche') as HTMLInputElement).value).toBe('meeting');
    expect(all()).toEqual(['Réunion équipe']);
    expect(launched).toEqual([]);
  });

  it('takes the focus on /, but not while typing elsewhere', () => {
    mount([A]);
    document.body.dispatchEvent(new KeyboardEvent('keydown', { key: '/', bubbles: true }));
    expect(document.activeElement?.id).toBe('recherche');
  });
});

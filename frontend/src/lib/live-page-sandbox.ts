import type { LivePageAction, LivePageDetail } from '../types/generated';
import { MAX_LIVE_PAGE_EMBED_REPORTS, MAX_LIVE_PAGE_EMBED_URL_CHARS } from './live-page-embeds';
import { standaloneDiscussionId, standaloneLivePageId } from './live-page-navigation';

export const LIVE_PAGE_CSP = [
  "default-src 'none'",
  "img-src data: blob:",
  "style-src 'unsafe-inline'",
  "script-src 'unsafe-inline'",
  "connect-src 'none'",
  "font-src data:",
  "media-src 'none'",
  "object-src 'none'",
  "base-uri 'none'",
  "form-action 'none'",
].join('; ');

export interface LivePageRuntimeData {
  version: 1;
  page: { id: string; slug: string; title: string; data_revision: number; params?: Record<string, string> };
  /** Display preferences the host keeps for this Page (KT-1030); the
   * sandbox has no storage of its own. */
  prefs?: Partial<Record<LivePagePrefKey, boolean>>;
  datasets: Record<string, {
    kind: string;
    current: unknown;
    points: Array<{ observed_at: string; value: unknown }>;
  }>;
}

const LIVE_PAGE_EXPORT_TIMEOUT_MS = 30_000;
const MAX_RENDERED_PAGE_HTML_CHARS = 16 * 1024 * 1024;
const MAX_RENDERED_PAGE_IMAGE_CHARS = 48 * 1024 * 1024;

export interface RenderedPageExport {
  html: string;
  pageImages: string[];
}

interface LivePageExportResponse {
  type: 'kronn:page-export';
  version: 1;
  channel_id: string;
  request_id: string;
  html?: string;
  viewport_width?: number;
  content_height?: number;
  error?: string;
}

/** The only preferences a Page may ask the host to remember: display-only
 * flags, never an action or a value the host would act on. */
export const LIVE_PAGE_PREF_KEYS = ['notice-dismissed'] as const;
export type LivePagePrefKey = typeof LIVE_PAGE_PREF_KEYS[number];
const PAGE_PREF_STORAGE = 'kronn:page-pref:';

function prefStorageKey(pageId: string, key: LivePagePrefKey): string {
  return `${PAGE_PREF_STORAGE}${pageId}:${key}`;
}

/** The preferences the host stored for a Page; empty when storage is refused. */
export function readLivePagePrefs(pageId: string): Partial<Record<LivePagePrefKey, boolean>> {
  const prefs: Partial<Record<LivePagePrefKey, boolean>> = {};
  for (const key of LIVE_PAGE_PREF_KEYS) {
    try {
      const raw = window.localStorage.getItem(prefStorageKey(pageId, key));
      if (raw === 'true' || raw === 'false') prefs[key] = raw === 'true';
    } catch { /* storage refused: the Page shows its default */ }
  }
  return prefs;
}

export function writeLivePagePref(pageId: string, key: LivePagePrefKey, value: boolean): void {
  try {
    window.localStorage.setItem(prefStorageKey(pageId, key), String(value));
  } catch { /* storage refused: the preference lasts this view only */ }
}

interface LivePagePrefRequest {
  type: 'kronn:page-pref';
  version: 1;
  channel_id: string;
  page_id: string;
  key: string;
  value: unknown;
}

interface LivePageOpenLinkRequest {
  type: 'kronn:page-open-link';
  version: 1;
  channel_id: string;
  url: string;
}

interface LivePageActionRequest {
  type: 'kronn:page-action';
  version: 1;
  channel_id: string;
  action_ref: string;
  bindings: Record<string, string>;
  binding_labels?: Record<string, string>;
  anchor: { left: number; top: number; width: number; height: number };
}

/** Sent while a card is open, so it tracks the row it belongs to as the Page
 * scrolls. It carries no action and no binding: it can only move a card the
 * user already opened, which is why it is accepted without user activation. */
interface LivePageActionAnchorRequest {
  type: 'kronn:page-action-anchor';
  version: 1;
  channel_id: string;
  anchor: { left: number; top: number; width: number; height: number; slot?: boolean };
}

export interface LivePageActionIntent {
  actionRef: string;
  bindings: Record<string, string>;
  /** Display text the Page gives a binding (`data-kronn-binding-labels`): shown
   * on the card only, never sent to the server, which resolves the selector. */
  bindingLabels?: Record<string, string>;
  anchor: LivePageActionAnchor;
}

export interface LivePageActionAnchor {
  left: number;
  top: number;
  width: number;
  height: number;
  /** The rectangle is the collapse the Page opened for this card, so the card
   * fills it. False (or absent) means it is the row, and the card sits under
   * it — the placement used for the frame between the click and the slot. */
  slot?: boolean;
}

export interface LivePageEmbedBox { left: number; top: number; width: number; height: number }

/** Third-party content the Page placed, as the bridge reports it. The host
 * still checks the URL's origin against the allowed sites before drawing it. */
export interface LivePageEmbedPlacement {
  /** Stable for the same element across reports, so the host keeps its iframe. */
  key: string;
  /** The placeholder's `data-kronn-embed`, as written by the Page. */
  url: string;
  /** In the Page iframe's viewport coordinates. */
  rect: LivePageEmbedBox;
  /** The part of `rect` its scrolling or clipping ancestors leave visible, in
   * the same coordinates; absent when no ancestor clips it. */
  clip?: LivePageEmbedBox;
  visible: boolean;
  /** The placeholder's computed border-radius, when it is a plain length list. */
  radius?: string;
}

interface LivePageEmbedsRequest {
  type: 'kronn:page-embeds';
  version: 1;
  channel_id: string;
  embeds: unknown;
}

const LIVE_PAGE_EMBED_KEY = /^e[0-9a-z]{1,13}:\d{1,3}$/;
const LIVE_PAGE_EMBED_RADIUS = /^\d+(?:\.\d+)?px(?: \d+(?:\.\d+)?px){0,3}$/;
const MAX_EMBED_COORD = 200_000;
const MAX_EMBED_SIZE = 20_000;

function parseEmbedBox(raw: unknown): LivePageEmbedBox | null {
  if (!raw || typeof raw !== 'object') return null;
  const { left, top, width, height } = raw as Record<string, unknown>;
  if (![left, top, width, height].every(value => typeof value === 'number' && Number.isFinite(value))) return null;
  const box = { left: left as number, top: top as number, width: width as number, height: height as number };
  if (Math.abs(box.left) > MAX_EMBED_COORD || Math.abs(box.top) > MAX_EMBED_COORD) return null;
  if (box.width < 0 || box.height < 0 || box.width > MAX_EMBED_SIZE || box.height > MAX_EMBED_SIZE) return null;
  return box;
}

/** Structural check of a `kronn:page-embeds` list: bounded, typed, finite.
 * Whether a URL may be drawn is decided later, by `planLivePageEmbeds`. */
export function parseLivePageEmbeds(raw: unknown, max = MAX_LIVE_PAGE_EMBED_REPORTS): LivePageEmbedPlacement[] | null {
  if (!Array.isArray(raw)) return null;
  const out: LivePageEmbedPlacement[] = [];
  const seen = new Set<string>();
  for (const entry of raw) {
    if (out.length >= max) break;
    if (!entry || typeof entry !== 'object') continue;
    const { key, url, rect, clip, visible, radius } = entry as Record<string, unknown>;
    if (typeof key !== 'string' || !LIVE_PAGE_EMBED_KEY.test(key) || seen.has(key)) continue;
    if (typeof url !== 'string' || !url || url.length > MAX_LIVE_PAGE_EMBED_URL_CHARS) continue;
    const box = parseEmbedBox(rect);
    if (!box) continue;
    // A clip that is present but malformed hides the content rather than
    // letting it spill over the Page's own controls.
    const clipBox = clip === undefined || clip === null ? null : parseEmbedBox(clip) ?? { ...box, width: 0, height: 0 };
    seen.add(key);
    out.push({
      key,
      url,
      rect: box,
      ...(clipBox ? { clip: clipBox } : {}),
      visible: visible === true,
      ...(typeof radius === 'string' && LIVE_PAGE_EMBED_RADIUS.test(radius) ? { radius } : {}),
    });
  }
  return out;
}

const MAX_LIVE_PAGE_LINK_CHARS = 8 * 1024;

/** The row a click is bound to, spelled exactly as the backend stores a
 * launch's `binding_key`: sorted `name=selector` pairs joined by U+001F, empty
 * for an unbound CTA. The iframe bridge below computes the same string. */
export const LIVE_PAGE_BINDING_LABEL_MAX = 200;

/** Labels for bindings the click actually carries; anything else is dropped. */
function parseBindingLabels(raw: unknown, bindings: Record<string, string>): Record<string, string> | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null;
  const labels = Object.fromEntries(Object.entries(raw as Record<string, unknown>).filter(([key, value]) => (
    Object.hasOwn(bindings, key) && typeof value === 'string' && value.trim() !== ''
      && value.length <= LIVE_PAGE_BINDING_LABEL_MAX
  ))) as Record<string, string>;
  return Object.keys(labels).length > 0 ? labels : null;
}

export function liveActionBindingKey(bindings: Record<string, string>): string {
  return Object.entries(bindings).map(([name, selector]) => `${name}=${selector}`).sort().join('\u001f');
}

/**
 * Mirror the host data-theme into the opaque iframe. Pages style explicit
 * light/dark values and may fall back to their media query for custom themes.
 */
export function postLivePageTheme(
  target: Window,
  channelId: string,
  theme: string,
  tokens: Record<string, string> = {},
): void {
  target.postMessage({
    type: 'kronn:page-theme',
    version: 1,
    channel_id: channelId,
    theme,
    tokens,
  }, '*');
}

/** Kronn's structural colours, mirrored into the Page as `--kr-<name>` so a Page can
 * paint its surfaces, rules and text in the shell's palette — and match the action
 * card the host draws over it. Semantic colours are deliberately not shared. */
export const LIVE_PAGE_THEME_TOKENS = [
  'bg-base', 'bg-surface', 'bg-elevated', 'text-primary', 'text-secondary', 'text-ghost', 'border-medium',
] as const;

/** A colour value and nothing else: these land in a Page's stylesheet. */
const SAFE_TOKEN_VALUE = /^[#(),.%\s\w-]{1,64}$/;

function safeTokens(tokens: Record<string, string> | null | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  for (const name of LIVE_PAGE_THEME_TOKENS) {
    const value = tokens?.[name];
    if (typeof value === 'string' && SAFE_TOKEN_VALUE.test(value) && !/url\s*\(/i.test(value)) out[name] = value;
  }
  return out;
}

/** The host's current values for {@link LIVE_PAGE_THEME_TOKENS}. */
export function hostThemeTokens(): Record<string, string> {
  const style = getComputedStyle(document.documentElement);
  const read: Record<string, string> = {};
  for (const name of LIVE_PAGE_THEME_TOKENS) read[name] = style.getPropertyValue('--kr-' + name).trim();
  return safeTokens(read);
}

/** The theme the host is currently showing, read from the attribute
 * `ThemeContext` maintains. `null` before the provider has run. */
export function hostTheme(): string | null {
  return document.documentElement.getAttribute('data-theme');
}

/**
 * Reserve space below the active row for the privileged host action card.
 * null closes the slot. Page code walking sibling rows must skip elements
 * marked data-kronn-action-slot.
 */
export function postLivePageActionSlot(
  target: Window,
  channelId: string,
  slot: { actionRef: string; bindingKey: string; height: number } | null,
): void {
  target.postMessage({
    type: 'kronn:page-action-slot',
    version: 1,
    channel_id: channelId,
    slot: slot && { action_ref: slot.actionRef, binding_key: slot.bindingKey, height: slot.height },
  }, '*');
}

/** Tell the Page which of its buttons have run, and how it went. The iframe
 * marks each matching `[data-kronn-action]` with `data-kronn-action-state`. */
export function postLivePageActionStates(
  target: Window,
  channelId: string,
  launches: Pick<LivePageAction, 'id' | 'action_ref' | 'binding_key' | 'state'>[],
): void {
  const states = launches.map(launch => ({
    action_ref: launch.action_ref,
    binding_key: launch.binding_key ?? '',
    state: launch.state,
    launch_id: launch.id,
  }));
  target.postMessage({ type: 'kronn:page-action-states', version: 1, channel_id: channelId, states }, '*');
}
const LIVE_PAGE_ACTION_REF = /^[A-Za-z0-9._~-]{1,256}$/;

export function runtimeData(detail: LivePageDetail, params?: Record<string, string>): LivePageRuntimeData {
  return {
    version: 1,
    page: {
      id: detail.id,
      slug: detail.slug,
      title: detail.title,
      data_revision: detail.data_revision,
      // Only the standalone tab has a URL of its own to carry view parameters.
      ...(params ? { params: { ...params } } : {}),
    },
    prefs: readLivePagePrefs(detail.id),
    datasets: Object.fromEntries(detail.datasets.map(dataset => [dataset.name, {
      kind: dataset.kind,
      current: dataset.current,
      points: dataset.points.map(point => ({
        observed_at: point.observed_at,
        value: point.payload,
      })),
    }])),
  };
}

/** A default look for a button whose row has run. Wrapped in `:where()` so it
 * weighs nothing: any rule the author writes for the same button wins. */
const ACTION_STATE_STYLE = `<style>
:where([data-kronn-action-state="launching"],[data-kronn-action-state="running"]){cursor:progress}
:where([data-kronn-action-state])::after{display:inline-block;margin-inline-start:.45em;line-height:1}
:where([data-kronn-action-state="launching"],[data-kronn-action-state="running"])::after{content:"";width:.75em;height:.75em;border:2px solid currentColor;border-right-color:transparent;border-radius:50%;vertical-align:-.1em;animation:kronn-action-spin .8s linear infinite}
:where([data-kronn-action-state="succeeded"])::after{content:"✓"}
:where([data-kronn-action-state="failed"],[data-kronn-action-state="preflight_failed"])::after{content:"⚠"}
@keyframes kronn-action-spin{to{transform:rotate(360deg)}}
</style>`;

/**
 * Inject the security policy before Page-authored markup and a tiny, local
 * data bridge. The iframe itself must still use `sandbox="allow-scripts"`
 * without `allow-same-origin`; CSP and sandbox are complementary boundaries.
 */
export function buildSandboxDocument(
  html: string,
  channelId: string,
  initialTheme?: string | null,
  initialTokens?: Record<string, string> | null,
): string {
  const safeChannel = JSON.stringify(channelId).replaceAll('<', '\\u003c');
  // Set before the Page's own markup parses, so it never paints in the wrong
  // theme for a frame. Runtime changes arrive by message instead, because
  // rebuilding this document would reload the iframe and lose its state.
  const theme = typeof initialTheme === 'string' && /^[A-Za-z0-9_-]{1,64}$/.test(initialTheme)
    ? `<script>document.documentElement.setAttribute('data-theme',${JSON.stringify(initialTheme)})</script>`
    : '';
  const tokens = Object.entries(safeTokens(initialTokens));
  const palette = tokens.length ? `<style>:root{${tokens.map(([k, v]) => `--kr-${k}:${v}`).join(';')}}</style>` : '';
  const head = `<meta http-equiv="Content-Security-Policy" content="${LIVE_PAGE_CSP}">${theme}${palette}${ACTION_STATE_STYLE}`;
  const bridge = `<script>(()=>{
    const channel=${safeChannel};
    const userActivation=navigator.userActivation;
    const stopImmediate=Event.prototype.stopImmediatePropagation;
    const portPost=MessagePort.prototype.postMessage;
    const portStart=MessagePort.prototype.start;
    const closest=Element.prototype.closest;
    const getAttribute=Element.prototype.getAttribute;
    const getBounds=Element.prototype.getBoundingClientRect;
    const parseJson=JSON.parse;
    const objectEntries=Object.entries;
    let latest=null;
    let linkPort=null;
    let actionStates=new Map();
    // The CTA that opened a card, so its anchor can be re-sent while this
    // document scrolls: the host draws the card in a layer that does NOT
    // scroll with us, so a one-shot anchor drifts away from its row.
    let anchored=null;
    let anchoredAt=null;
    let anchorQueued=false;
    // A row, not the button: the card belongs under the whole line it acts on.
    // A Page may name where its collapse opens: the card then sits under the CTA's own
    // block instead of after the whole table row that happens to contain it.
    const slotHost=element=>closest.call(element,'[data-kronn-action-slot-host]');
    const anchorRect=element=>{
      const row=slotHost(element)||closest.call(element,'tr,li')||element;
      const rect=getBounds.call(row);
      return {left:rect.left,top:rect.top,width:rect.width,height:rect.height};
    };
    // The collapse the host asked us to open, in THIS document, so the rows
    // below are pushed down instead of being covered by a floating panel.
    let slotEl=null;
    let slotSpec=null;
    const dropSlot=()=>{
      if(slotEl&&slotEl.parentNode)slotEl.parentNode.removeChild(slotEl);
      slotEl=null;
    };
    const findCta=(ref,key)=>{
      const all=document.querySelectorAll('[data-kronn-action]');
      for(let i=0;i<all.length;i+=1){
        const el=all[i];
        if((getAttribute.call(el,'data-kronn-action')||'').trim()!==ref)continue;
        if(bindingKey(readBindings(el))===key)return el;
      }
      return null;
    };
    const openSlot=(ref,key,height)=>{
      const cta=findCta(ref,key);
      const host=cta?slotHost(cta):null;
      const row=host||(cta?closest.call(cta,'tr,li')||cta:null);
      if(!row||!row.parentNode){dropSlot();return;}
      const inRow=!host&&row.tagName==='TR';
      if(!slotEl||slotEl.__row!==row){
        dropSlot();
        slotEl=document.createElement(host?'div':inRow?'tr':'li');
        slotEl.setAttribute('data-kronn-action-slot','');
        if(inRow){
          // Exactly the row's own span: an oversized colspan adds phantom columns, and a
          // table-layout:fixed table then shares its free width with them.
          const span=Array.prototype.reduce.call(row.cells,(n,c)=>n+(c.colSpan||1),0)||1;
          const cell=document.createElement('td');
          cell.setAttribute('colspan',String(span));
          cell.style.padding='0';
          cell.style.border='0';
          slotEl.appendChild(cell);
        }else if(!host){
          slotEl.style.listStyle='none';
        }
        slotEl.__row=row;
        if(host)host.appendChild(slotEl);
        else row.parentNode.insertBefore(slotEl,row.nextSibling);
      }
      const box=inRow?slotEl.firstChild:slotEl;
      box.style.height=height+'px';
      queueAnchor();
    };
    const sendAnchor=()=>{
      anchorQueued=false;
      if(!anchored||!linkPort)return;
      // A Page that redraws its rows replaces the CTA: follow its successor.
      if(!anchored.isConnected)anchored=anchoredAt&&findCta(anchoredAt.ref,anchoredAt.key);
      if(!anchored)return;
      // With a slot open the card fills it, so the slot IS the anchor.
      const inSlot=Boolean(slotEl&&slotEl.isConnected);
      const rect=inSlot?getBounds.call(slotEl):anchorRect(anchored);
      portPost.call(linkPort,{type:'kronn:page-action-anchor',version:1,channel_id:channel,anchor:{left:rect.left,top:rect.top,width:rect.width,height:rect.height,slot:inSlot}});
    };
    const queueAnchor=()=>{
      if(anchorQueued||!anchored)return;
      anchorQueued=true;
      if(typeof requestAnimationFrame==='function')requestAnimationFrame(sendAnchor);
      else setTimeout(sendAnchor,16);
    };
    addEventListener('scroll',queueAnchor,{capture:true,passive:true});
    addEventListener('resize',queueAnchor,{passive:true});
    const readBindings=element=>{
      let bindings={};
      const raw=getAttribute.call(element,'data-kronn-bindings');
      if(raw){
        try{
          const parsed=parseJson(raw);
          if(parsed&&typeof parsed==='object'&&!Array.isArray(parsed)){
            bindings=Object.fromEntries(objectEntries(parsed).filter(([key,value])=>key.length<=128&&typeof value==='string'&&value.length<=4096));
          }
        }catch(_error){}
      }
      return bindings;
    };
    const readBindingLabels=(element,bindings)=>{
      const labels={};
      const raw=getAttribute.call(element,'data-kronn-binding-labels');
      if(!raw)return labels;
      try{
        const parsed=parseJson(raw);
        if(parsed&&typeof parsed==='object'&&!Array.isArray(parsed)){
          objectEntries(parsed).forEach(([key,value])=>{if(typeof bindings[key]==='string'&&typeof value==='string'&&value.length<=${LIVE_PAGE_BINDING_LABEL_MAX})labels[key]=value;});
        }
      }catch(_error){}
      return labels;
    };
    const bindingKey=bindings=>objectEntries(bindings).map(([name,selector])=>name+'='+selector).sort().join('\\u001f');
    const markActions=()=>{
      document.querySelectorAll('[data-kronn-action]').forEach(element=>{
        const ref=(getAttribute.call(element,'data-kronn-action')||'').trim();
        const entry=actionStates.get(ref+'\\n'+bindingKey(readBindings(element)));
        if(entry){
          const state=entry.state;
          if(getAttribute.call(element,'data-kronn-action-state')!==state)element.setAttribute('data-kronn-action-state',state);
          // Tells one attempt of a row from the next, even when both succeeded.
          if(entry.launch){
            if(getAttribute.call(element,'data-kronn-action-launch')!==entry.launch)element.setAttribute('data-kronn-action-launch',entry.launch);
          }else element.removeAttribute('data-kronn-action-launch');
          if(state==='launching'||state==='running')element.setAttribute('aria-busy','true');
          else element.removeAttribute('aria-busy');
        }else if(element.hasAttribute('data-kronn-action-state')){
          element.removeAttribute('data-kronn-action-state');
          element.removeAttribute('data-kronn-action-launch');
          element.removeAttribute('aria-busy');
        }
      });
    };
    // Pages render their rows from data, often after this runs: re-mark
    // whenever rows appear or change their binding, never on our own marks.
    new MutationObserver(()=>{
      if(actionStates.size)markActions();
      // Redrawn rows take the open collapse with them: reopen it under the new row.
      if(slotSpec&&!(slotEl&&slotEl.isConnected))openSlot(slotSpec.ref,slotSpec.key,slotSpec.height);
      else if(anchored&&!anchored.isConnected)queueAnchor();
    }).observe(document,{childList:true,subtree:true,attributes:true,attributeFilter:['data-kronn-action','data-kronn-bindings']});
    Object.defineProperty(window,'KronnPageData',{configurable:false,get:()=>latest});
    const materializedRoot=()=>{
      const root=document.documentElement.cloneNode(true);
      const sourceCanvases=document.querySelectorAll('canvas');
      root.querySelectorAll('canvas').forEach((canvas,index)=>{
        const source=sourceCanvases[index];
        if(!source)return;
        try{
          const image=document.createElement('img');
          const bounds=source.getBoundingClientRect();
          image.src=source.toDataURL('image/png');
          image.alt=source.getAttribute('aria-label')||source.getAttribute('title')||'Chart';
          image.className=source.className;
          image.style.cssText=source.style.cssText;
          if(bounds.width>0)image.style.width=bounds.width+'px';
          if(bounds.height>0)image.style.height=bounds.height+'px';
          canvas.replaceWith(image);
        }catch(_error){}
      });
      root.querySelectorAll('script').forEach(script=>script.remove());
      root.setAttribute('xmlns','http://www.w3.org/1999/xhtml');
      return root;
    };
    const renderedExport=()=>{
      const root=materializedRoot();
      const width=Math.max(1,Math.ceil(document.documentElement.scrollWidth,document.body?document.body.scrollWidth:0,innerWidth));
      const totalHeight=Math.max(1,Math.ceil(document.documentElement.scrollHeight,document.body?document.body.scrollHeight:0,innerHeight));
      return {html:'<!doctype html>'+root.outerHTML,viewport_width:width,content_height:totalHeight};
    };
    const relayOpenLink=url=>{
      if(url==null||String(url).trim()==='')return null;
      if(!linkPort||(userActivation&&!userActivation.isActive))return null;
      let resolved;
      try{resolved=new URL(String(url),document.baseURI).href;}catch(_error){return null;}
      portPost.call(linkPort,{type:'kronn:page-open-link',version:1,channel_id:channel,url:resolved});
      return null;
    };
    addEventListener('click',event=>{
      if(event.defaultPrevented||event.button!==0)return;
      const element=event.target instanceof Element?event.target:event.target&&event.target.parentElement;
      const action=element?closest.call(element,'[data-kronn-action]'):null;
      if(action){
        event.preventDefault();
        if(!linkPort||(userActivation&&!userActivation.isActive))return;
        const actionRef=(getAttribute.call(action,'data-kronn-action')||'').trim();
        if(!/^[A-Za-z0-9._~-]{1,256}$/.test(actionRef))return;
        const bindings=readBindings(action);
        anchored=action;
        anchoredAt={ref:actionRef,key:bindingKey(bindings)};
        portPost.call(linkPort,{type:'kronn:page-action',version:1,channel_id:channel,action_ref:actionRef,bindings,binding_labels:readBindingLabels(action,bindings),anchor:anchorRect(action)});
        return;
      }
      const anchor=element&&element.closest?element.closest('a[href]'):null;
      if(!anchor||anchor.target.toLowerCase()!=='_blank')return;
      event.preventDefault();
      relayOpenLink(anchor.href);
    },true);
    // A display preference for this Page, remembered by the host: one
    // allow-listed key, a boolean, after a live click.
    const prefKeys=${JSON.stringify(LIVE_PAGE_PREF_KEYS)};
    try{
      Object.defineProperty(window,'KronnPagePref',{configurable:false,writable:false,value:(key,value)=>{
        if(!linkPort||(userActivation&&!userActivation.isActive))return false;
        if(prefKeys.indexOf(key)<0||typeof value!=='boolean'||!latest||!latest.page||typeof latest.page.id!=='string')return false;
        portPost.call(linkPort,{type:'kronn:page-pref',version:1,channel_id:channel,page_id:latest.page.id,key,value});
        return true;
      }});
    }catch(_error){}
    try{
      Object.defineProperty(window,'open',{configurable:false,writable:false,value:url=>relayOpenLink(url)});
    }catch(_error){}
    // A Page that declares <meta name="kronn-page-height" content="auto"> is sized to its
    // content by the host, which then scrolls it: the host's action card lives in that same
    // scroll context and follows the Page natively, instead of chasing it frame by frame.
    // The html box height is the content height, whatever the current frame size — so a
    // collapse that closes lets the frame shrink back.
    let heightSent=-1;
    let heightQueued=false;
    const autoHeight=()=>{
      const meta=document.querySelector('meta[name="kronn-page-height"]');
      return Boolean(meta)&&(getAttribute.call(meta,'content')||'').trim()==='auto';
    };
    const sendHeight=()=>{
      heightQueued=false;
      if(!linkPort)return;
      const height=Math.ceil(getBounds.call(document.documentElement).height);
      if(!isFinite(height)||height<=0||height>200000||height===heightSent)return;
      heightSent=height;
      portPost.call(linkPort,{type:'kronn:page-height',version:1,channel_id:channel,height});
    };
    const queueHeight=()=>{
      if(heightQueued)return;
      heightQueued=true;
      if(typeof requestAnimationFrame==='function')requestAnimationFrame(sendHeight);else setTimeout(sendHeight,16);
    };
    let heightWatched=false;
    const watchHeight=()=>{
      if(!autoHeight())return;
      document.documentElement.style.overflow='hidden';
      heightSent=-1;
      queueHeight();
      if(heightWatched)return;
      heightWatched=true;
      if(typeof ResizeObserver==='function'){
        const observer=new ResizeObserver(queueHeight);
        observer.observe(document.documentElement);
        if(document.body)observer.observe(document.body);
      }
      addEventListener('load',queueHeight);
    };
    // Third-party content: a nested iframe would inherit this sandbox and never
    // play, so the host draws the real content over the placeholder. The Page
    // names a URL; the host decides whether its site is allowed. Each
    // placeholder keeps the same key while it lives (its URL and rank among its
    // twins), so the host moves its iframe instead of reloading it. The bound
    // here only caps the work: refused placeholders must not hide allowed ones,
    // so the players' own quota is applied by the host, after its check.
    let embedsSent=null;
    let embedsQueued=false;
    const embedSizes=typeof ResizeObserver==='function'?new ResizeObserver(()=>queueEmbeds()):null;
    const embedWatched=new WeakSet();
    const embedHash=value=>{
      let hash=5381;
      for(let i=0;i<value.length;i+=1)hash=((hash*33)^value.charCodeAt(i))>>>0;
      return hash.toString(36);
    };
    // What the scrolling or clipping ancestors leave visible of a box. The
    // frame's own viewport is clipped by the host; a fixed ancestor escapes
    // everything above it.
    const embedClip=(el,r)=>{
      let left=r.left,top=r.top,right=r.right,bottom=r.bottom,clipped=false;
      for(let node=el.parentElement;node&&node!==document.body&&node!==document.documentElement;node=node.parentElement){
        let style=null;
        try{style=getComputedStyle(node);}catch(_error){}
        if(!style)continue;
        const overflowX=style.overflowX||style.overflow,overflowY=style.overflowY||style.overflow;
        const clipX=Boolean(overflowX)&&overflowX!=='visible';
        const clipY=Boolean(overflowY)&&overflowY!=='visible';
        if(clipX||clipY){
          const b=getBounds.call(node);
          const innerLeft=b.left+node.clientLeft,innerTop=b.top+node.clientTop;
          if(clipX){left=Math.max(left,innerLeft);right=Math.min(right,innerLeft+node.clientWidth);}
          if(clipY){top=Math.max(top,innerTop);bottom=Math.min(bottom,innerTop+node.clientHeight);}
          clipped=true;
        }
        if(style.position==='fixed')break;
      }
      return clipped?{left,top,width:Math.max(0,right-left),height:Math.max(0,bottom-top)}:null;
    };
    const sendEmbeds=()=>{
      embedsQueued=false;
      if(!linkPort)return;
      const embeds=[];
      const ranks=new Map();
      const all=document.querySelectorAll('[data-kronn-embed]');
      for(let i=0;i<all.length&&embeds.length<${MAX_LIVE_PAGE_EMBED_REPORTS};i+=1){
        const el=all[i];
        const url=(getAttribute.call(el,'data-kronn-embed')||'').trim();
        const scheme=url.slice(0,8).toLowerCase();
        if(url.length>${MAX_LIVE_PAGE_EMBED_URL_CHARS}||(scheme!=='https://'&&scheme.slice(0,7)!=='http://'))continue;
        if(embedSizes&&!embedWatched.has(el)){embedWatched.add(el);embedSizes.observe(el);}
        const twin='e'+embedHash(url);
        const rank=ranks.get(twin)||0;
        ranks.set(twin,rank+1);
        const r=getBounds.call(el);
        const clip=embedClip(el,r);
        const area=clip||{left:r.left,top:r.top,width:r.width,height:r.height};
        let shown=area.width>0&&area.height>0&&area.top+area.height>0&&area.left+area.width>0&&area.top<innerHeight&&area.left<innerWidth;
        if(shown&&typeof el.checkVisibility==='function')shown=el.checkVisibility({visibilityProperty:true});
        let radius='';
        try{radius=String(getComputedStyle(el).borderRadius||'').slice(0,64);}catch(_error){}
        const entry={key:twin+':'+rank,url,rect:{left:r.left,top:r.top,width:r.width,height:r.height},visible:shown,radius};
        if(clip)entry.clip=clip;
        embeds.push(entry);
      }
      const signature=JSON.stringify(embeds);
      if(signature===embedsSent)return;
      embedsSent=signature;
      portPost.call(linkPort,{type:'kronn:page-embeds',version:1,channel_id:channel,embeds});
    };
    const queueEmbeds=()=>{
      if(embedsQueued)return;
      embedsQueued=true;
      if(typeof requestAnimationFrame==='function')requestAnimationFrame(sendEmbeds);else setTimeout(sendEmbeds,16);
    };
    addEventListener('scroll',queueEmbeds,{capture:true,passive:true});
    addEventListener('resize',queueEmbeds,{passive:true});
    addEventListener('load',queueEmbeds);
    new MutationObserver(queueEmbeds).observe(document,{childList:true,subtree:true,attributes:true,attributeFilter:['data-kronn-embed','style','class','hidden']});
    if(embedSizes){
      embedSizes.observe(document.documentElement);
      if(document.body)embedSizes.observe(document.body);
    }
    addEventListener('message',event=>{
      const message=event.data;
      if(!message||message.version!==1||message.channel_id!==channel)return;
      if(message.type==='kronn:page-link-port'){
        if(event.ports.length!==1)return;
        stopImmediate.call(event);
        linkPort=event.ports[0];
        portStart.call(linkPort);
        watchHeight();
        // A new port is a new host listener: it gets the full list, even empty.
        embedsSent=null;
        queueEmbeds();
        return;
      }
      if(message.type==='kronn:page-data'){
        latest=message.data;
        dispatchEvent(new CustomEvent('kronn:page-data',{detail:latest}));
        return;
      }
      if(message.type==='kronn:page-theme'){
        const t=message.theme;
        if(typeof t!=='string'||t.length>64)return;
        document.documentElement.setAttribute('data-theme',t);
        const tokens=message.tokens&&typeof message.tokens==='object'?message.tokens:{};
        for(const name of ${JSON.stringify(LIVE_PAGE_THEME_TOKENS)}){
          const value=tokens[name];
          if(typeof value==='string'&&/^[#(),.%\\s\\w-]{1,64}$/.test(value)&&!/url\\s*\\(/i.test(value)){
            document.documentElement.style.setProperty('--kr-'+name,value);
          }
        }
        dispatchEvent(new CustomEvent('kronn:page-theme',{detail:t}));
        return;
      }
      if(message.type==='kronn:page-action-slot'){
        const slot=message.slot;
        if(!slot){slotSpec=null;dropSlot();queueAnchor();return;}
        if(typeof slot.action_ref!=='string'||typeof slot.binding_key!=='string')return;
        const height=Number(slot.height);
        if(!isFinite(height)||height<0||height>4000)return;
        slotSpec={ref:slot.action_ref,key:slot.binding_key,height};
        openSlot(slot.action_ref,slot.binding_key,height);
        return;
      }
      if(message.type==='kronn:page-action-states'){
        if(!Array.isArray(message.states))return;
        actionStates=new Map(message.states
          .filter(entry=>entry&&typeof entry.action_ref==='string'&&typeof entry.binding_key==='string'&&typeof entry.state==='string')
          .map(entry=>[entry.action_ref+'\\n'+entry.binding_key,{state:entry.state,launch:typeof entry.launch_id==='string'?entry.launch_id:null}]));
        markActions();
        return;
      }
      if(message.type!=='kronn:page-export-request'||typeof message.request_id!=='string')return;
      const reply=()=>{
        try{
          const rendered=renderedExport();
          parent.postMessage({type:'kronn:page-export',version:1,channel_id:channel,request_id:message.request_id,...rendered},'*');
        }catch(error){
          parent.postMessage({type:'kronn:page-export',version:1,channel_id:channel,request_id:message.request_id,error:String(error)},'*');
        }
      };
      if(typeof requestAnimationFrame==='function')requestAnimationFrame(()=>requestAnimationFrame(reply));
      else setTimeout(reply,0);
    });
  })();</script>`;
  const injection = `${head}${bridge}`;
  const match = /<head(?:\s[^>]*)?>/i.exec(html);
  if (match?.index != null) {
    const offset = match.index + match[0].length;
    return `${html.slice(0, offset)}${injection}${html.slice(offset)}`;
  }
  return `<!doctype html><html><head>${injection}</head><body>${html}</body></html>`;
}

function safeLivePageLink(value: unknown): string | null {
  if (typeof value !== 'string' || value.length === 0 || value.length > MAX_LIVE_PAGE_LINK_CHARS) {
    return null;
  }
  try {
    const url = new URL(value);
    if ((url.protocol !== 'http:' && url.protocol !== 'https:') || url.username || url.password) {
      return null;
    }
    return url.href;
  } catch {
    return null;
  }
}

function isInternalKronnLink(value: string): boolean {
  const url = new URL(value);
  return url.origin === window.location.origin
    && url.pathname === window.location.pathname
    && Boolean(standaloneDiscussionId(url.hash) || standaloneLivePageId(url.hash));
}

function navigateInternalKronnLink(url: string): void {
  window.history.pushState(null, '', url);
  window.dispatchEvent(new Event('hashchange'));
}

/**
 * Let a sandboxed Live Page request one browser-controlled external tab.
 * The parent transfers a private MessagePort to the injected bridge. Its
 * initialization event is stopped before Page-authored listeners can observe
 * the port, so arbitrary authored postMessage calls never reach this handler.
 * User activation and URL validation provide separate defense-in-depth. The
 * sandbox intentionally remains `allow-scripts` without broad `allow-popups`.
 */
export interface LivePageOpenLinkRelay {
  connect(target: Window | null): void;
  dispose(): void;
}

export interface LivePageOpenLinkRelayOptions {
  /** Opens an external link; `window.open` by default. */
  openExternal?: (url: string, target: string, features: string) => unknown;
  onAction?: (intent: LivePageActionIntent) => void;
  onAnchor?: (anchor: LivePageActionIntent['anchor']) => void;
  onHeight?: (height: number) => void;
  /** Follows a link to another Kronn screen; same-tab hash navigation by default. */
  navigateInternal?: (url: string) => unknown;
  onEmbeds?: (embeds: LivePageEmbedPlacement[]) => void;
  /** The Page the frame shows now: a preference is stored only for it. */
  pageId?: () => string | null | undefined;
}

export function createLivePageOpenLinkRelay(
  channelId: string,
  {
    openExternal = window.open.bind(window),
    onAction,
    onAnchor,
    onHeight,
    navigateInternal = navigateInternalKronnLink,
    onEmbeds,
    pageId,
  }: LivePageOpenLinkRelayOptions = {},
): LivePageOpenLinkRelay {
  let activePort: MessagePort | null = null;
  const validAnchor = (anchor: LivePageActionAnchor | undefined): anchor is LivePageActionAnchor => (
    Boolean(anchor) && [anchor?.left, anchor?.top, anchor?.width, anchor?.height].every(Number.isFinite)
  );
  const onMessage = (message: LivePageOpenLinkRequest | LivePageActionRequest | LivePageActionAnchorRequest | LivePageEmbedsRequest | LivePagePrefRequest) => {
    if (
      !message
      || message.version !== 1
      || message.channel_id !== channelId
    ) return;
    // Before the activation gate on purpose: a scroll is not a click, and this
    // message only repositions a card that is already open.
    if (message.type === 'kronn:page-action-anchor') {
      if (!validAnchor(message.anchor)) return;
      onAnchor?.({ ...message.anchor, slot: message.anchor.slot === true });
      return;
    }
    // Where the Page placed its players: it only positions host-owned content
    // the host re-validates, so it needs no user activation either.
    if (message.type === 'kronn:page-embeds') {
      const embeds = parseLivePageEmbeds(message.embeds);
      if (embeds) onEmbeds?.(embeds);
      return;
    }
    // A content-sized Page reporting its height: layout, not a user action.
    if ((message as { type?: string }).type === 'kronn:page-height') {
      const height = (message as { height?: unknown }).height;
      if (typeof height !== 'number' || !Number.isFinite(height) || height <= 0 || height > 200_000) return;
      onHeight?.(Math.ceil(height));
      return;
    }
    if (navigator.userActivation && !navigator.userActivation.isActive) return;
    // Stored, never acted on: one known key, a boolean, for the Page shown.
    if (message.type === 'kronn:page-pref') {
      const current = pageId?.();
      const key = (LIVE_PAGE_PREF_KEYS as readonly string[]).find(known => known === message.key) as LivePagePrefKey | undefined;
      if (!current || message.page_id !== current || !key || typeof message.value !== 'boolean') return;
      writeLivePagePref(current, key, message.value);
      return;
    }
    if (message.type === 'kronn:page-action') {
      // A click may launch without a card: only a proven live gesture counts.
      if (navigator.userActivation?.isActive !== true) return;
      if (
        typeof message.action_ref !== 'string'
        || !LIVE_PAGE_ACTION_REF.test(message.action_ref)
        || !message.bindings
        || typeof message.bindings !== 'object'
        || !message.anchor
      ) return;
      const bindings = Object.fromEntries(Object.entries(message.bindings).filter(([key, value]) => (
        key.length <= 128 && typeof value === 'string' && value.length <= 4096
      ))) as Record<string, string>;
      const anchor = message.anchor;
      if (!validAnchor(anchor)) return;
      const bindingLabels = parseBindingLabels(message.binding_labels, bindings);
      onAction?.({ actionRef: message.action_ref, bindings, ...(bindingLabels ? { bindingLabels } : {}), anchor });
      return;
    }
    if (message.type !== 'kronn:page-open-link') return;
    const url = safeLivePageLink(message.url);
    if (!url) return;
    if (isInternalKronnLink(url)) {
      navigateInternal(url);
      return;
    }
    openExternal(url, '_blank', 'noopener,noreferrer');
  };
  return {
    connect(target) {
      activePort?.close();
      activePort = null;
      if (!target) return;
      const linkChannel = new MessageChannel();
      activePort = linkChannel.port1;
      activePort.onmessage = event => onMessage(event.data as LivePageOpenLinkRequest);
      activePort.start();
      target.postMessage({
        type: 'kronn:page-link-port',
        version: 1,
        channel_id: channelId,
      }, '*', [linkChannel.port2]);
    },
    dispose() {
      activePort?.close();
      activePort = null;
    },
  };
}

/**
 * Ask the opaque Page iframe for the DOM it currently displays. postMessage
 * keeps the sandbox boundary intact while allowing PDF/DOCX export to include
 * data-driven text, SVG charts, inline styles and rasterized canvas charts.
 */
export function requestRenderedPageHtml(
  frame: HTMLIFrameElement,
  channelId: string,
  timeoutMs = LIVE_PAGE_EXPORT_TIMEOUT_MS,
  capture: typeof captureRenderedPageImages = captureRenderedPageImages,
): Promise<RenderedPageExport> {
  const target = frame.contentWindow;
  if (!target) return Promise.reject(new Error('Page preview is not ready'));
  const requestId = globalThis.crypto?.randomUUID?.() ?? `export-${Date.now()}-${Math.random()}`;

  return new Promise((resolve, reject) => {
    const cleanup = () => {
      window.clearTimeout(timer);
      window.removeEventListener('message', onMessage);
    };
    const fail = (message: string) => {
      cleanup();
      reject(new Error(message));
    };
    const onMessage = (event: MessageEvent<LivePageExportResponse>) => {
      const message = event.data;
      if (
        event.source !== target
        || !message
        || message.type !== 'kronn:page-export'
        || message.version !== 1
        || message.channel_id !== channelId
        || message.request_id !== requestId
      ) return;
      if (message.error) {
        fail(message.error);
        return;
      }
      if (typeof message.html !== 'string' || !message.html.trim()) {
        fail('Page preview returned an empty document');
        return;
      }
      if (message.html.length > MAX_RENDERED_PAGE_HTML_CHARS) {
        fail('Rendered Page is too large to export');
        return;
      }
      const renderedHtml = message.html;
      window.removeEventListener('message', onMessage);
      const captured = capture(renderedHtml, message.viewport_width, message.content_height);
      void captured.then(pageImages => {
        if (
          pageImages.length === 0
          || pageImages.some(image => typeof image !== 'string' || !image.startsWith('data:image/png;base64,'))
          || pageImages.reduce((total, image) => total + image.length, 0) > MAX_RENDERED_PAGE_IMAGE_CHARS
        ) {
          fail('Page preview returned invalid rendered pages');
          return;
        }
        cleanup();
        resolve({ html: renderedHtml, pageImages });
      }).catch(cause => {
        fail(cause instanceof Error ? cause.message : String(cause));
      });
    };
    const timer = window.setTimeout(
      () => fail('Page preview did not answer the export request'),
      timeoutMs,
    );
    window.addEventListener('message', onMessage);
    target.postMessage({
      type: 'kronn:page-export-request',
      version: 1,
      channel_id: channelId,
      request_id: requestId,
    }, '*');
  });
}

function captureSvgPage(
  markup: string,
  width: number,
  totalHeight: number,
  offset: number,
  height: number,
): Promise<string> {
  return new Promise((resolve, reject) => {
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 ${offset} ${width} ${height}"><foreignObject x="0" y="0" width="${width}" height="${totalHeight}">${markup}</foreignObject></svg>`;
    const url = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
    const image = new Image();
    image.onload = () => {
      try {
        const scale = Math.max(1, Math.min(2, 8192 / width, 8192 / height));
        const canvas = document.createElement('canvas');
        canvas.width = Math.max(1, Math.round(width * scale));
        canvas.height = Math.max(1, Math.round(height * scale));
        const context = canvas.getContext('2d');
        if (!context) throw new Error('Canvas 2D is unavailable');
        context.drawImage(image, 0, 0, canvas.width, canvas.height);
        resolve(canvas.toDataURL('image/png'));
      } catch (cause) {
        reject(cause);
      }
    };
    image.onerror = () => {
      reject(new Error('Browser render capture failed'));
    };
    image.src = url;
  });
}

export async function captureRenderedPageImages(
  html: string,
  viewportWidth?: number,
  contentHeight?: number,
): Promise<string[]> {
  const width = Math.max(1, Math.ceil(viewportWidth ?? 0));
  const totalHeight = Math.max(1, Math.ceil(contentHeight ?? 0));
  if (!Number.isFinite(width) || !Number.isFinite(totalHeight)) {
    throw new Error('Page preview returned invalid render dimensions');
  }
  const pageHeight = Math.max(1, Math.floor(width * 297 / 210));
  const pageCount = Math.ceil(totalHeight / pageHeight);
  if (pageCount > 50) throw new Error('Rendered Page exceeds 50 export pages');
  const parser = new DOMParser();
  const parsed = parser.parseFromString(html, 'text/html');
  parsed.documentElement.setAttribute('xmlns', 'http://www.w3.org/1999/xhtml');
  const markup = new XMLSerializer().serializeToString(parsed.documentElement);
  const pageImages: string[] = [];
  for (let page = 0; page < pageCount; page += 1) {
    const offset = page * pageHeight;
    const height = Math.min(pageHeight, totalHeight - offset);
    pageImages.push(await captureSvgPage(markup, width, totalHeight, offset, height));
  }
  return pageImages;
}

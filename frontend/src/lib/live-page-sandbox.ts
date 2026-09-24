import type { LivePageAction, LivePageDetail } from '../types/generated';

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
  page: { id: string; slug: string; title: string; data_revision: number };
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

const MAX_LIVE_PAGE_LINK_CHARS = 8 * 1024;

/** The row a click is bound to, spelled exactly as the backend stores a
 * launch's `binding_key`: sorted `name=selector` pairs joined by U+001F, empty
 * for an unbound CTA. The iframe bridge below computes the same string. */
export function liveActionBindingKey(bindings: Record<string, string>): string {
  return Object.entries(bindings).map(([name, selector]) => `${name}=${selector}`).sort().join('\u001f');
}

/**
 * Mirror the host data-theme into the opaque iframe. Pages style explicit
 * light/dark values and may fall back to their media query for custom themes.
 */
export function postLivePageTheme(target: Window, channelId: string, theme: string): void {
  target.postMessage({
    type: 'kronn:page-theme',
    version: 1,
    channel_id: channelId,
    theme,
  }, '*');
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
  launches: Pick<LivePageAction, 'action_ref' | 'binding_key' | 'state'>[],
): void {
  const states = launches.map(launch => ({
    action_ref: launch.action_ref,
    binding_key: launch.binding_key ?? '',
    state: launch.state,
  }));
  target.postMessage({ type: 'kronn:page-action-states', version: 1, channel_id: channelId, states }, '*');
}
const LIVE_PAGE_ACTION_REF = /^[A-Za-z0-9._~-]{1,256}$/;

export function runtimeData(detail: LivePageDetail): LivePageRuntimeData {
  return {
    version: 1,
    page: {
      id: detail.id,
      slug: detail.slug,
      title: detail.title,
      data_revision: detail.data_revision,
    },
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
export function buildSandboxDocument(html: string, channelId: string, initialTheme?: string | null): string {
  const safeChannel = JSON.stringify(channelId).replaceAll('<', '\\u003c');
  // Set before the Page's own markup parses, so it never paints in the wrong
  // theme for a frame. Runtime changes arrive by message instead, because
  // rebuilding this document would reload the iframe and lose its state.
  const theme = typeof initialTheme === 'string' && /^[A-Za-z0-9_-]{1,64}$/.test(initialTheme)
    ? `<script>document.documentElement.setAttribute('data-theme',${JSON.stringify(initialTheme)})</script>`
    : '';
  const head = `<meta http-equiv="Content-Security-Policy" content="${LIVE_PAGE_CSP}">${theme}${ACTION_STATE_STYLE}`;
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
    let anchorQueued=false;
    // A row, not the button: the card belongs under the whole line it acts on.
    const anchorRect=element=>{
      const row=closest.call(element,'tr,li')||element;
      const rect=getBounds.call(row);
      return {left:rect.left,top:rect.top,width:rect.width,height:rect.height};
    };
    // The collapse the host asked us to open, in THIS document, so the rows
    // below are pushed down instead of being covered by a floating panel.
    let slotEl=null;
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
      const row=cta?closest.call(cta,'tr,li')||cta:null;
      if(!row||!row.parentNode){dropSlot();return;}
      const inRow=row.tagName==='TR';
      if(!slotEl||slotEl.__row!==row){
        dropSlot();
        slotEl=document.createElement(inRow?'tr':'li');
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
        }else{
          slotEl.style.listStyle='none';
        }
        slotEl.__row=row;
        row.parentNode.insertBefore(slotEl,row.nextSibling);
      }
      const box=inRow?slotEl.firstChild:slotEl;
      box.style.height=height+'px';
      queueAnchor();
    };
    const sendAnchor=()=>{
      anchorQueued=false;
      if(!anchored||!linkPort)return;
      if(!anchored.isConnected){anchored=null;return;}
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
    const bindingKey=bindings=>objectEntries(bindings).map(([name,selector])=>name+'='+selector).sort().join('\\u001f');
    const markActions=()=>{
      document.querySelectorAll('[data-kronn-action]').forEach(element=>{
        const ref=(getAttribute.call(element,'data-kronn-action')||'').trim();
        const state=actionStates.get(ref+'\\n'+bindingKey(readBindings(element)));
        if(state){
          if(getAttribute.call(element,'data-kronn-action-state')!==state)element.setAttribute('data-kronn-action-state',state);
          if(state==='launching'||state==='running')element.setAttribute('aria-busy','true');
          else element.removeAttribute('aria-busy');
        }else if(element.hasAttribute('data-kronn-action-state')){
          element.removeAttribute('data-kronn-action-state');
          element.removeAttribute('aria-busy');
        }
      });
    };
    // Pages render their rows from data, often after this runs: re-mark
    // whenever rows appear or change their binding, never on our own marks.
    new MutationObserver(()=>{if(actionStates.size)markActions();}).observe(document,{childList:true,subtree:true,attributes:true,attributeFilter:['data-kronn-action','data-kronn-bindings']});
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
        portPost.call(linkPort,{type:'kronn:page-action',version:1,channel_id:channel,action_ref:actionRef,bindings,anchor:anchorRect(action)});
        return;
      }
      const anchor=element&&element.closest?element.closest('a[href]'):null;
      if(!anchor||anchor.target.toLowerCase()!=='_blank')return;
      event.preventDefault();
      relayOpenLink(anchor.href);
    },true);
    try{
      Object.defineProperty(window,'open',{configurable:false,writable:false,value:url=>relayOpenLink(url)});
    }catch(_error){}
    addEventListener('message',event=>{
      const message=event.data;
      if(!message||message.version!==1||message.channel_id!==channel)return;
      if(message.type==='kronn:page-link-port'){
        if(event.ports.length!==1)return;
        stopImmediate.call(event);
        linkPort=event.ports[0];
        portStart.call(linkPort);
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
        dispatchEvent(new CustomEvent('kronn:page-theme',{detail:t}));
        return;
      }
      if(message.type==='kronn:page-action-slot'){
        const slot=message.slot;
        if(!slot){dropSlot();queueAnchor();return;}
        if(typeof slot.action_ref!=='string'||typeof slot.binding_key!=='string')return;
        const height=Number(slot.height);
        if(!isFinite(height)||height<0||height>4000)return;
        openSlot(slot.action_ref,slot.binding_key,height);
        return;
      }
      if(message.type==='kronn:page-action-states'){
        if(!Array.isArray(message.states))return;
        actionStates=new Map(message.states
          .filter(entry=>entry&&typeof entry.action_ref==='string'&&typeof entry.binding_key==='string'&&typeof entry.state==='string')
          .map(entry=>[entry.action_ref+'\\n'+entry.binding_key,entry.state]));
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

export function createLivePageOpenLinkRelay(
  channelId: string,
  openExternal: (url: string, target: string, features: string) => unknown = window.open.bind(window),
  onAction?: (intent: LivePageActionIntent) => void,
  onAnchor?: (anchor: LivePageActionIntent['anchor']) => void,
): LivePageOpenLinkRelay {
  let activePort: MessagePort | null = null;
  const validAnchor = (anchor: LivePageActionAnchor | undefined): anchor is LivePageActionAnchor => (
    Boolean(anchor) && [anchor?.left, anchor?.top, anchor?.width, anchor?.height].every(Number.isFinite)
  );
  const onMessage = (message: LivePageOpenLinkRequest | LivePageActionRequest | LivePageActionAnchorRequest) => {
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
    if (navigator.userActivation && !navigator.userActivation.isActive) return;
    if (message.type === 'kronn:page-action') {
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
      onAction?.({ actionRef: message.action_ref, bindings, anchor });
      return;
    }
    if (message.type !== 'kronn:page-open-link') return;
    const url = safeLivePageLink(message.url);
    if (!url) return;
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

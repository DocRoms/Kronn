import type { LivePageDetail } from '../types/generated';

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

/**
 * Inject the security policy before Page-authored markup and a tiny, local
 * data bridge. The iframe itself must still use `sandbox="allow-scripts"`
 * without `allow-same-origin`; CSP and sandbox are complementary boundaries.
 */
export function buildSandboxDocument(html: string, channelId: string): string {
  const safeChannel = JSON.stringify(channelId).replaceAll('<', '\\u003c');
  const head = `<meta http-equiv="Content-Security-Policy" content="${LIVE_PAGE_CSP}">`;
  const bridge = `<script>(()=>{
    const channel=${safeChannel};
    let latest=null;
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
    addEventListener('message',event=>{
      const message=event.data;
      if(!message||message.version!==1||message.channel_id!==channel)return;
      if(message.type==='kronn:page-data'){
        latest=message.data;
        dispatchEvent(new CustomEvent('kronn:page-data',{detail:latest}));
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

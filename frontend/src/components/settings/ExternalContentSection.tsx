import { useEffect, useRef, useState } from 'react';
import { AppWindow, Globe, Plus, Trash2 } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { userError } from '../../lib/userError';
import { normalizeEmbedOrigin } from '../../lib/live-page-embeds';
import { changeEmbedAllowedOrigins, useEmbedAllowedOrigins } from '../../hooks/useEmbedAllowedOrigins';
import { useAsyncGuard } from '../../hooks/useAsyncGuard';
import type { ToastFn } from '../../hooks/useToast';

export const EXTERNAL_CONTENT_SECTION_ID = 'settings-artifacts';

export interface EmbedOriginPrefill {
  origin: string;
  /** Changes on every request, so asking twice for the same site refocuses it. */
  nonce: number;
}

/**
 * Configuration → Artifacts → External content: the sites every Live Page of
 * this Kronn may embed content from. The same list the import dialog adds to
 * and the Page warnings link to. A prefilled site is only typed in, never
 * added: the user confirms.
 */
export function ExternalContentSection({ prefill, toast }: { prefill?: EmbedOriginPrefill | null; toast: ToastFn }) {
  const { t } = useT();
  const allowed = useEmbedAllowedOrigins();
  const [draft, setDraft] = useState('');
  const card = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);

  // A new prefill request types its site in (adjusting state during render,
  // not in an effect); the effect below only scrolls and focuses.
  const [appliedNonce, setAppliedNonce] = useState<number | null>(null);
  if (prefill && prefill.nonce !== appliedNonce) {
    setAppliedNonce(prefill.nonce);
    setDraft(prefill.origin);
  }
  useEffect(() => {
    if (!prefill) return;
    // Two frames: one for the page to render this section, one for layout.
    requestAnimationFrame(() => requestAnimationFrame(() => {
      card.current?.scrollIntoView?.({ behavior: 'smooth', block: 'start' });
      input.current?.focus({ preventScroll: true });
    }));
  }, [prefill]);

  const normalized = draft.trim() ? normalizeEmbedOrigin(draft) : null;
  const duplicate = normalized !== null && allowed?.has(normalized) === true;
  const problem = !draft.trim() ? null
    : normalized === null ? t('settings.embeds.invalid')
      : duplicate ? t('settings.embeds.duplicate')
        : null;

  const apply = useAsyncGuard(async (change: { add?: string[]; remove?: string[] }) => {
    try {
      await changeEmbedAllowedOrigins({ add: change.add ?? [], remove: change.remove ?? [] });
      if (change.add) setDraft('');
    } catch (err) {
      toast(t('common.actionFailed', userError(err)), 'error');
    }
  });
  const add = () => {
    if (normalized && !duplicate) void apply({ add: [normalized] });
  };

  const list = allowed ? [...allowed] : [];
  return (
    <div id={EXTERNAL_CONTENT_SECTION_ID} ref={card} className="set-card" data-testid="settings-external-content">
      <div className="set-section">
        <div className="flex-row gap-6 set-section-header-lg">
          <AppWindow size={14} className="text-accent" />
          <span className="font-semibold text-lg">{t('config.artifacts')}</span>
        </div>

        <div className="flex-row gap-4 mb-3">
          <Globe size={12} className="text-muted" />
          <span className="label">{t('settings.embeds.title')}</span>
          <span className="text-sm text-dim" style={{ marginLeft: 'auto' }}>{t('settings.embeds.subtitle')}</span>
        </div>
        <p className="text-sm text-faint mb-4">{t('settings.embeds.explanation')}</p>

        <label className="label" htmlFor="settings-embed-origin">{t('settings.embeds.originLabel')}</label>
        <div className="flex-row gap-3 mt-2">
          <input
            id="settings-embed-origin"
            ref={input}
            type="url"
            inputMode="url"
            autoComplete="off"
            spellCheck={false}
            className="set-input set-input-sm flex-1"
            placeholder="https://player.example.com"
            value={draft}
            aria-invalid={problem ? true : undefined}
            aria-describedby="settings-embed-origin-status"
            onChange={event => setDraft(event.target.value)}
            onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); add(); } }}
          />
          <button
            type="button"
            className="set-icon-btn text-accent"
            style={{ padding: '4px 8px' }}
            disabled={!normalized || duplicate}
            onClick={add}
          >
            <Plus size={12} /> {t('common.add')}
          </button>
        </div>
        <p id="settings-embed-origin-status" className="text-sm mt-2" role="status" data-testid="settings-embed-origin-status">
          {problem
            ? <span className="text-error">{problem}</span>
            : normalized && <span className="text-dim">{t('settings.embeds.willSave')} <code className="set-code">{normalized}</code></span>}
        </p>

        <div className="mt-4" data-testid="settings-embed-origins">
          {allowed && list.length === 0 && <p className="text-sm text-dim">{t('settings.embeds.empty')}</p>}
          {list.map(origin => (
            <div key={origin} className="flex-row gap-4 py-2" data-origin={origin}>
              <code className="set-code text-sm flex-1 truncate">{origin}</code>
              <button
                type="button"
                className="set-icon-btn"
                style={{ padding: '2px 6px' }}
                onClick={() => void apply({ remove: [origin] })}
              >
                <Trash2 size={10} className="text-error" style={{ opacity: 0.6 }} /> {t('settings.embeds.revoke')}
              </button>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
